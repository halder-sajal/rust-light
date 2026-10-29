// No console window in release builds (debug builds keep it, in case
// println!/eprintln! debugging is needed) — without this, Rust's default
// "console" subsystem opens a terminal alongside the GUI, and closing that
// terminal kills the app too.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Entry point. GUI: Slint (window + tray). A dedicated background thread
//! owns the capture session: it blocks on each frame (correct there, since
//! it's not the UI thread), runs the full color pipeline, sends the result
//! to every configured WLED device over UDP, and pushes the same colors to
//! the Processing page preview — so the preview is a literal mirror of
//! what's being sent, and send latency is bounded only by how fast the
//! screen actually changes, never by polling or UI-thread contention.

mod config;
mod led_layout;

slint::include_modules!();

use capture::ScreenCapturer;
use pipeline::{Effect, Sampler, Smoother};
use std::cell::RefCell;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// Index of the Processing page in the sidebar (`selected-page` in the
/// Slint UI) — the send thread only needs to build/push the live preview
/// when this page is actually visible.
const PROCESSING_PAGE_INDEX: i32 = 3;

/// WLED stays in realtime mode for this many seconds after the last packet
/// before reverting to normal mode on its own — short enough that closing
/// or crashing this app doesn't leave a device stuck in realtime forever.
const WLED_REALTIME_TIMEOUT_SECS: u8 = 5;

/// A UDP sender for one configured device, or `None` if the host couldn't
/// be resolved / the socket couldn't be created.
struct DeviceSender {
    sender: Option<wled_proto::WledSender>,
}

fn build_device_senders(devices: &[config::DeviceConfig]) -> Vec<DeviceSender> {
    devices
        .iter()
        .map(|d| {
            let sender = (d.host.as_str(), d.port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut addrs| addrs.next())
                .and_then(|addr| wled_proto::WledSender::new(addr).ok())
                .map(|s| s.with_timeout(WLED_REALTIME_TIMEOUT_SECS));
            DeviceSender { sender }
        })
        .collect()
}

/// The small slice of state the background send thread actually needs,
/// snapshotted out of `state` (which stays `Rc<RefCell<..>>`, UI-thread-only)
/// into something `Send` the thread can read every loop iteration.
#[derive(Clone)]
struct LiveParams {
    zones: Vec<config::LedZone>,
    processing: config::ProcessingConfig,
    devices: Vec<config::DeviceConfig>,
    max_send_fps: f32,
}

impl LiveParams {
    fn from_config(cfg: &config::Config) -> Self {
        Self {
            zones: cfg.led.zones.clone(),
            processing: cfg.processing,
            devices: cfg.devices.clone(),
            max_send_fps: cfg.max_send_fps,
        }
    }
}

/// Re-snapshots `live` from `cfg` — called on the UI thread after any
/// mutation that affects LED zones, processing settings, or the device
/// list, so the next send-thread iteration sees it.
fn sync_live(live: &Arc<Mutex<LiveParams>>, cfg: &config::Config) {
    *live.lock().unwrap() = LiveParams::from_config(cfg);
}

