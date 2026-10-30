#!/usr/bin/env bash
# Builds the Linux release binary.
#
# Usage: ./scripts/build-linux.sh
#
# Unlike Windows, Linux doesn't lock a binary file while it's running, so
# there's no need to kill a stray already-running instance first the way
# dev.sh/build-windows.ps1 do.
#
# Needs PipeWire's dev headers installed system-wide to build the
# `capture` crate's Linux backend - see CLAUDE.md's "crates/capture"
# section:
#   sudo apt install libpipewire-0.3-dev libspa-0.2-dev
# (or the equivalent for your distro).
#
# macOS builds aren't implemented yet - that capture backend is still a
# stub (crates/capture/src/macos.rs). Once it's written, add a matching
# scripts/build-macos.sh alongside this one.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p app
