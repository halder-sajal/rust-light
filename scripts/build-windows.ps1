# Builds the Windows release binary.
#
# Usage: .\scripts\build-windows.ps1
#
# macOS/Linux builds aren't implemented yet - their capture backends are
# still stubs (crates/capture/src/macos.rs, linux.rs). Once one of those is
# actually written, add a matching scripts/build-macos.sh or
# scripts/build-linux.sh alongside this one.

Stop-Process -Name app -ErrorAction SilentlyContinue -Force
cargo build --release -p app
