//! Linux backend: two capture paths, tried in order.
//!
//! **Path one, direct to GNOME/Mutter** (`mutter` module below): calls
//! Mutter's own private `ScreenCast` + `DisplayConfig` D-Bus interfaces
//! straight up, with no `xdg-desktop-portal` involved at all. This is
//! automatic - real monitor enumeration (connector names, resolutions,
//! via `mutter::monitors()`), no dialog, no one-time setup, same as
//! `windows.rs`'s `Monitor::enumerate()` + silent capture. Verified
//! against a live GNOME 51/Mutter session during development: direct
//! D-Bus calls for both monitor listing and starting a cast, through to
//! a real PipeWire node id, no prompt at any step.
//!
//! The catch, and it's a real one: this is GNOME-only (Mutter's private
//! D-Bus API; KDE/KWin has its own different private protocol, and
//! wlroots compositors have their own Wayland-level protocols - neither
//! implemented here), and it's explicitly an *unstable, private*
//! interface ("not intended to be used outside of the integrated system
//! that uses libmutter," per Mutter's own XML doc comment) that could
//! change on any Mutter upgrade with no notice, unlike the portal, which
//! is a stable cross-desktop spec. It also has no user-facing consent
//! step at all (GNOME Shell still shows its usual screen-cast indicator
//! in the top bar while a session is active, but there's no "allow
//! this?" prompt) - acceptable for a trusted, non-sandboxed app the user
//! installed and runs themselves (this is exactly how GNOME's own
//! `gnome-remote-desktop` and third-party tools like Sunshine's
//! KWin-equivalent path already operate), but worth knowing if this
//! code ever gets reused somewhere that distinction matters (e.g. a
//! Flatpak build, where the sandbox would likely block this bus name
//! anyway, naturally falling through to the portal path below).
//!
//! **Path two, xdg-desktop-portal's ScreenCast portal + PipeWire**
//! (`open_portal`): the fallback, used whenever the Mutter path isn't
//! available (any non-GNOME compositor with a portal backend - KDE/KWin,
//! or a wlroots compositor via `xdg-desktop-portal-wlr`) or fails for any
//! reason. Unlike the Mutter path, there's no API to enumerate monitors
//! ahead of time here: the compositor shows its own picker dialog the
//! moment a session starts, and hands back a PipeWire stream for
//! whatever source the user picked there. That dialog is unavoidable the
//! *first* time - that's Wayland's security model working as intended
//! for a mediated, portal-brokered app, true for every such app (OBS
//! included), not something a different capture library would get
//! around. After that, `open_portal` uses `PersistMode::ExplicitlyRevoked`
//! and caches the restore token the portal hands back (see
//! `restore_token_path`), so every later run through this path silently
//! reconnects to the same previously-approved source with no dialog -
//! until the user revokes that grant from their desktop's privacy
//! settings, which re-triggers one.
//!
//! Both paths end up with a PipeWire node id, and from there share
//! everything else in this file: `run_pipewire_thread` connects to
//! PipeWire (directly via the default local socket for the Mutter path,
//! or via the fd the portal hands over for sandboxed-style access - see
//! its `fd: Option<OwnedFd>` parameter) and negotiates a video format.
//! Frame delivery mirrors `windows.rs`'s push-to-pull bridge: PipeWire
//! calls our `process` callback from a dedicated thread it manages via
//! its own mainloop, and we forward each frame through a size-1
//! `sync_channel` (latest-frame-wins, same reasoning as the Windows
//! backend: ambient lighting only ever wants the current screen state).
//!
//! Only RGB-family pixel formats (RGB/RGBA/RGBx/BGRx/BGRA and their
//! x-prefixed variants) are converted to this crate's RGBA8 `Frame` -
//! YUV formats aren't handled. This isn't a real limitation in practice:
//! screencast producers read straight from a GPU/CPU framebuffer and
//! offer raw RGB, not YUV (YUV is a video-codec concern that doesn't
//! apply here), and `select_sources`'s format offer below only lists
//! RGB-family formats in the first place, so the compositor has nothing
//! else to negotiate down to.

use crate::{CaptureError, DisplayId, DisplayInfo, Frame, ScreenCapturer};
use pipewire as pw;
use pw::spa;
use pw::spa::param::video::VideoFormat;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::thread::JoinHandle;

struct Terminate;

