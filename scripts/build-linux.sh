#!/usr/bin/env bash
# Builds the Linux release binary.
#
# Usage: ./scripts/build-linux.sh
#
# Unlike Windows, Linux doesn't lock a binary file while it's running, so
# there's no need to kill a stray already-running instance first the way
# dev.sh/build-windows.ps1 do.
#
# Needs a few dev packages installed system-wide - PipeWire's for the
# `capture` crate's Linux backend (see CLAUDE.md's "crates/capture"
# section), the rest for Slint's GUI backend (see its own docs/building.md):
#   sudo apt install libpipewire-0.3-dev libspa-0.2-dev \
#     libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libfontconfig-dev
# (or the equivalent for your distro). A desktop Linux install usually has
# the Slint-related ones already as part of the desktop environment - a
# bare/CI machine won't, which is how this got missed until the release
# workflow's `ubuntu-latest` runner hit it first.
#
# macOS builds aren't implemented yet - that capture backend is still a
# stub (crates/capture/src/macos.rs). Once it's written, add a matching
# scripts/build-macos.sh alongside this one.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p app
