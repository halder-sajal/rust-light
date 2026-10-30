//! Measures, on the real display: (1) how fast WindowsCapturer actually
//! delivers frames, and (2) how fast the full processing pipeline (sample
//! with letterbox cutoff + threshold + max-pass smoothing + color temp +
//! gamma, all at worst-case/strongest settings) can chew through them.
//! Not a permanent feature — a one-off measurement tool, kept as an
//! example so it's easy to re-run after future pipeline changes.
//!
//! Default run is a fixed 120 frames (quick regression check). Pass a
//! duration in seconds as the first arg to instead run for that long,
//! capturing however many frames actually arrive - e.g.
//! `cargo run --release --example bench -p pipeline -- 60` while playing a
//! video or game, to measure real sustained throughput under genuine
//! screen motion rather than whatever's static on screen at the moment.

#[cfg(windows)]
fn main() {
    use capture::{DisplayId, PlatformCapturer, ScreenCapturer};
    use pipeline::{BlackPoint, ColorTemperature, Effect, GammaCorrection, Sampler, Smoother};
    use pipeline::{SpatialSmoother, Zone, ZoneSampler};
    use std::time::{Duration, Instant};

    let duration_secs: Option<f64> = std::env::args().nth(1).and_then(|s| s.parse().ok());

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
    if let Some(d) = duration_secs {
        println!("running for {d:.0}s (capturing as many frames as arrive)...");
    } else {
        println!("running a fixed 120 frames (pass a duration in seconds for a timed run)...");
    }

    let mut capturer = PlatformCapturer::new().expect("failed to start capture");
    let id = DisplayId("0".to_string());

    // Warm up: first few frames after starting capture can be slow/stale.
    for _ in 0..5 {
        let _ = capturer.capture(&id);
    }

    let mut capture_times = Vec::new();
    let mut process_times = Vec::new();
    let mut total_times = Vec::new();

    let run_start = Instant::now();
    let deadline = duration_secs.map(|d| run_start + Duration::from_secs_f64(d));

    loop {
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                break;
            }
        } else if total_times.len() >= 120 {
            break;
        }

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

    let elapsed = run_start.elapsed();
    println!(
        "\ncaptured {} frames in {:.1}s -> {:.1} fps overall sustained\n",
        total_times.len(),
        elapsed.as_secs_f64(),
        total_times.len() as f64 / elapsed.as_secs_f64()
    );

    report("capture only", &capture_times);
    report("process only (worst-case settings)", &process_times);
    report("combined (one real loop iteration)", &total_times);
}

#[cfg(windows)]
fn report(label: &str, samples: &[std::time::Duration]) {
    let n = samples.len() as f64;
    let total: std::time::Duration = samples.iter().sum();
    let avg = total.as_secs_f64() / n;

    let mut sorted = samples.to_vec();
    sorted.sort();
    let pct = |p: f64| -> f64 {
        let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[idx].as_secs_f64() * 1000.0
    };

    println!(
        "{label}: avg={:.2}ms min={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms  -> {:.1} fps sustained (n={})",
        avg * 1000.0,
        pct(0.0),
        pct(0.50),
        pct(0.95),
        pct(0.99),
        pct(1.0),
        1.0 / avg,
        samples.len(),
    );
}

#[cfg(not(windows))]
fn main() {
    eprintln!("this example is Windows-only");
}