/// Lists capturable displays without starting a capture session - for
/// populating a display picker in settings UI.
///
/// Real monitor info (connector, name, resolution) when Mutter's
/// `DisplayConfig` is reachable; otherwise falls back to a single
/// placeholder entry, since the portal path has no enumeration API at all
/// (see module docs).
pub fn available_displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    if let Some(monitors) = mutter::monitors() {
        return Ok(monitors);
    }
    Ok(vec![DisplayInfo {
        id: DisplayId("portal".to_string()),
        name: "Choose via system screen-share dialog".to_string(),
        width: 0,
        height: 0,
    }])
}

/// GNOME/Mutter's own private D-Bus interfaces for display info and
/// screen casting - see the module docs above for why this exists and
/// what it costs.
mod mutter {
    use crate::{DisplayId, DisplayInfo};
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zbus::MatchRule;

    const SCREENCAST_BUS: &str = "org.gnome.Mutter.ScreenCast";
    const SCREENCAST_PATH: &str = "/org/gnome/Mutter/ScreenCast";
    const SCREENCAST_SESSION_IFACE: &str = "org.gnome.Mutter.ScreenCast.Session";
    const SCREENCAST_STREAM_IFACE: &str = "org.gnome.Mutter.ScreenCast.Stream";
    const DISPLAY_CONFIG_BUS: &str = "org.gnome.Mutter.DisplayConfig";
    const DISPLAY_CONFIG_PATH: &str = "/org/gnome/Mutter/DisplayConfig";

    // Matches `GetCurrentState`'s exact D-Bus signature
    // `a((ssss)a(siiddada{sv})a{sv})` / `a(iiduba(ssss)a{sv})`, documented
    // in Mutter's `org.gnome.Mutter.DisplayConfig.xml`.
    type ModeTuple = (
        String,
        i32,
        i32,
        f64,
        f64,
        Vec<f64>,
        HashMap<String, OwnedValue>,
    );
    type MonitorTuple = (
        (String, String, String, String),
        Vec<ModeTuple>,
        HashMap<String, OwnedValue>,
    );
    type LogicalMonitorTuple = (
        i32,
        i32,
        f64,
        u32,
        bool,
        Vec<(String, String, String, String)>,
        HashMap<String, OwnedValue>,
    );

    /// Real monitor list via `DisplayConfig.GetCurrentState` - connector
    /// names, human-readable names, and current resolution, same shape as
    /// `windows.rs`'s `Monitor::enumerate()`. `None` if Mutter's D-Bus
    /// service isn't reachable at all (not running GNOME, sandboxed with
    /// this bus name blocked, etc.) - callers fall back to the portal
    /// path. The primary monitor, if GNOME reports one, sorts first, so a
    /// caller that just wants "the" monitor can take the first entry -
    /// same default-to-primary behavior as `WindowsCapturer`.
    pub fn monitors() -> Option<Vec<DisplayInfo>> {
        let conn = Connection::session().ok()?;
        let reply = conn
            .call_method(
                Some(DISPLAY_CONFIG_BUS),
                DISPLAY_CONFIG_PATH,
                Some(DISPLAY_CONFIG_BUS),
                "GetCurrentState",
                &(),
            )
            .ok()?;
        let (_serial, monitor_tuples, logical_tuples, _props): (
            u32,
            Vec<MonitorTuple>,
            Vec<LogicalMonitorTuple>,
            HashMap<String, OwnedValue>,
        ) = reply.body().deserialize().ok()?;

        let primary_connector = logical_tuples
            .iter()
            .find(|logical| logical.4)
            .and_then(|logical| logical.5.first())
            .map(|monitor_ref| monitor_ref.0.clone());

        let mut displays: Vec<DisplayInfo> = monitor_tuples
            .into_iter()
            .map(|((connector, vendor, product, _serial), modes, props)| {
                let current_mode = modes
                    .iter()
                    .find(|mode| {
                        mode.6
                            .get("is-current")
                            .and_then(|v| bool::try_from(v.clone()).ok())
                            .unwrap_or(false)
                    })
                    .or_else(|| modes.first());
                let (width, height) = current_mode
                    .map(|mode| (mode.1.max(0) as u32, mode.2.max(0) as u32))
                    .unwrap_or_default();
                let display_name = props
                    .get("display-name")
                    .and_then(|v| String::try_from(v.clone()).ok())
                    .unwrap_or_else(|| format!("{vendor} {product}"));
                DisplayInfo {
                    id: DisplayId(connector),
                    name: format!("{display_name} ({width}x{height})"),
                    width,
                    height,
                }
            })
            .collect();

        if let Some(primary) = primary_connector {
            displays.sort_by_key(|d| d.id.0 != primary);
        }

        if displays.is_empty() {
            None
        } else {
            Some(displays)
        }
    }

