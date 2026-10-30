# Builds the Windows release binary.
#
# Usage: .\scripts\build-windows.ps1
#
# Linux has its own release build script: scripts/build-linux.sh.
# macOS isn't implemented yet - that capture backend is still a stub
# (crates/capture/src/macos.rs). Once it's written, add a matching
# scripts/build-macos.sh alongside this one.

Stop-Process -Name app -ErrorAction SilentlyContinue -Force
cargo build --release -p app