/// Captures, processes, and sends continuously for the lifetime of the
/// process: blocks on the next real frame (so a static screen costs
/// nothing), snapshots the latest settings, runs the pipeline, sends to
/// every configured device, and pushes the preview + connected status back
/// to the UI thread via `invoke_from_event_loop`.
fn spawn_send_thread(
    window_weak: slint::Weak<MainWindow>,
    live: Arc<Mutex<LiveParams>>,
    current_page: Arc<AtomicI32>,
) {
    std::thread::spawn(move || {
        let Ok(mut capturer) = capture::PlatformCapturer::new() else {
            return;
        };
        let display_id = capture::DisplayId("0".to_string());

        let mut senders: Vec<DeviceSender> = Vec::new();
        let mut senders_for: Vec<config::DeviceConfig> = Vec::new();
        let mut connected: Vec<bool> = Vec::new();
        // Far enough in the past that the very first frame always sends.
        let mut last_sent = std::time::Instant::now() - std::time::Duration::from_secs(1);

        // Reused across iterations instead of allocating fresh every frame.
        let mut zones_for: Vec<config::LedZone> = Vec::new();
        let mut sampler = pipeline::ZoneSampler { zones: Vec::new(), letterbox_cutoff: 0.0 };
        let mut pixels: Vec<wled_proto::Rgb> = Vec::new();

        loop {
            let Ok(frame) = capturer.capture(&display_id) else {
                break;
            };

            let params = live.lock().unwrap().clone();
            if params.zones.is_empty() {
                continue;
            }

            // Rate cap: drop this frame entirely (no sampling/processing/
            // send work at all) if it arrived before the minimum interval
            // has elapsed since the last one we actually sent.
            if params.max_send_fps > 0.0 {
                let min_interval = std::time::Duration::from_secs_f64(1.0 / params.max_send_fps as f64);
                if last_sent.elapsed() < min_interval {
                    continue;
                }
            }
            last_sent = std::time::Instant::now();

            if senders_for != params.devices {
                senders = build_device_senders(&params.devices);
                connected = vec![false; senders.len()];
                senders_for = params.devices.clone();
            }

            if zones_for != params.zones {
                sampler.zones.clear();
                sampler
                    .zones
                    .extend(params.zones.iter().map(|z| pipeline::Zone { x: z.x, y: z.y, w: z.w, h: z.h }));
                zones_for = params.zones.clone();
            }
            sampler.letterbox_cutoff = params.processing.letterbox_cutoff / 100.0;
            let mut colors = sampler.sample(&frame, params.zones.len());

            // Threshold before smoothing, so crushed/lifted noise doesn't
            // get spread onto neighbors first.
            let amount = (params.processing.threshold - 50.0) / 50.0;
            pipeline::BlackPoint { amount }.apply(&mut colors);

            let mut smoother = pipeline::SpatialSmoother { amount: params.processing.smoothing / 100.0 };
            let mut colors = smoother.smooth(&colors);

            pipeline::ColorTemperature { amount: (params.processing.color_temp - 50.0) / 50.0 }
                .apply(&mut colors);

            // Gamma last, right before send — it's compensating for the LED
            // device's own response curve, not the capture.
            let gamma = GAMMA_RANGE.powf((params.processing.gamma - 50.0) / 50.0);
            pipeline::GammaCorrection { gamma }.apply(&mut colors);

            pixels.clear();
            pixels.extend(colors.iter().map(|c| wled_proto::Rgb { r: c.r, g: c.g, b: c.b }));
            let mut changed = false;
            for (i, ds) in senders.iter().enumerate() {
                let ok =
                    ds.sender.as_ref().map(|s| s.send_dnrgb_frame(&pixels).is_ok()).unwrap_or(false);
                if connected[i] != ok {
                    connected[i] = ok;
                    changed = true;
                }
            }

            // Only build the preview when the Processing page is actually
            // visible — the UI push (allocation + cross-thread dispatch)
            // is otherwise pure waste every single frame.
            let preview_visible = current_page.load(Ordering::Relaxed) == PROCESSING_PAGE_INDEX;
            let preview_rows = if preview_visible {
                Some(
                    params
                        .zones
                        .iter()
                        .zip(colors.iter())
                        .map(|(z, c)| LedColorRow {
                            x: z.x,
                            y: z.y,
                            w: z.w,
                            h: z.h,
                            color: slint::Color::from_rgb_u8(c.r, c.g, c.b),
                        })
                        .collect::<Vec<LedColorRow>>(),
                )
            } else {
                None
            };
            let devices_for_ui = if changed { Some((params.devices.clone(), connected.clone())) } else { None };

            if preview_rows.is_none() && devices_for_ui.is_none() {
                continue;
            }

            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                if let Some(rows) = preview_rows {
                    window.set_processing_leds(slint::ModelRc::new(slint::VecModel::from(rows)));
                }
                if let Some((devices, connected)) = devices_for_ui {
                    window.set_devices(slint::ModelRc::new(slint::VecModel::from(device_rows(
                        &devices, &connected,
                    ))));
                }
            });
        }
    });
}

/// (label, width, height) — width/height 0,0 means "native" (no downscale).
const RESOLUTION_PRESETS: &[(&str, u32, u32)] = &[
    ("Native", 0, 0),
    ("1920x1080", 1920, 1080),
    ("1280x720", 1280, 720),
    ("640x360", 640, 360),
    ("320x180", 320, 180),
];

