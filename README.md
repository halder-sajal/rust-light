# rustlight

A Rust desktop app that captures your screen and drives [WLED](https://kno.wled.ge/)
ambient/bias LED lighting in real time — Lightpack-style, built from scratch.

Set up your LED layout (how many LEDs per screen edge, where numbering
starts, physical gaps, sampling depth), point it at a WLED device, and it
continuously samples your screen and streams colors to your LEDs over
WLED's UDP realtime protocol, with live smoothing, threshold, gamma, color
temperature, and letterbox-aware corner sampling.

Windows-first today; Linux and macOS are scaffolded but not implemented yet
(see [CLAUDE.md](./CLAUDE.md) for the cross-platform architecture).

## Features

- **LED layout wizard** — per-edge LED counts, clockwise/counter-clockwise
  numbering from any start point, physical gaps (strip breaks), adjustable
  sampling depth, and a draggable live preview.
- **Corner overlap control** — adjacent edges taper cleanly at corners by
  default; a slider lets you trade that back for more corner coverage
  (useful for letterboxed video reaching into a corner).
- **Live processing pipeline** — smoothing, a bidirectional threshold
  (crush/lift), gamma correction, color temperature, and a position-aware
  letterbox cutoff, all previewed live before they're sent.
- **Multiple WLED devices** — add/remove by host and port; live connected
  status per device.
- **Low-latency send architecture** — a dedicated background thread reacts
  to screen changes instantly rather than polling on a timer. See
  [CLAUDE.md](./CLAUDE.md) for the measured numbers and the reasoning.

## Getting started

Requires a recent [Rust toolchain](https://rustup.rs/).

```powershell
cargo run -p app               # or: .\scripts\dev.ps1 (also kills a
                                # stray app.exe, a Windows file-lock issue
                                # with running `cargo run` twice)
```

For a release build (meaningfully faster — see [CLAUDE.md](./CLAUDE.md) for
why debug-mode numbers aren't representative):

```powershell
.\scripts\build-windows.ps1    # -> target\release\app.exe
```

Run the test suite:

```powershell
cargo test --workspace
```

## Project layout

```
crates/wled-proto/   WLED UDP realtime protocol (WARLS/DRGB/DRGBW/DNRGB)
crates/capture/      Screen capture, behind a cross-platform trait
crates/pipeline/     Color processing: sampling, smoothing, effects
app/                 Slint GUI, config persistence, the send pipeline
```

See [CLAUDE.md](./CLAUDE.md) for the full architecture writeup, the
reasoning behind some non-obvious design decisions, known gaps, and the
verified WLED UDP protocol reference.

## License

MIT — see [LICENSE](./LICENSE).

This project uses [Slint](https://slint.dev/) under its Royalty-free
license tier; usage is disclosed via the "About" popup in the app's
sidebar, per that license's terms.
