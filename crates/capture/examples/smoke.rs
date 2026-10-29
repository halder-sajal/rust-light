//! Manual smoke test for the Windows capture backend: grabs a few real
//! frames from the primary monitor and prints basic stats so we can
//! confirm it's not just returning zeroed/garbage buffers.

#[cfg(windows)]
fn main() {
    use capture::{DisplayId, PlatformCapturer, ScreenCapturer};

    let mut capturer = PlatformCapturer::new().expect("failed to start capture");
    let displays = capturer.list_displays().expect("failed to list displays");
    println!("displays: {displays:?}");

    for i in 0..5 {
        let frame = capturer
            .capture(&DisplayId("0".to_string()))
            .expect("capture failed");
        let pixel_count = (frame.width * frame.height) as usize;
        assert_eq!(frame.rgba.len(), pixel_count * 4, "buffer size mismatch");

        let sum: u64 = frame.rgba.iter().map(|&b| b as u64).sum();
        let avg = sum as f64 / frame.rgba.len() as f64;

        println!(
            "frame {i}: {}x{} bytes={} avg_byte={avg:.1}",
            frame.width,
            frame.height,
            frame.rgba.len(),
        );
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("this example is Windows-only");
}
