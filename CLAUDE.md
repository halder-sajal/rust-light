# rustlight

A from-scratch Rust desktop app that captures the screen and drives WLED
ambient/bias LED lighting in real time (Lightpack-style). Windows-first;
Linux/macOS capture backends are scaffolded but not implemented yet.

This file is project knowledge for whoever (human or Claude) picks this up
next: architecture, how things actually work, hard-won gotchas, and what's
still missing. Read this before making changes — several of the "obvious"
ways to do things here were tried and reverted; the reasons are below so
they don't get re-discovered the expensive way.

## What it does, today

Open the app, set up your LED layout (how many LEDs per screen edge, where
numbering starts, physical gaps, sampling depth), add a WLED device by
host/port, and it continuously captures your screen, samples a color per
LED, runs it through a processing pipeline (smoothing, threshold, gamma,
color temperature, letterbox-aware cutoff), and streams it to the device(s)
over WLED's UDP realtime protocol. The Processing page shows a live preview
that's a literal mirror of what's being sent.

## Quick start

```powershell
cargo dev                      # alias for `cargo run -p app` (debug build)
.\scripts\dev.ps1              # same, but kills a stray app.exe first
                                # (Windows locks the binary file while a
                                # previous instance is still running)
.\scripts\build-windows.ps1    # release build -> target\release\app.exe
cargo test --workspace         # all unit tests (44 as of this writing)
```

**Always test/measure on the release build, not debug.** The processing
pipeline runs noticeably slower and less consistently in debug mode (we
measured ~18ms vs ~5.5ms for a 276-LED layout) — don't draw performance
conclusions from `cargo run`/`dev.ps1`.

## Workspace layout

```
crates/wled-proto/   WLED UDP realtime protocol sender. No app/UI deps.
crates/capture/      Screen capture, behind a ScreenCapturer trait.
crates/pipeline/     Pure color-processing logic (sampling, smoothing,
                      effects). No network or OS capture deps - takes
                      capture::Frame in, produces Vec<Rgb> out.
app/                 Slint GUI + glue: config persistence, the LED-layout
                      wizard, and the background send thread that wires
                      capture -> pipeline -> wled-proto together live.
```

Each crate is independently testable and has no knowledge of the layers
above it. `app` is the only crate that depends on all three.

### `crates/wled-proto`

Implements WARLS/DRGB/DRGBW/DNRGB over UDP, port 21324 default. See the
protocol reference at the bottom of this file. `WledSender::new(addr)`
opens a connected UDP socket; `.with_timeout(secs)` sets the realtime-mode
timeout byte. We use **DNRGB** (`send_dnrgb_frame`) for actual sends — it
auto-splits across packets for any LED count, so it works whether you have
60 LEDs or 600 without special-casing.

### `crates/capture`

```rust
pub trait ScreenCapturer {
    fn list_displays(&self) -> Result<Vec<DisplayId>, CaptureError>;
    fn capture(&mut self, display: &DisplayId) -> Result<Frame, CaptureError>;
    fn try_capture(&mut self, display: &DisplayId) -> Result<Option<Frame>, CaptureError>;
}
```

Use `capture::PlatformCapturer` and `capture::available_displays()` —
these always resolve to whichever backend matches the build target. Never
name `WindowsCapturer`/`LinuxCapturer`/`MacosCapturer` directly outside
their own module or a deliberately platform-specific example/test.

