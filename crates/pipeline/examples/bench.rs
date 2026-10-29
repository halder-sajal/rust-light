//! Measures, on the real display: (1) how fast WindowsCapturer actually
//! delivers frames, and (2) how fast the full processing pipeline (sample
//! with letterbox cutoff + threshold + max-pass smoothing + color temp +
//! gamma, all at worst-case/strongest settings) can chew through them.
//! Not a permanent feature — a one-off measurement tool, kept as an
//! example so it's easy to re-run after future pipeline changes.

#[cfg(windows)]
fn main() {
    use capture::{DisplayId, PlatformCapturer, ScreenCapturer};
    use pipeline::{BlackPoint, ColorTemperature, Effect, GammaCorrection, Sampler, Smoother};
    use pipeline::{SpatialSmoother, Zone, ZoneSampler};
    use std::time::Instant;

    const N: u32 = 120;

    // A representative 60-LED perimeter zone set (20 top, 10 right, 20
    // bottom, 10 left), close enough in size/count to a real layout for
    // timing purposes.
    let mut zones = Vec::new();
    for i in 0..20 {
        zones.push(Zone { x: i as f32 / 20.0, y: 0.0, w: 1.0 / 20.0, h: 0.1 });
    }
    for i in 0..10 {
        zones.push(Zone { x: 0.9, y: i as f32 / 10.0, w: 0.1, h: 1.0 / 10.0 });
    }
    for i in 0..20 {
        zones.push(Zone { x: i as f32 / 20.0, y: 0.9, w: 1.0 / 20.0, h: 0.1 });
    }
    for i in 0..10 {
        zones.push(Zone { x: 0.0, y: i as f32 / 10.0, w: 0.1, h: 1.0 / 10.0 });
    }
    println!("zones: {}", zones.len());

    let mut capturer = PlatformCapturer::new().expect("failed to start capture");
    let id = DisplayId("0".to_string());

    // Warm up: first few frames after starting capture can be slow/stale.
    for _ in 0..5 {
        let _ = capturer.capture(&id);
    }

    let mut capture_times = Vec::with_capacity(N as usize);
    let mut process_times = Vec::with_capacity(N as usize);
    let mut total_times = Vec::with_capacity(N as usize);

    for _ in 0..N {
        let loop_start = Instant::now();

        let t0 = Instant::now();
        let frame = capturer.capture(&id).expect("capture failed");
        capture_times.push(t0.elapsed());

        let t1 = Instant::now();
        let sampler = ZoneSampler { zones: zones.clone(), letterbox_cutoff: 1.0 };
        let mut colors = sampler.sample(&frame, zones.len());
        BlackPoint { amount: -1.0 }.apply(&mut colors);
        let mut smoother = SpatialSmoother { amount: 1.0 };
        let mut colors = smoother.smooth(&colors);
        ColorTemperature { amount: 1.0 }.apply(&mut colors);
        GammaCorrection { gamma: 3.0 }.apply(&mut colors);
        process_times.push(t1.elapsed());

        total_times.push(loop_start.elapsed());
    }

    report("capture only", &capture_times);
    report("process only (worst-case settings)", &process_times);
    report("combined (one real loop iteration)", &total_times);
}

#[cfg(windows)]
fn report(label: &str, samples: &[std::time::Duration]) {
    let n = samples.len() as f64;
    let total: std::time::Duration = samples.iter().sum();
    let avg = total.as_secs_f64() / n;
    let max = samples.iter().max().unwrap();
    let min = samples.iter().min().unwrap();
    println!(
        "{label}: avg={:.2}ms min={:.2}ms max={:.2}ms  -> {:.1} fps sustained",
        avg * 1000.0,
        min.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0,
        1.0 / avg
    );
}

#[cfg(not(windows))]
fn main() {
    eprintln!("this example is Windows-only");
}