/// (label, fps) — 0.0 means unlimited (process/send as fast as frames
/// arrive).
const MAX_FPS_PRESETS: &[(&str, f32)] = &[
    ("Unlimited", 0.0),
    ("60 FPS", 60.0),
    ("30 FPS", 30.0),
    ("15 FPS", 15.0),
];

/// How far the gamma slider can push gamma away from 1.0 (neutral), in
/// either direction — e.g. 3.0 means the slider spans 1/3.0..3.0.
const GAMMA_RANGE: f32 = 3.0;

/// `connected` here just means "the host resolved and the OS accepted the
/// socket" (confirmed by the most recent send succeeding) — UDP has no
/// handshake, so this isn't a true liveness check of the device itself.
fn device_rows(devices: &[config::DeviceConfig], connected: &[bool]) -> Vec<DeviceRow> {
    devices
        .iter()
        .enumerate()
        .map(|(i, d)| DeviceRow {
            name: d.name.as_str().into(),
            host: d.host.as_str().into(),
            port: d.port as i32,
            connected: connected.get(i).copied().unwrap_or(false),
        })
        .collect()
}

fn clear_status_after(window: &MainWindow, setter: fn(&MainWindow, slint::SharedString)) {
    let window_weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(3), move || {
        if let Some(window) = window_weak.upgrade() {
            setter(&window, "".into());
        }
    });
}

const LED_EDGES: [config::Edge; 4] =
    [config::Edge::Top, config::Edge::Right, config::Edge::Bottom, config::Edge::Left];

fn led_zone_rows(zones: &[config::LedZone]) -> Vec<LedZoneRow> {
    zones.iter().map(|z| LedZoneRow { x: z.x, y: z.y, w: z.w, h: z.h }).collect()
}

fn edge_index(edge: config::Edge) -> i32 {
    LED_EDGES.iter().position(|e| *e == edge).unwrap_or(0) as i32
}

fn gap_rows(gaps: &[config::Gap]) -> Vec<GapRow> {
    gaps.iter()
        .map(|g| GapRow {
            edge_index: edge_index(g.edge),
            after_led: g.after_led as i32,
            size: g.size.round() as i32,
        })
        .collect()
}

fn recompute_led_zones(led: &config::LedConfig) -> Vec<config::LedZone> {
    led_layout::compute_default_zones(
        &led.counts,
        led.direction,
        led.start,
        led.sampling_depth_pct,
        led.corner_overlap_pct,
        &led.gaps,
    )
}

/// Recomputes zones from the window's current live inputs + `state`'s
/// current gaps, writes the result back into `state`, pushes it to the
/// Slint model so the preview updates immediately, and syncs `live` so the
/// send thread picks it up on its next iteration.
fn recompute_and_push_led_zones(
    window: &MainWindow,
    state: &Rc<RefCell<config::Config>>,
    live: &Arc<Mutex<LiveParams>>,
) {
    let gaps = state.borrow().led.gaps.clone();
    let mut led = read_led_config_from_window(window, gaps, Vec::new());
    led.zones = recompute_led_zones(&led);
    window.set_led_zones(slint::ModelRc::new(slint::VecModel::from(led_zone_rows(&led.zones))));
    state.borrow_mut().led = led;
    sync_live(live, &state.borrow());
}