- **`windows.rs`** (implemented): wraps the `windows-capture` crate
  (Windows.Graphics.Capture). Push-based API internally
  (`on_frame_arrived` callback from WGC's own thread) bridged to
  `ScreenCapturer`'s pull-based interface via a size-1 `sync_channel`
  (latest-frame-wins; the callback uses `try_send` and drops the frame if
  the channel is still full, rather than blocking the capture thread).
- **`linux.rs`** / **`macos.rs`** (stubs): implement the trait, compile
  cleanly on their target, every method returns `CaptureError::NotImplemented`.
  `capture`'s Windows-only dependency is already scoped to
  `[target.'cfg(windows)'.dependencies]` in `crates/capture/Cargo.toml`, so
  building for another OS doesn't try to pull in `windows-capture` at all.
  Likely implementation path when someone picks this up: Linux via
  PipeWire + xdg-desktop-portal's ScreenCast portal (works across Wayland
  compositors, asks permission like WGC does) with an X11 fallback; macOS
  via ScreenCaptureKit (also push-based, so the same channel-bridge pattern
  from `windows.rs` should carry over directly).

**`capture()` vs `try_capture()` — this distinction exists for a real
reason, don't collapse it back to one method.** `capture()` blocks until a
frame is available. On Windows.Graphics.Capture (and, per Microsoft's own
docs, DXGI Desktop Duplication too — the older API Lightpack uses, see
below) **a new frame is only delivered when the screen content actually
changes**, not on a fixed clock. That means `capture()` can block for an
arbitrarily long time on a static screen. That's fine and correct on a
*dedicated thread* (see "Send architecture" below) — but calling it from a
fixed UI timer either stalls the UI or, if you reach for a short timeout
instead, re-introduces an artificial latency floor. `try_capture()` exists
for contexts where blocking is unacceptable (we no longer use it in `app`
after moving to the dedicated thread, but it's there for a settings-preview
style use case if one comes back).

### `crates/pipeline`

Trait boundaries: `Sampler` (frame -> `Vec<Rgb>`), `Smoother` (frame-to-frame
or spatial blending), `Effect` (in-place color transform). Concrete
implementations, applied in this order by `app`'s send thread:

1. **`ZoneSampler`** — averages pixels inside each LED's rectangle
   (`Zone { x, y, w, h }`, relative 0..1 coords). `letterbox_cutoff`
   (0.0..1.0) de-weights dark pixels more aggressively near a zone's
   *outer* edge (facing the screen border) than its *inner* edge (facing
   screen center) — so a letterbox bar doesn't dilute a bright highlight
   elsewhere in the same zone. At full cutoff, a single bright pixel in an
   otherwise-black zone can dominate the average. Samples every other pixel
   on each axis (`STEP = 2`) for speed; this is a preview-grade average,
   not per-pixel-precise.
2. **`BlackPoint`** — bidirectional "Threshold" (`amount: -1.0..1.0`,
   0.0 = neutral). Negative crushes dark colors toward black; positive lifts
   everything including blacks. **Not** a one-directional cutoff — an
   earlier version was, and got explicitly corrected: the slider's middle
   must mean "nothing applied," not "0 = full black."
3. **`SpatialSmoother`** — blends each LED with its neighbors.
   `amount: 0.0..1.0` scales **both** per-pass blend weight **and** number
   of passes (1 to `MAX_PASSES = 8`). Only scaling blend weight (an earlier
   version) made high settings feel weak, because it only ever blended with
   *immediate* neighbors regardless of the slider — the blend *radius*
   needs to grow too.
4. **`ColorTemperature`** — shifts R/G channels oppositely
   (`amount: -1.0..1.0`), G untouched.
5. **`GammaCorrection`** — power-law curve, applied **last**, right before
   send — it's compensating for the LED strip's own response curve, not the
   capture.

Run `cargo run --release --example bench -p pipeline` to re-measure
processing time after changing this pipeline (it uses a representative
60-LED layout; real timing scales roughly with actual LED count).

### `app`

