// Diagnostic tool: measure streaming per-phase timing
//
// Directly calls low-level components, bypassing CapturePipeline wrapper,
// precisely measuring drain / wait / read phase timings.
//
// Usage: cargo run --release --example diagnose_streaming

use std::fmt::Write as FmtWrite;
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use hdrcapture::capture::wgc::WGCCapture;
use hdrcapture::capture::{enable_dpi_awareness, find_monitor, init_capture, CapturePolicy};
use hdrcapture::d3d11::texture::TextureReader;
use hdrcapture::d3d11::{create_d3d11_device, D3D11Context};

const WARMUP_FRAMES: usize = 10;
const TEST_FRAMES: usize = 100;
const FRAME_TIMEOUT: Duration = Duration::from_secs(1);

struct FrameTiming {
    drain_ms: f64,
    drain_count: usize,
    wait_ms: f64,
    read_ms: f64,
    total_ms: f64,
}

fn wait_frame(
    capture: &WGCCapture,
    deadline: Instant,
) -> windows::Graphics::Capture::Direct3D11CaptureFrame {
    loop {
        if let Ok(f) = capture.try_get_next_frame() {
            return f;
        }
        if Instant::now() >= deadline {
            panic!("Timeout waiting for frame");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn capture_one_frame_timed(
    capture: &WGCCapture,
    reader: &mut TextureReader,
    fresh: bool,
) -> FrameTiming {
    let t_total = Instant::now();
    let deadline = Instant::now() + FRAME_TIMEOUT;

    // Phase 1: drain
    let t_drain = Instant::now();
    let mut drain_count = 0usize;
    let mut latest = None;
    while let Ok(f) = capture.try_get_next_frame() {
        drain_count += 1;
        if !fresh {
            latest = Some(f);
        }
    }
    let drain_ms = t_drain.elapsed().as_secs_f64() * 1000.0;

    // Phase 2: wait
    let t_wait = Instant::now();
    let frame = match latest {
        Some(f) => f,
        None => wait_frame(capture, deadline),
    };
    let wait_ms = t_wait.elapsed().as_secs_f64() * 1000.0;

    // Phase 3: read (frame_to_texture + read_texture)
    let t_read = Instant::now();
    let texture = WGCCapture::frame_to_texture(&frame).unwrap();
    let _data = reader.read_texture(&texture).unwrap();
    let read_ms = t_read.elapsed().as_secs_f64() * 1000.0;

    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    FrameTiming {
        drain_ms,
        drain_count,
        wait_ms,
        read_ms,
        total_ms,
    }
}

fn percentile(sorted: &[f64], p: usize) -> f64 {
    sorted[sorted.len() * p / 100]
}

fn run_diagnosis(d3d_ctx: &D3D11Context, fresh: bool) -> String {
    let mode = if fresh { "fresh" } else { "drain" };
    let hmonitor = find_monitor(0).unwrap();
    let capture = init_capture(
        d3d_ctx,
        hdrcapture::capture::wgc::CaptureTarget::Monitor(hmonitor),
        CapturePolicy::Auto,
    )
    .unwrap();
    capture.start().unwrap();

    let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

    // First frame
    let _ = wait_frame(&capture, Instant::now() + FRAME_TIMEOUT);

    // Warm-up
    for _ in 0..WARMUP_FRAMES {
        capture_one_frame_timed(&capture, &mut reader, fresh);
    }

    // Test
    let mut timings: Vec<FrameTiming> = Vec::with_capacity(TEST_FRAMES);
    for _ in 0..TEST_FRAMES {
        timings.push(capture_one_frame_timed(&capture, &mut reader, fresh));
    }

    // Statistics
    let mut drain_vals: Vec<f64> = timings.iter().map(|t| t.drain_ms).collect();
    let mut wait_vals: Vec<f64> = timings.iter().map(|t| t.wait_ms).collect();
    let mut read_vals: Vec<f64> = timings.iter().map(|t| t.read_ms).collect();
    let mut total_vals: Vec<f64> = timings.iter().map(|t| t.total_ms).collect();
    let drain_counts: Vec<usize> = timings.iter().map(|t| t.drain_count).collect();

    drain_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    wait_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    read_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    total_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let total_drained: usize = drain_counts.iter().sum();
    let max_drained = drain_counts.iter().max().unwrap();

    let mut s = String::new();
    writeln!(s, "[streaming {} mode] {} frames:", mode, TEST_FRAMES).unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "  drain:  p50={:.2}ms  p95={:.2}ms  max={:.2}ms  (total drained: {}, max/frame: {})",
        percentile(&drain_vals, 50),
        percentile(&drain_vals, 95),
        drain_vals.last().unwrap(),
        total_drained,
        max_drained,
    )
    .unwrap();
    writeln!(
        s,
        "  wait:   p50={:.2}ms  p95={:.2}ms  max={:.2}ms",
        percentile(&wait_vals, 50),
        percentile(&wait_vals, 95),
        wait_vals.last().unwrap(),
    )
    .unwrap();
    writeln!(
        s,
        "  read:   p50={:.2}ms  p95={:.2}ms  max={:.2}ms",
        percentile(&read_vals, 50),
        percentile(&read_vals, 95),
        read_vals.last().unwrap(),
    )
    .unwrap();
    writeln!(
        s,
        "  TOTAL:  p50={:.2}ms  p95={:.2}ms  max={:.2}ms",
        percentile(&total_vals, 50),
        percentile(&total_vals, 95),
        total_vals.last().unwrap(),
    )
    .unwrap();
    writeln!(s).unwrap();

    // Output first 10 frame details for pattern observation
    writeln!(s, "  first 10 frames detail:").unwrap();
    for (i, t) in timings.iter().take(10).enumerate() {
        writeln!(
            s,
            "    #{:>3}: drain={:.2}ms({}) wait={:.2}ms read={:.2}ms total={:.2}ms",
            i, t.drain_ms, t.drain_count, t.wait_ms, t.read_ms, t.total_ms,
        )
        .unwrap();
    }
    writeln!(s).unwrap();

    // Output slowest 5 frames
    let mut indexed: Vec<(usize, &FrameTiming)> = timings.iter().enumerate().collect();
    indexed.sort_by(|a, b| b.1.total_ms.partial_cmp(&a.1.total_ms).unwrap());
    writeln!(s, "  slowest 5 frames:").unwrap();
    for (i, t) in indexed.iter().take(5) {
        writeln!(
            s,
            "    #{:>3}: drain={:.2}ms({}) wait={:.2}ms read={:.2}ms total={:.2}ms",
            i, t.drain_ms, t.drain_count, t.wait_ms, t.read_ms, t.total_ms,
        )
        .unwrap();
    }
    writeln!(s).unwrap();

    s
}

fn main() {
    enable_dpi_awareness();
    let d3d_ctx = create_d3d11_device().unwrap();

    let mut report = String::from("=== Streaming Diagnosis ===\n\n");

    let s = run_diagnosis(&d3d_ctx, true);
    print!("{s}");
    write!(report, "{s}").unwrap();

    let s = run_diagnosis(&d3d_ctx, false);
    print!("{s}");
    write!(report, "{s}").unwrap();

    fs::create_dir_all("tests/results").ok();
    fs::write("tests/results/diagnose_streaming.txt", &report).unwrap();
    println!("Report saved to tests/results/diagnose_streaming.txt");
}
