//! True end-to-end benchmark: real screen capture, your real saved LED
//! layout and processing settings (loaded straight from the live
//! config.toml), real UDP DNRGB sends to every device configured in the
//! app - the same per-frame work `app`'s send thread does in
//! `spawn_send_thread`, instrumented with separate timing for each stage.
//! This genuinely drives your real LEDs for the run's duration, so it
//! doubles as a visual sanity check as well as a timing measurement.
//!
//! Needs a config.toml with at least one device and a saved LED layout -
//! run the real app once and set those up first if you haven't.
//!
//! Run: `cargo run --release --example e2e_bench -p pipeline -- 60`
//! (duration in seconds, default 60 if omitted). Play a video or game
//! during the run so the capture side has real motion to react to.

#[cfg(windows)]
fn main() {
    use capture::{DisplayId, PlatformCapturer, ScreenCapturer};
    use pipeline::{BlackPoint, ColorTemperature, Effect, GammaCorrection, Sampler, Smoother};
    use pipeline::{SpatialSmoother, Zone, ZoneSampler};
    use std::time::{Duration, Instant};
    use wled_proto::WledSender;

    // Mirrors app/src/main.rs's constants of the same name.
    const GAMMA_RANGE: f32 = 3.0;
    const WLED_REALTIME_TIMEOUT_SECS: u8 = 5;

    let duration_secs: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60.0);

    let config_path = std::path::PathBuf::from(std::env::var("APPDATA").expect("APPDATA not set"))
        .join("rustlight")
        .join("rustlight")
        .join("config")
        .join("config.toml");
    let config_text = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        panic!(
            "couldn't read {}: {e}\nrun the real app once and set up a device + LED layout first",
            config_path.display()
        )
    });
    let config: toml::Value = config_text.parse().expect("couldn't parse config.toml");

    let zones: Vec<Zone> = config
        .get("led")
        .and_then(|l| l.get("zones"))
        .and_then(|z| z.as_array())
        .filter(|a| !a.is_empty())
        .expect("config.toml has no [[led.zones]] - set up and save an LED layout in the app first")
        .iter()
        .map(|z| Zone {
            x: z["x"].as_float().unwrap() as f32,
            y: z["y"].as_float().unwrap() as f32,
            w: z["w"].as_float().unwrap() as f32,
            h: z["h"].as_float().unwrap() as f32,
        })
        .collect();

    let proc = &config["processing"];
    let letterbox_cutoff = proc["letterbox_cutoff"].as_float().unwrap() as f32 / 100.0;
    let threshold_amount = (proc["threshold"].as_float().unwrap() as f32 - 50.0) / 50.0;
    let smoothing_amount = proc["smoothing"].as_float().unwrap() as f32 / 100.0;
    let color_temp_amount = (proc["color_temp"].as_float().unwrap() as f32 - 50.0) / 50.0;
    let gamma = GAMMA_RANGE.powf((proc["gamma"].as_float().unwrap() as f32 - 50.0) / 50.0);

    let senders: Vec<WledSender> = config
        .get("devices")
        .and_then(|d| d.as_array())
        .filter(|a| !a.is_empty())
        .expect("config.toml has no [[devices]] - add a WLED device in the app first")
        .iter()
        .filter_map(|d| {
            let host = d["host"].as_str().unwrap();
            let port = d["port"].as_integer().unwrap() as u16;
            let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let sender = format!("{host}:{port}")
                .parse()
                .ok()
                .and_then(|addr| WledSender::new(addr).ok());
            match sender {
                Some(s) => {
                    println!("sending real frames to device \"{name}\" at {host}:{port}");
                    Some(s.with_timeout(WLED_REALTIME_TIMEOUT_SECS))
                }
                None => {
                    eprintln!("warning: couldn't open a socket for device \"{name}\" at {host}:{port} - skipping");
                    None
                }
            }
        })
        .collect();

    println!("zones: {} (from your saved layout)", zones.len());
    if senders.is_empty() {
        eprintln!("warning: no reachable devices - this run will measure capture+process only, nothing will be sent");
    }
    println!("running for {duration_secs:.0}s - play your video/game now...");

    let mut capturer = PlatformCapturer::new().expect("failed to start capture");
    let id = DisplayId("0".to_string());

    // Warm up: first few frames after starting capture can be slow/stale.
    for _ in 0..5 {
        let _ = capturer.capture(&id);
    }

    let led_count = zones.len();
    let sampler = ZoneSampler { zones, letterbox_cutoff };

    let mut capture_times = Vec::new();
    let mut process_times = Vec::new();
    let mut send_times = Vec::new();
    let mut total_times = Vec::new();

    let run_start = Instant::now();
    let deadline = run_start + Duration::from_secs_f64(duration_secs);

    while Instant::now() < deadline {
        let loop_start = Instant::now();

        let t0 = Instant::now();
        let frame = capturer.capture(&id).expect("capture failed");
        capture_times.push(t0.elapsed());

        let t1 = Instant::now();
        let mut colors = sampler.sample(&frame, led_count);
        BlackPoint { amount: threshold_amount }.apply(&mut colors);
        let mut smoother = SpatialSmoother { amount: smoothing_amount };
        let mut colors = smoother.smooth(&colors);
        ColorTemperature { amount: color_temp_amount }.apply(&mut colors);
        GammaCorrection { gamma }.apply(&mut colors);
        process_times.push(t1.elapsed());

        let t2 = Instant::now();
        for s in &senders {
            let _ = s.send_dnrgb_frame(&colors);
        }
        send_times.push(t2.elapsed());

        total_times.push(loop_start.elapsed());
    }

    let elapsed = run_start.elapsed();
    println!(
        "\ncaptured+processed{} {} frames in {:.1}s -> {:.1} fps overall sustained\n",
        if senders.is_empty() { "" } else { "+sent" },
        total_times.len(),
        elapsed.as_secs_f64(),
        total_times.len() as f64 / elapsed.as_secs_f64()
    );

    report("capture", &capture_times);
    report("process (your real saved settings)", &process_times);
    if !senders.is_empty() {
        report(&format!("UDP send ({} device(s))", senders.len()), &send_times);
    }
    report("combined (one real loop iteration: capture -> process -> send)", &total_times);
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