    /// Opens a ScreenCast session directly against Mutter for `connector`,
    /// with no portal and no dialog, and returns the PipeWire node id once
    /// streaming actually starts - plus the `Connection` that session is
    /// tied to. **The caller must keep that connection alive for as long
    /// as it wants the stream to keep working**: Mutter tears a ScreenCast
    /// session (and its PipeWire producer node) down as soon as the D-Bus
    /// connection that created it closes, same as it would for any peer
    /// disappearing from the bus. Dropping it right after this function
    /// returns - which an earlier version of this code did - gets you a
    /// valid-looking node id for a node that's already gone by the time
    /// PipeWire tries to link to it: no error anywhere, just a stream
    /// that silently never delivers a frame.
    ///
    /// `Err` on any D-Bus failure (wrong connector, Mutter's ScreenCast
    /// service not reachable, timed out waiting for the stream to come
    /// up); callers fall back to the portal path.
    pub fn record_monitor(connector: &str) -> Result<(u32, Connection), String> {
        let conn = Connection::session().map_err(|e| e.to_string())?;

        let create_props: HashMap<&str, Value> = HashMap::new();
        let reply = conn
            .call_method(
                Some(SCREENCAST_BUS),
                SCREENCAST_PATH,
                Some(SCREENCAST_BUS),
                "CreateSession",
                &(create_props,),
            )
            .map_err(|e| e.to_string())?;
        let session_path: OwnedObjectPath =
            reply.body().deserialize().map_err(|e| e.to_string())?;

        let mut record_props: HashMap<&str, Value> = HashMap::new();
        record_props.insert("cursor-mode", Value::U32(0)); // hidden
        let reply = conn
            .call_method(
                Some(SCREENCAST_BUS),
                session_path.as_str(),
                Some(SCREENCAST_SESSION_IFACE),
                "RecordMonitor",
                &(connector, record_props),
            )
            .map_err(|e| e.to_string())?;
        let stream_path: OwnedObjectPath = reply.body().deserialize().map_err(|e| e.to_string())?;

        // Subscribe to the stream's "PipeWireStreamAdded" signal *before*
        // calling Start(), so there's no race between it firing and us
        // listening for it.
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(SCREENCAST_STREAM_IFACE)
            .map_err(|e| e.to_string())?
            .member("PipeWireStreamAdded")
            .map_err(|e| e.to_string())?
            .path(stream_path.as_str())
            .map_err(|e| e.to_string())?
            .build();
        let mut signals =
            MessageIterator::for_match_rule(rule, &conn, Some(4)).map_err(|e| e.to_string())?;

        conn.call_method(
            Some(SCREENCAST_BUS),
            session_path.as_str(),
            Some(SCREENCAST_SESSION_IFACE),
            "Start",
            &(),
        )
        .map_err(|e| e.to_string())?;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Some(Ok(msg)) = signals.next() {
                let _ = tx.send(msg.body().deserialize::<u32>().map_err(|e| e.to_string()));
            }
        });
        let node_id = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "timed out waiting for PipeWireStreamAdded".to_string())??;
        Ok((node_id, conn))
    }
}

/// Where the portal's screen-share restore token gets cached between runs,
/// so a second launch can skip the share dialog entirely (see module docs).
/// `None` if the platform has no sensible config dir - callers just skip
/// persistence in that case rather than failing.
fn restore_token_path() -> Option<std::path::PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "rustlight", "rustlight")?;
    Some(dirs.config_dir().join("linux_screencast_restore_token"))
}

fn load_restore_token() -> Option<String> {
    std::fs::read_to_string(restore_token_path()?)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_restore_token(token: &str) {
    let Some(path) = restore_token_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, token);
}

async fn open_portal() -> Result<(u32, OwnedFd), ashpd::Error> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    use ashpd::enumflags2::BitFlags;

    let cached_token = load_restore_token();

    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_restore_token(cached_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await?;

    let response = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;

    if let Some(token) = response.restore_token() {
        save_restore_token(token);
    }

    let stream = response
        .streams()
        .first()
        .ok_or(ashpd::Error::NoResponse)?
        .to_owned();
    let node_id = stream.pipe_wire_node_id();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    Ok((node_id, fd))
}

