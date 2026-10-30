#!/usr/bin/env bash
# Quick dev-run helper for Linux, mirroring dev.ps1's role on Windows:
# kills a stray already-running instance first, then builds and runs the
# app. (Linux doesn't lock the binary file the way Windows does, so this
# isn't strictly required to avoid a relink failure - it's here mainly so
# a leftover background instance doesn't hold the PipeWire/portal
# ScreenCast session open and fight the new one for the screen-share
# picker dialog.)
#
# Needs a few dev packages installed system-wide - see
# scripts/build-linux.sh for the full list and why. This script doesn't
# install them for you - if `cargo run` fails with a pkg-config error,
# that's the fix.
set -euo pipefail
cd "$(dirname "$0")/.."

pkill -f 'target/debug/app' 2>/dev/null || true

exec cargo run -p app