- **`config.rs`** — `Config` persisted as TOML via
  `directories::ProjectDirs::from("dev", "rustlight", "rustlight")`, at
  (on Windows) `%AppData%\rustlight\rustlight\config\config.toml`. Sections:
  `display`, `devices` (`Vec<DeviceConfig>`), `led` (`LedConfig`),
  `processing` (`ProcessingConfig`).

  **Gotcha that actually bit us once:** every field in `ProcessingConfig`
  needs `#[serde(default = "...")]` (not just a struct-level
  `impl Default`). A struct-level `Default` only helps when the *whole
  section* is missing from the file — if the section exists but is missing
  one newly-added/renamed field, `toml::from_str` fails to parse
  `ProcessingConfig`, which fails the whole `Config` parse, which silently
  falls back to `Config::default()` for *everything*, including your LED
  layout. This happened for real after a field rename (`black_point` ->
  `threshold`, `black_cutoff` -> `letterbox_cutoff`) and looked like "my
  saved layout vanished." Any new field on `ProcessingConfig` (and ideally
  any struct embedded in `Config`) needs its own `#[serde(default = ...)]`.

- **`led_layout.rs`** — `compute_default_zones(counts, direction, start,
  depth_pct, corner_overlap_pct, gaps) -> Vec<LedZone>`. Two-step: lay out
  a canonical always-clockwise walk starting at the top-left corner
  (gap-aware per-edge slot sizing), then reorder into logical (LED #1..N)
  order based on `direction`/`start`. Numbering direction/start position
  never change where a LED *physically* sits, only which index it gets.

  - **Gaps** are edge-local: `after_led` is a 0-based count of LEDs on that
    specific edge, in that edge's own canonical walk order (not global
    numbering). A gap genuinely shrinks that edge's LED slot width/height
    (eats into the edge's usable length) — it's not cosmetic.
  - **Corner tapering**: every edge's LEDs used to sample a full-depth
    strip regardless of position, so near each corner, the last few LEDs of
    one edge and the first few of the adjacent edge claimed the same
    depth×depth screen square. `taper_depth()` shrinks the *inward-reaching*
    side of a corner-adjacent LED's zone (keeping it flush with its own
    screen border) as it approaches a corner. `corner_overlap_pct`
    (0..100, user-facing "Corner overlap" slider) controls how aggressive
    this is: 0 = clean diagonal seam (floor at 30% of full depth right at
    the corner), 100 = taper fully disabled (full depth everywhere, like
    before this feature existed). Needed because 0% overlap can miss bright
    corner content (e.g. letterboxed video reaching into the corner) — the
    slider exists specifically to trade a little overlap back for corner
    coverage.
  - This only runs when zones are *recomputed* (changing counts/gaps/
    direction/start, or hitting "Reset Layout"). Explicit saved `zones` in
    config are used as-is otherwise, so changing the algorithm doesn't
    retroactively touch an already-tuned, manually-dragged layout.

- **`main.rs`** — Slint window/tray setup, per-page callback wiring, and
  the send architecture below.

#### Send architecture (capture -> pipeline -> WLED)

A **dedicated background thread** (`spawn_send_thread`, started once at the
end of `main()`) owns the capture session and runs forever:

```rust
loop {
    let frame = capturer.capture(&display_id)?;  // blocks - correct here,
                                                   // not the UI thread
    let params = live.lock().unwrap().clone();    // snapshot of live settings
    // sample -> threshold -> smooth -> color temp -> gamma
    // send DNRGB to every configured device
    // push preview + connected-status to the UI thread
}
```

This went through two prior designs worth knowing about, because the
reasoning generalizes to anything else that touches capture timing:

1. **Fixed 150ms UI-thread timer + blocking `capture()`.** Built as a
   "good enough for a settings preview" shortcut. Turned out to add up to
   150ms of artificial latency on top of whatever `capture()` itself
   blocked for (10-140ms, since WGC only delivers on change) - and risked
   stalling the whole UI on a static screen, since a Slint timer callback
   blocking indefinitely is bad.
2. **Non-blocking `try_capture()` polled every ~16ms.** Fixed the stall
   risk and shrank the artificial gate from 150ms to 16ms, but it's still a
   polling floor.