/// Byte offsets of R, G, B (and optionally A) within one pixel of `format`,
/// plus the pixel's total byte width. `None` for anything outside the
/// RGB family (see module docs).
fn channel_layout(format: VideoFormat) -> Option<(usize, usize, usize, Option<usize>, usize)> {
    Some(match format {
        VideoFormat::RGB => (0, 1, 2, None, 3),
        VideoFormat::BGR => (2, 1, 0, None, 3),
        VideoFormat::RGBA => (0, 1, 2, Some(3), 4),
        VideoFormat::RGBx => (0, 1, 2, None, 4),
        VideoFormat::BGRA => (2, 1, 0, Some(3), 4),
        VideoFormat::BGRx => (2, 1, 0, None, 4),
        VideoFormat::ARGB => (1, 2, 3, Some(0), 4),
        VideoFormat::ABGR => (3, 2, 1, Some(0), 4),
        VideoFormat::xRGB => (1, 2, 3, None, 4),
        VideoFormat::xBGR => (3, 2, 1, None, 4),
        _ => return None,
    })
}

/// Converts one raw PipeWire video buffer into this crate's tightly
/// packed, top-left-origin RGBA8 layout, honoring whatever row stride the
/// compositor used (rarely exactly `width * bpp` - there's often padding).
fn convert_to_rgba(
    format: VideoFormat,
    width: u32,
    height: u32,
    stride: i32,
    src: &[u8],
) -> Option<Vec<u8>> {
    let (ro, go, bo, ao, bpp) = channel_layout(format)?;
    let (width, height) = (width as usize, height as usize);
    let row_bytes = width * bpp;
    let stride = if stride > 0 {
        stride as usize
    } else {
        row_bytes
    };

    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let row_start = y * stride;
        if row_start + row_bytes > src.len() {
            break;
        }
        let row = &src[row_start..row_start + row_bytes];
        let out_row = &mut out[y * width * 4..(y + 1) * width * 4];
        for x in 0..width {
            let s = &row[x * bpp..x * bpp + bpp];
            let d = &mut out_row[x * 4..x * 4 + 4];
            d[0] = s[ro];
            d[1] = s[go];
            d[2] = s[bo];
            d[3] = ao.map_or(255, |o| s[o]);
        }
    }
    Some(out)
}

struct StreamUserData {
    format: spa::param::video::VideoInfoRaw,
    tx: SyncSender<Frame>,
}

/// Where a PipeWire node id came from, and whatever needs to stay alive
/// for as long as we want to keep streaming from it.
enum PipeWireSource {
    /// Direct-to-Mutter path. `connection` is the D-Bus connection that
    /// created this ScreenCast session - it must be kept alive for the
    /// stream's whole lifetime, since Mutter tears the session (and its
    /// PipeWire producer node) down the moment that connection closes
    /// (see `mutter::record_monitor`'s doc comment for how this bit us).
    Mutter {
        node_id: u32,
        connection: zbus::blocking::Connection,
    },
    /// Portal path: the fd `open_pipe_wire_remote` handed us.
    Portal { node_id: u32, fd: OwnedFd },
}

/// Runs entirely on its own thread: connects to PipeWire, negotiates a
/// video format, and forwards frames to `tx` until a `Terminate` message
/// arrives on `pw_rx`.
fn run_pipewire_thread(
    source: PipeWireSource,
    tx: SyncSender<Frame>,
    pw_rx: pw::channel::Receiver<Terminate>,
) -> Result<(), pw::Error> {
    // `_mutter_connection` only exists to be held onto - see
    // `PipeWireSource::Mutter`'s doc comment. `fd` is `Some` for the
    // portal path and `None` for the direct Mutter path, which has no fd
    // hand-off at all - we're not sandboxed, so we just connect to the
    // default local PipeWire socket like any other native client.
    let (node_id, fd, _mutter_connection) = match source {
        PipeWireSource::Mutter {
            node_id,
            connection,
        } => (node_id, None, Some(connection)),
        PipeWireSource::Portal { node_id, fd } => (node_id, Some(fd), None),
    };

    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let _terminate_listener = pw_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |Terminate| mainloop.quit()
    });

    let context = pw::context::ContextBox::new(mainloop.loop_(), None)?;
    let core = match fd {
        Some(fd) => context.connect_fd(fd, None)?,
        None => context.connect(None)?,
    };

    let stream = pw::stream::StreamBox::new(
        &core,
        "rustlight-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let user_data = StreamUserData {
        format: Default::default(),
        tx,
    };

    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let _ = user_data.format.parse(param);
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let chunk_size = datas[0].chunk().size() as usize;
            let stride = datas[0].chunk().stride();
            let width = user_data.format.size().width;
            let height = user_data.format.size().height;
            if width == 0 || height == 0 {
                return;
            }
            let Some(slice) = datas[0].data() else {
                return;
            };
            let len = chunk_size.min(slice.len());
            if let Some(rgba) = convert_to_rgba(
                user_data.format.format(),
                width,
                height,
                stride,
                &slice[..len],
            ) {
                let _ = user_data.tx.try_send(Frame {
                    width,
                    height,
                    rgba,
                });
            }
        })
        .register()?;

    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::BGRA,
            VideoFormat::RGB,
            VideoFormat::BGR,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    let values = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .expect("serializing the format pod cannot fail - it's built from valid, hardcoded values")
    .0
    .into_inner();
    let mut params =
        [spa::pod::Pod::from_bytes(&values).expect("just-serialized pod is well-formed")];

    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    mainloop.run();
    Ok(())
}

