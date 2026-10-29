# Quick dev-run helper: kills any already-running app.exe first (a leftover
# instance locks the binary and makes `cargo run` fail to relink on Windows),
# then builds and runs the app.
Stop-Process -Name app -ErrorAction SilentlyContinue -Force
cargo run -p app