/// Reads the LED form's current live values out of the window (not the
/// last-saved config) — used by both Reset and Save, since both need
/// "whatever the user has typed right now," not the on-disk state.
fn read_led_config_from_window(
    window: &MainWindow,
    gaps: Vec<config::Gap>,
    zones: Vec<config::LedZone>,
) -> config::LedConfig {
    let led_type =
        if window.get_led_type_index() == 1 { config::LedType::Rgbw } else { config::LedType::Rgb };
    let counts = config::EdgeCounts {
        top: window.get_led_count_top().max(0) as u32,
        right: window.get_led_count_right().max(0) as u32,
        bottom: window.get_led_count_bottom().max(0) as u32,
        left: window.get_led_count_left().max(0) as u32,
    };
    let direction = if window.get_led_reverse_direction() {
        config::Direction::CounterClockwise
    } else {
        config::Direction::Clockwise
    };
    let edge = LED_EDGES[window.get_led_start_edge_index().clamp(0, 3) as usize];
    let offset = (window.get_led_start_offset().max(0) as u32)
        .min(counts.for_edge(edge).saturating_sub(1));

    config::LedConfig {
        led_type,
        counts,
        direction,
        start: config::StartPosition { edge, offset },
        gaps,
        sampling_depth_pct: window.get_led_sampling_depth(),
        corner_overlap_pct: window.get_led_corner_overlap(),
        zones,
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    let tray = AppTray::new()?;

    let state = Rc::new(RefCell::new(config::Config::load()));
    {
        let mut cfg = state.borrow_mut();
        let total = cfg.led.counts.total() as usize;
        if cfg.led.zones.len() != total {
            cfg.led.zones = recompute_led_zones(&cfg.led);
        }
    }
    let live: Arc<Mutex<LiveParams>> = Arc::new(Mutex::new(LiveParams::from_config(&state.borrow())));

    // Tracks which sidebar page is visible, so the send thread can skip
    // building the Processing page preview when it isn't shown.
    let current_page: Arc<AtomicI32> = Arc::new(AtomicI32::new(0));
    {
        let current_page = current_page.clone();
        window.on_page_changed(move |index| {
            current_page.store(index, Ordering::Relaxed);
        });
    }

    // --- Display page ---
    let displays = capture::available_displays().unwrap_or_default();
    let display_labels: Vec<slint::SharedString> = if displays.is_empty() {
        vec!["No displays found".into()]
    } else {
        displays.iter().map(|d| d.name.as_str().into()).collect()
    };
    window.set_display_options(slint::ModelRc::new(slint::VecModel::from(display_labels)));

    let resolution_labels: Vec<slint::SharedString> = RESOLUTION_PRESETS
        .iter()
        .map(|(label, _, _)| (*label).into())
        .collect();
    window.set_resolution_options(slint::ModelRc::new(slint::VecModel::from(resolution_labels)));

    let max_fps_labels: Vec<slint::SharedString> =
        MAX_FPS_PRESETS.iter().map(|(label, _)| (*label).into()).collect();
    window.set_max_fps_options(slint::ModelRc::new(slint::VecModel::from(max_fps_labels)));

    {
        let cfg = state.borrow();
        let initial_display_index = if displays.is_empty() {
            0
        } else {
            (cfg.display.monitor_index as i32).clamp(0, displays.len() as i32 - 1)
        };
        window.set_selected_display_index(initial_display_index);

        let initial_resolution_index = RESOLUTION_PRESETS
            .iter()
            .position(|(_, w, h)| *w == cfg.display.capture_width && *h == cfg.display.capture_height)
            .unwrap_or(0) as i32;
        window.set_selected_resolution_index(initial_resolution_index);

        let initial_max_fps_index = MAX_FPS_PRESETS
            .iter()
            .position(|(_, fps)| *fps == cfg.max_send_fps)
            .unwrap_or(0) as i32;
        window.set_max_fps_index(initial_max_fps_index);
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_save_display_settings(move |display_index, resolution_index, max_fps_index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let (_, capture_width, capture_height) = RESOLUTION_PRESETS
                [resolution_index.clamp(0, RESOLUTION_PRESETS.len() as i32 - 1) as usize];
            let (_, max_send_fps) =
                MAX_FPS_PRESETS[max_fps_index.clamp(0, MAX_FPS_PRESETS.len() as i32 - 1) as usize];

            state.borrow_mut().display = config::DisplayConfig {
                monitor_index: display_index.max(0) as usize,
                capture_width,
                capture_height,
            };
            state.borrow_mut().max_send_fps = max_send_fps;

            let status = match state.borrow().save() {
                Ok(()) => {
                    sync_live(&live, &state.borrow());
                    "Saved".to_string()
                }
                Err(e) => format!("Failed to save: {e}"),
            };
            window.set_display_status(status.into());
            clear_status_after(&window, MainWindow::set_display_status);
        });
    }

    // --- Devices page ---
    // Real `connected` status is only known by the send thread (it owns the
    // actual UDP senders); until it reports in, a freshly added/removed
    // device list shows as not-connected.
    window.set_devices(slint::ModelRc::new(slint::VecModel::from(device_rows(
        &state.borrow().devices,
        &[],
    ))));

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_add_device(move |name, host, port_text| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let name = name.trim().to_string();
            let host = host.trim().to_string();

            if name.is_empty() || host.is_empty() {
                window.set_devices_status("Name and host are required".into());
                clear_status_after(&window, MainWindow::set_devices_status);
                return;
            }

            let port: u16 = port_text.trim().parse().unwrap_or(wled_proto::DEFAULT_PORT);

            state.borrow_mut().devices.push(config::DeviceConfig { name, host, port });

            let status = match state.borrow().save() {
                Ok(()) => {
                    sync_live(&live, &state.borrow());
                    window.set_devices(slint::ModelRc::new(slint::VecModel::from(device_rows(
                        &state.borrow().devices,
                        &[],
                    ))));
                    window.set_new_device_name("".into());
                    window.set_new_device_host("".into());
                    "Added".to_string()
                }
                Err(e) => {
                    state.borrow_mut().devices.pop();
                    format!("Failed to save: {e}")
                }
            };
            window.set_devices_status(status.into());
            clear_status_after(&window, MainWindow::set_devices_status);
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_remove_device(move |index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let index = index as usize;
            if index >= state.borrow().devices.len() {
                return;
            }

            let removed = state.borrow_mut().devices.remove(index);
            let status = match state.borrow().save() {
                Ok(()) => {
                    sync_live(&live, &state.borrow());
                    window.set_devices(slint::ModelRc::new(slint::VecModel::from(device_rows(
                        &state.borrow().devices,
                        &[],
                    ))));
                    "Removed".to_string()
                }
                Err(e) => {
                    state.borrow_mut().devices.insert(index, removed);
                    format!("Failed to save: {e}")
                }
            };
            window.set_devices_status(status.into());
            clear_status_after(&window, MainWindow::set_devices_status);
        });
    }

    // --- Led page ---
    {
        let cfg = state.borrow();

        window.set_led_type_index(if cfg.led.led_type == config::LedType::Rgbw { 1 } else { 0 });
        window.set_led_count_top(cfg.led.counts.top as i32);
        window.set_led_count_right(cfg.led.counts.right as i32);
        window.set_led_count_bottom(cfg.led.counts.bottom as i32);
        window.set_led_count_left(cfg.led.counts.left as i32);
        window.set_led_reverse_direction(cfg.led.direction == config::Direction::CounterClockwise);
        window.set_led_start_edge_index(edge_index(cfg.led.start.edge));
        window.set_led_start_offset(cfg.led.start.offset as i32);
        window.set_led_sampling_depth(cfg.led.sampling_depth_pct);
        window.set_led_corner_overlap(cfg.led.corner_overlap_pct);
        window.set_led_gaps(slint::ModelRc::new(slint::VecModel::from(gap_rows(&cfg.led.gaps))));
        window.set_led_zones(slint::ModelRc::new(slint::VecModel::from(led_zone_rows(
            &cfg.led.zones,
        ))));
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_add_led_gap(move |edge_idx, after, size| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let edge = LED_EDGES[edge_idx.clamp(0, 3) as usize];
            state.borrow_mut().led.gaps.push(config::Gap {
                edge,
                after_led: after.max(0) as u32,
                size: size.max(1) as f32,
            });
            window.set_led_gaps(slint::ModelRc::new(slint::VecModel::from(gap_rows(
                &state.borrow().led.gaps,
            ))));
            recompute_and_push_led_zones(&window, &state, &live);
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_remove_led_gap(move |index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let index = index as usize;
            if index < state.borrow().led.gaps.len() {
                state.borrow_mut().led.gaps.remove(index);
            }
            window.set_led_gaps(slint::ModelRc::new(slint::VecModel::from(gap_rows(
                &state.borrow().led.gaps,
            ))));
            recompute_and_push_led_zones(&window, &state, &live);
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_led_zone_moved(move |index, x, y| {
            let mut cfg = state.borrow_mut();
            if let Some(z) = cfg.led.zones.get_mut(index as usize) {
                z.x = x;
                z.y = y;
            }
            drop(cfg);
            sync_live(&live, &state.borrow());
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_led_zone_resized(move |index, w, h| {
            let mut cfg = state.borrow_mut();
            if let Some(z) = cfg.led.zones.get_mut(index as usize) {
                z.w = w.max(0.01);
                z.h = h.max(0.01);
            }
            drop(cfg);
            sync_live(&live, &state.borrow());
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_reset_led_layout(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            recompute_and_push_led_zones(&window, &state, &live);
            window.set_led_status("Layout updated".into());
            clear_status_after(&window, MainWindow::set_led_status);
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let live = live.clone();
        window.on_save_led_settings(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let (gaps, zones) = {
                let cfg = state.borrow();
                (cfg.led.gaps.clone(), cfg.led.zones.clone())
            };
            let mut led = read_led_config_from_window(&window, gaps, zones);

            let expected_total = led.counts.total() as usize;
            if led.zones.len() != expected_total {
                led.zones = recompute_led_zones(&led);
                window.set_led_zones(slint::ModelRc::new(slint::VecModel::from(led_zone_rows(
                    &led.zones,
                ))));
            }

            state.borrow_mut().led = led;
            let status = match state.borrow().save() {
                Ok(()) => "Saved".to_string(),
                Err(e) => format!("Failed to save: {e}"),
            };
            sync_live(&live, &state.borrow());
            window.set_led_status(status.into());
            clear_status_after(&window, MainWindow::set_led_status);
        });
    }

    // --- Processing page: live capture -> sample -> preview -> send ---
    // The actual work happens on a dedicated background thread (see
    // `spawn_send_thread`), reading `live` and pushing results back via
    // `invoke_from_event_loop`. Started once, at the end of `main`, after
    // all the callbacks below that keep `live` in sync are wired up.

    window.set_processing_smoothing(state.borrow().processing.smoothing);
    window.set_processing_threshold(state.borrow().processing.threshold);
    window.set_processing_gamma(state.borrow().processing.gamma);
    window.set_processing_color_temp(state.borrow().processing.color_temp);
    window.set_processing_letterbox_cutoff(state.borrow().processing.letterbox_cutoff);

    {
        let state = state.clone();
        let live = live.clone();
        window.on_processing_smoothing_changed(move |v| {
            state.borrow_mut().processing.smoothing = v;
            sync_live(&live, &state.borrow());
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_processing_threshold_changed(move |v| {
            state.borrow_mut().processing.threshold = v;
            sync_live(&live, &state.borrow());
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_processing_gamma_changed(move |v| {
            state.borrow_mut().processing.gamma = v;
            sync_live(&live, &state.borrow());
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_processing_color_temp_changed(move |v| {
            state.borrow_mut().processing.color_temp = v;
            sync_live(&live, &state.borrow());
        });
    }

    {
        let state = state.clone();
        let live = live.clone();
        window.on_processing_letterbox_cutoff_changed(move |v| {
            state.borrow_mut().processing.letterbox_cutoff = v;
            sync_live(&live, &state.borrow());
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_save_processing_settings(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let status = match state.borrow().save() {
                Ok(()) => "Saved".to_string(),
                Err(e) => format!("Failed to save: {e}"),
            };
            window.set_processing_status(status.into());
            clear_status_after(&window, MainWindow::set_processing_status);
        });
    }

    // --- window/tray lifecycle ---
    let window_weak = window.as_weak();
    tray.on_show_window(move || {
        if let Some(w) = window_weak.upgrade() {
            let _ = w.show();
        }
    });

    tray.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    let window_weak = window.as_weak();
    window.on_hide_to_tray(move || {
        if let Some(w) = window_weak.upgrade() {
            let _ = w.hide();
        }
    });

    spawn_send_thread(window.as_weak(), live.clone(), current_page.clone());

    // `--minimized`: start hidden in the tray (used by the Windows
    // installer's optional "start with Windows" autostart entry).
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    if !start_minimized {
        window.show()?;
    }
    tray.show()?;
    slint::run_event_loop()?;
    Ok(())
}
