// Diagnostic tool: measure single-shot per-phase timing
//
// Starts from zero each round, precisely measuring:
//   1. find_monitor — target resolution
//   2. create_d3d11_device — D3D11 device creation
//   3. init_capture — WGC session creation
//   4. start — start capture
//   5. wait — wait for first frame
//   6. read — texture readback
//
// Usage: cargo run --release --example diagnose_singleshot

use std::fmt::Write as FmtWrite;
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use hdrcapture::capture::wgc::{CaptureTarget, WGCCapture};
use hdrcapture::capture::{enable_dpi_awareness, find_monitor, init_capture, CapturePolicy};
use hdrcapture::d3d11::create_d3d11_device;
use hdrcapture::d3d11::texture::TextureReader;

const ROUNDS: usize = 20;
const FRAME_TIMEOUT: Duration = Duration::from_secs(1);

struct ShotTiming {
    find_ms: f64,
    device_ms: f64,
    init_ms: f64,
    start_ms: f64,
    wait_ms: f64,
    read_ms: f64,
    total_ms: f64,
}

fn main() {
    enable_dpi_awareness();

    let mut timings: Vec<ShotTiming> = Vec::with_capacity(ROUNDS);

    for _ in 0..ROUNDS {
        let t_total = Instant::now();

        // 1. find_monitor
        let t = Instant::now();
        let hmonitor = find_monitor(0).unwrap();
        let find_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 2. create_d3d11_device
        let t = Instant::now();
        let d3d_ctx = create_d3d11_device().unwrap();
        let device_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 3. init_capture
        let t = Instant::now();
        let capture = init_capture(
            &d3d_ctx,
            CaptureTarget::Monitor(hmonitor),
            CapturePolicy::Auto,
        )
        .unwrap();
        let init_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 4. start
        let t = Instant::now();
        capture.start().unwrap();
        let start_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 5. wait for first frame
        let t = Instant::now();
        let deadline = Instant::now() + FRAME_TIMEOUT;
        let frame = loop {
            if let Ok(f) = capture.try_get_next_frame() {
                break f;
            }
            if Instant::now() >= deadline {
                panic!("Timeout waiting for first frame");
            }
            thread::sleep(Duration::from_millis(1));
        };
        let wait_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 6. read (frame_to_texture + read_texture)
        let t = Instant::now();
        let texture = WGCCapture::frame_to_texture(&frame).unwrap();
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());
        let _data = reader.read_texture(&texture).unwrap();
        let read_ms = t.elapsed().as_secs_f64() * 1000.0;

        let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

        timings.push(ShotTiming {
            find_ms,
            device_ms,
            init_ms,
            start_ms,
            wait_ms,
            read_ms,
            total_ms,
        });
    }

    // --- Statistics ---
    let mut report = String::from("=== Single-Shot Diagnosis ===\n\n");

    // Per-round details
    writeln!(report, "per-round detail ({} rounds):", ROUNDS).unwrap();
    writeln!(
        report,
        "  {:>3}  {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "#", "find", "device", "init", "start", "wait", "read", "TOTAL"
    )
    .unwrap();
    for (i, t) in timings.iter().enumerate() {
        writeln!(
            report,
            "  {:>3}  {:>7.2}  {:>7.2}  {:>7.2}  {:>7.2}  {:>7.2}  {:>7.2}  {:>7.2}",
            i, t.find_ms, t.device_ms, t.init_ms, t.start_ms, t.wait_ms, t.read_ms, t.total_ms,
        )
        .unwrap();
    }
    writeln!(report).unwrap();

    // Per-phase p50
    macro_rules! p50 {
        ($field:ident) => {{
            let mut v: Vec<f64> = timings.iter().map(|t| t.$field).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        }};
    }

    writeln!(report, "p50 summary:").unwrap();
    writeln!(report, "  find:   {:.2} ms", p50!(find_ms)).unwrap();
    writeln!(report, "  device: {:.2} ms", p50!(device_ms)).unwrap();
    writeln!(report, "  init:   {:.2} ms", p50!(init_ms)).unwrap();
    writeln!(report, "  start:  {:.2} ms", p50!(start_ms)).unwrap();
    writeln!(report, "  wait:   {:.2} ms", p50!(wait_ms)).unwrap();
    writeln!(report, "  read:   {:.2} ms", p50!(read_ms)).unwrap();
    writeln!(report, "  TOTAL:  {:.2} ms", p50!(total_ms)).unwrap();

    print!("{report}");

    fs::create_dir_all("tests/results").ok();
    fs::write("tests/results/diagnose_singleshot.txt", &report).unwrap();
    println!("\nReport saved to tests/results/diagnose_singleshot.txt");
}