/// Captures a screen, via PipeWire - direct to Mutter if this is GNOME
/// (automatic, real monitor, no dialog), else through the
/// xdg-desktop-portal ScreenCast dialog (see module docs for both paths).
///
/// Source selection isn't wired up beyond "whichever monitor each path
/// picks" (the primary monitor for the Mutter path, whatever the portal
/// dialog returns for the fallback) - `capture()`'s `display` argument is
/// currently ignored, same as `WindowsCapturer` today.
pub struct LinuxCapturer {
    rx: Receiver<Frame>,
    terminate: Option<pw::channel::Sender<Terminate>>,
    thread: Option<JoinHandle<()>>,
}

impl LinuxCapturer {
    pub fn new() -> Result<Self, CaptureError> {
        let source = Self::open_source()?;

        let (tx, rx) = sync_channel(1);
        let (pw_tx, pw_rx) = pw::channel::channel::<Terminate>();

        let thread = std::thread::spawn(move || {
            if let Err(e) = run_pipewire_thread(source, tx, pw_rx) {
                eprintln!("rustlight: PipeWire capture thread exited with an error: {e}");
            }
        });

        Ok(Self {
            rx,
            terminate: Some(pw_tx),
            thread: Some(thread),
        })
    }

    /// Tries the direct Mutter path first (primary monitor, no dialog),
    /// falling back to the portal on anything other than GNOME, or if the
    /// direct path fails for any reason.
    fn open_source() -> Result<PipeWireSource, CaptureError> {
        if let Some(monitors) = mutter::monitors() {
            if let Some(primary) = monitors.first() {
                match mutter::record_monitor(&primary.id.0) {
                    Ok((node_id, connection)) => {
                        return Ok(PipeWireSource::Mutter { node_id, connection })
                    }
                    Err(e) => eprintln!(
                        "rustlight: direct Mutter screen-cast failed ({e}), falling back to the xdg-desktop-portal"
                    ),
                }
            }
        }

        async_io::block_on(open_portal())
            .map(|(node_id, fd)| PipeWireSource::Portal { node_id, fd })
            .map_err(|e| CaptureError::Backend(format!("screen-share portal request failed: {e}")))
    }
}

impl ScreenCapturer for LinuxCapturer {
    fn list_displays(&self) -> Result<Vec<DisplayId>, CaptureError> {
        Ok(available_displays()?.into_iter().map(|d| d.id).collect())
    }

    /// Blocks until the next frame arrives from the PipeWire stream.
    fn capture(&mut self, _display: &DisplayId) -> Result<Frame, CaptureError> {
        self.rx
            .recv()
            .map_err(|e| CaptureError::Backend(e.to_string()))
    }

    fn try_capture(&mut self, _display: &DisplayId) -> Result<Option<Frame>, CaptureError> {
        match self.rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(e) => Err(CaptureError::Backend(e.to_string())),
        }
    }
}

impl Drop for LinuxCapturer {
    fn drop(&mut self) {
        if let Some(tx) = self.terminate.take() {
            let _ = tx.send(Terminate);
        }
        // Don't join: the PipeWire thread only unblocks from mainloop.run()
        // once it processes that Terminate message on its own schedule,
        // and Drop shouldn't risk hanging indefinitely if that's delayed.
        drop(self.thread.take());
    }
}