3. **Current: dedicated thread, blocking `capture()`, zero polling.**
   Reacts the instant a frame actually arrives, no gate at all. This is the
   design the original code comment predicted would eventually be needed -
   it was right.

State the thread needs (LED zones, processing settings, device list) lives
in a small `Send` snapshot struct (`LiveParams`) behind `Arc<Mutex<_>>`,
kept in sync from the UI thread's existing callbacks (every place that
mutates the `Rc<RefCell<Config>>` used for UI/persistence also calls
`sync_live()`). Results go back to the UI thread via
`slint::invoke_from_event_loop` (confirmed `Send`/`Sync`-safe: `Weak<T>` and
`SharedString`/generated row structs all have explicit `unsafe impl Send`
in `i-slint-core`, so this is an intentionally-supported cross-thread
pattern, not something we're getting away with by luck).

**Measured** (release build, 276-LED layout, real WLED device on LAN):
processing ~5.5-7ms, UDP send ~0.05ms — both negligible. Remaining latency
is `capture-wait` (10-60ms during active screen motion, much higher on an
idle screen) - that's genuinely how often the screen is changing, not
something software here can shorten further. This was cross-checked
against another open-source ambient-lighting app's Windows capture backend
(DXGI Desktop Duplication, the older pre-WGC API): it confirmed the same
"only delivers on change" OS-level behavior, and that app polls at a fixed
50ms-default interval rather than blocking — our dedicated-thread design has
no such floor at all, so it's strictly tighter than that default.

**"Connected" status in the Devices page** means "the host resolved and
the OS accepted the socket, confirmed by the most recent send succeeding."
UDP has no handshake - this is not a true liveness check of the physical
device.

## Slint gotchas (hard-won, don't re-guess these)

When unsure of a Slint API, **read the actual bundled source** under
`C:\Users\<user>\.cargo\registry\src\...\i-slint-compiler-<version>\`
(`builtin_elements.rs` for properties/callbacks, `widgets/<style>/*.slint`
for real widget implementations like `combobox.slint`'s `PopupWindow`
usage) rather than guessing - guessing cost real cycles earlier in this
project before that discipline was established.

- **`preferred-width`/`preferred-height`, not `width`/`height`**, on the
  top-level `Window`. Setting `width`/`height` directly pins min=max and
  **disables the OS maximize button** - a real bug we hit and fixed.
- **Layout stretching on window resize**: a plain `VerticalLayout` /
  `HorizontalLayout` with no `alignment` set, as the sole child of a
  `Rectangle`, auto-fills its parent and stretches when the window is
  maximized. Fix pattern (used on every page in `app-window.slint`):
  ```
  VerticalLayout { padding; alignment: start;
      HorizontalLayout { alignment: start;
          content := VerticalLayout { max-width: Npx; ... }
      }
  }
  ```
  This pins content to its natural size, top-left-aligned, immune to
  window resizing. A per-row `vertical-stretch: 0` patch was tried first
  and wasn't sufficient - use the wrapping pattern above, not that.
- **`TouchArea`** has `mouse-x`/`mouse-y`/`pressed-x`/`pressed-y`/`pressed`/
  `moved` - `moved` is a bare signal with no event payload, so drag math
  reads `self.mouse-x - self.pressed-x` inside the handler (accumulates
  naturally since `mouse-x` is relative to the element's current,
  already-updated position).
- **`init =>`** fires once per element instantiation - used to seed
  otherwise-free local properties from `for`-loop model data (e.g. the LED
  page's draggable zone rectangles: `init` copies the model's `x/y/w/h`
  into local `rel-x/rel-y/rel-w/rel-h` properties that `TouchArea` handlers
  can then mutate directly, avoiding a binding-vs-imperative-assignment
  conflict).
- **`PopupWindow`** (see `about-popup` in `app-window.slint`'s `Sidebar`):
  named instance, `.show()`/`.close()` called imperatively from a
  `TouchArea`'s `clicked`, `close-policy: close-on-click-outside`.
- `parent.X` is single-level only - never verified/used `parent.parent`.

## Slint licensing

We're on Slint's **Royalty-free** license tier (not GPL, not paid
Commercial). That tier's actual requirement: disclose Slint usage via
*either* the `AboutSlint` widget in an About screen/dialog reachable from
the app's top-level menu (our choice - see the Sidebar's "About" popup in
`app-window.slint`) *or* a Slint badge on the app's public download page.
It does **not** require the badge to sit in the main UI - we moved it off
the sidebar into a popup for exactly this reason. If this project ever
needs the badge removed entirely, that requires either switching to the
GPL-3.0 license (open-sourcing the app) or buying a Commercial license -
removing it while staying on the free tier would violate the license terms.

## Releasing (Windows installer via GitHub Actions)

`.github/workflows/release.yml` builds the release binary, packages it with
Inno Setup, and publishes the installer to a GitHub Release. Trigger it by
pushing a version tag:

```
git tag v0.2.0 && git push origin v0.2.0
```

(or run it manually from the Actions tab with a tag name via
`workflow_dispatch`). The installer's `MyAppVersion` is **not** read from
`Cargo.toml`/the tag automatically at the `.iss` level - the workflow passes
it in via `ISCC.exe /DMyAppVersion=<version>`, and `installer/windows.iss`
only falls back to its hardcoded `0.1.0` when that's absent (local manual
builds via `scripts/build-windows-installer.ps1`). If you add a second
release trigger path, keep passing `/DMyAppVersion` through it too, or the
installer filename/AppVersion will silently drift from the actual tag.

## Known gaps / not yet done

- **Linux/macOS capture**: scaffolded (trait + stub modules), not
  implemented. See `crates/capture/src/linux.rs` / `macos.rs` for the
  intended approach.
- **`scripts/build-macos.sh` / `build-linux.sh`**: don't exist yet: add them
  alongside their capture backend, following `build-windows.ps1`'s pattern.
- **RGBW**: `LedType::Rgbw` exists as a config option but the pipeline only
  ever produces RGB `Rgb` triples - there's no white-channel computation
  anywhere downstream yet, and sending always uses DNRGB (RGB-only).
- **Multi-monitor**: `DisplayId`/`capture()` take a display argument, but
  `WindowsCapturer` always captures `Monitor::primary()` regardless of what's
  passed - the Display page's monitor picker UI exists but isn't wired to
  actually select a non-primary monitor for capture yet.
- **No per-device LED range**: every configured device receives the same
  full frame (all LEDs). No way yet to say "device A gets LEDs 1-100,
  device B gets 101-200" for a multi-controller physical setup.
- **No clean shutdown of the send thread**: it runs for the process
  lifetime; relies on the OS killing all threads on process exit. WLED's
  own realtime timeout (5s, `WLED_REALTIME_TIMEOUT_SECS` in `main.rs`)
  handles the device-side consequence of an abrupt exit gracefully, so this
  hasn't needed fixing.
- **HTTP JSON API / mDNS discovery**: out of scope, not investigated beyond
  noting they exist (see protocol section below) - UDP realtime (blasting
  colors at a known IP) is the only communication channel implemented.

## WLED UDP realtime protocol (verified)

Sources cross-checked: [kno.wled.ge/interfaces/udp-realtime](https://kno.wled.ge/interfaces/udp-realtime/)
(current docs) and the [GitHub wiki UDP-Realtime-Control page](https://github.com/wled/WLED/wiki/UDP-Realtime-Control)
(older mirror, last edited 2021 - has some stale/inconsistent notes, see
Caveats below), plus cross-checked against another open-source ambient-
lighting app's working WLED UDP sender during initial development. Both
docs sources agree with each other and with `crates/wled-proto`'s
implementation.

- **Transport:** UDP, default port **21324** (configurable in WLED's settings).
- **Packet structure:** `[byte0: protocol id][byte1: timeout][payload...]`
- **Timeout byte:** seconds to stay in realtime mode after the last packet
  before WLED reverts to normal mode. `255` = stay in realtime mode
  indefinitely (no timeout). `wled-proto` defaults to `255`; `app` overrides
  this to `5` (see "Send architecture" above).
- **Protocol ids (byte 0):**

  | id | name  | payload layout                                      | max LEDs           |
  |----|-------|------------------------------------------------------|---------------------|
  | 0  | Notifier (WLED-internal sync) | not needed for this project — do not implement |
  | 1  | WARLS | repeated `[index(1B), R, G, B]` per LED, only changed LEDs need sending | 255 (single-byte index) |
  | 2  | DRGB  | flat `R,G,B,R,G,B,...` for every LED starting at index 0 | ~490 (UDP packet size limit) |
  | 3  | DRGBW | flat `R,G,B,W,R,G,B,W,...` | ~367 |
  | 4  | DNRGB | `[startIndex high byte][startIndex low byte]` then flat `R,G,B,...` for that chunk; send multiple packets to cover more LEDs than fit in one | ~489 per packet |

- **DNRGB start index byte order:** high byte first, then low byte
  (`(startIndex >> 8)`, then `startIndex & 0xFF)`) — implemented/tested in
  `crates/wled-proto/src/lib.rs`.
- **Diffing is optional, not protocol-mandated:** a sender can send only
  changed LEDs/chunks in WARLS and DNRGB as a bandwidth optimization; DRGB
  always resends the full frame because it has no indexing. `rustlight`
  always sends full frames (via DNRGB) - no diffing implemented, and at
  measured send costs (~0.05ms) there's been no reason to add it.
- **Empty/keepalive packet:** if nothing changed and the timeout isn't
  infinite, a sender should still send a header-only (or header + zero
  start index) packet so WLED doesn't fall out of realtime mode.
  `wled-proto::WledSender::send_keepalive` implements this; `app` doesn't
  currently call it (its 5s timeout + continuous real sends make it
  unnecessary in practice, but if the send thread ever gets a "pause when
  idle" feature, wire this in then).

### What communication needs still exist beyond UDP realtime

UDP realtime only pushes pixel colors — it cannot discover devices or read
device state/config. If `rustlight` ever needs more than "blast colors at a
known IP":

- **HTTP JSON API** (`GET/POST http://<device-ip>/json/...`) — device info,
  state, presets, effects, segments, LED count.
- **mDNS discovery** (`_wled._tcp.local`) — find WLED devices on the LAN
  without knowing their IP ahead of time.

Neither is implemented. Don't start implementing either without confirming
scope first — they're bigger asks than UDP realtime was.

### Caveats / things to double-check if protocol behavior seems off

- The GitHub wiki page's max-LED numbers (255/490/367/489) are stated without
  a derivation formula on the page; they fall out of the ~1472-byte practical
  UDP payload size divided by bytes-per-LED for each protocol, rounded down,
  minus the 2 (or 4, for DNRGB) header bytes. If real-world send sizes
  misbehave, recompute from actual MTU rather than trusting the table blindly.
  - DRGB: (1472 - 2) / 3 ≈ 490
  - DRGBW: (1472 - 2) / 4 ≈ 367
  - DNRGB: (1472 - 4) / 3 ≈ 489
  - WARLS: capped at 255 by the single-byte index, not by packet size.
- The wiki page has an internal inconsistency where a Hyperion-integration
  section mentions "protocol 0 on port 19446," which contradicts the 21324
  default stated elsewhere on the same page. Protocol 0 (Notifier) is out of
  scope for this project anyway — ignore it.
- Always prefer `kno.wled.ge` (current docs) over the GitHub wiki mirror if
  they ever disagree; the wiki page is a known-stale copy.
