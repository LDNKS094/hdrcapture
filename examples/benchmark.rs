// Performance benchmark: measure capture() / grab() latency
//
// Test scenarios:
// 1. Single-shot: new pipeline → capture(), measure end-to-end latency
// 2. Continuous capture: continuous frames after pipeline warm-up, measure steady-state latency
//
// Execute on both monitor and window, results saved to tests/results/
//
// Usage: cargo run --release --example benchmark

use std::fmt::Write as FmtWrite;
use std::fs;
use std::time::Instant;

use hdrcapture::pipeline::{CapturePipeline, CapturePolicy};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Single-shot test rounds
const SINGLE_SHOT_ROUNDS: usize = 20;

/// Continuous capture warm-up frames
const WARMUP_FRAMES: usize = 10;

/// Continuous capture test frames
const STREAMING_FRAMES: usize = 100;

/// Target window process name (skipped if not exists)
const WINDOW_PROCESS: &str = "notepad.exe";

// ---------------------------------------------------------------------------
// Statistics utilities
// ---------------------------------------------------------------------------

struct Stats {
    avg_ms: f64,
    min_ms: f64,
    max_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

fn compute_stats(durations: &mut Vec<f64>) -> Stats {
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = durations.len();
    let total: f64 = durations.iter().sum();
    Stats {
        avg_ms: total / n as f64,
        min_ms: durations[0],
        max_ms: durations[n - 1],
        p50_ms: durations[n / 2],
        p95_ms: durations[n * 95 / 100],
        p99_ms: durations[n * 99 / 100],
    }
}

fn format_stats(label: &str, resolution: &str, count: usize, stats: &Stats) -> String {
    let mut s = String::new();
    writeln!(s, "[{}] {}, {} rounds:", label, resolution, count).unwrap();
    writeln!(s, "  avg: {:.2} ms", stats.avg_ms).unwrap();
    writeln!(
        s,
        "  min: {:.2} ms | max: {:.2} ms",
        stats.min_ms, stats.max_ms
    )
    .unwrap();
    writeln!(
        s,
        "  p50: {:.2} ms | p95: {:.2} ms | p99: {:.2} ms",
        stats.p50_ms, stats.p95_ms, stats.p99_ms
    )
    .unwrap();
    s
}

fn format_pool_stats(stats: hdrcapture::memory::PoolStats) -> String {
    let mut s = String::new();
    writeln!(s, "  pool total frames: {}", stats.total_frames).unwrap();
    writeln!(s, "  pool free frames: {}", stats.free_frames).unwrap();
    writeln!(s, "  pool expand count: {}", stats.expand_count).unwrap();
    writeln!(s, "  pool shrink count: {}", stats.shrink_count).unwrap();
    writeln!(s, "  pool reuse rate: {:.2}%", stats.reuse_rate() * 100.0).unwrap();
    s
}

// ---------------------------------------------------------------------------
// Test scenarios
// ---------------------------------------------------------------------------

enum Target {
    Monitor(usize),
    Window(&'static str),
}

fn create_pipeline(target: &Target) -> Option<CapturePipeline> {
    match target {
        Target::Monitor(idx) => Some(
            CapturePipeline::monitor(*idx, CapturePolicy::Auto)
                .expect("Failed to create monitor pipeline"),
        ),
        Target::Window(name) => {
            CapturePipeline::window(Some(name), None, None, Some(0), CapturePolicy::Auto, true).ok()
        }
    }
}

fn target_label(target: &Target) -> String {
    match target {
        Target::Monitor(idx) => format!("monitor_{}", idx),
        Target::Window(name) => format!("window_{}", name.replace('.', "_")),
    }
}

/// Single-shot: new pipeline each time → capture(), measure end-to-end latency
fn bench_single_shot(target: &Target, report: &mut String) {
    let label = target_label(target);
    let mut durations = Vec::with_capacity(SINGLE_SHOT_ROUNDS);
    let mut resolution = String::new();

    for i in 0..SINGLE_SHOT_ROUNDS {
        let t = Instant::now();
        let mut pipeline = match create_pipeline(target) {
            Some(p) => p,
            None => return,
        };
        let frame = pipeline.capture().unwrap();
        let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
        durations.push(elapsed_ms);

        if i == 0 {
            resolution = format!("{}x{}", frame.width, frame.height);
            // Save a screenshot to verify image
            fs::create_dir_all("tests/results").ok();
            frame
                .save(format!("tests/results/bench_{}_single.png", label))
                .unwrap();
        }
    }

    let stats = compute_stats(&mut durations);
    let s = format_stats(
        &format!("{} single-shot", label),
        &resolution,
        SINGLE_SHOT_ROUNDS,
        &stats,
    );
    print!("{s}");
    write!(report, "{s}").unwrap();
}

/// Continuous capture: continuous frames after pipeline warm-up
fn bench_streaming(target: &Target, use_capture: bool, report: &mut String) {
    let label = target_label(target);
    let mode = if use_capture { "capture" } else { "grab" };

    let mut pipeline = match create_pipeline(target) {
        Some(p) => p,
        None => return,
    };

    // Warm-up
    for _ in 0..WARMUP_FRAMES {
        if use_capture {
            pipeline.capture().unwrap();
        } else {
            pipeline.grab().unwrap();
        }
    }

    let mut durations = Vec::with_capacity(STREAMING_FRAMES);
    let mut resolution = String::new();

    for i in 0..STREAMING_FRAMES {
        let t = Instant::now();
        let frame = if use_capture {
            pipeline.capture().unwrap()
        } else {
            pipeline.grab().unwrap()
        };
        let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
        durations.push(elapsed_ms);

        if i == 0 {
            resolution = format!("{}x{}", frame.width, frame.height);
            fs::create_dir_all("tests/results").ok();
            frame
                .save(format!("tests/results/bench_{}_{}.png", label, mode))
                .unwrap();
        }

        std::hint::black_box(&frame.data);
    }

    let stats = compute_stats(&mut durations);
    let mut s = format_stats(
        &format!("{} streaming {}", label, mode),
        &resolution,
        STREAMING_FRAMES,
        &stats,
    );
    s.push_str(&format_pool_stats(pipeline.pool_stats()));
    print!("{s}");
    write!(report, "{s}").unwrap();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let mut report = String::new();
    writeln!(report, "=== HDR_CAP Benchmark ===").unwrap();
    writeln!(report).unwrap();

    let targets = [Target::Monitor(0), Target::Window(WINDOW_PROCESS)];

    for target in &targets {
        let label = target_label(target);

        // Check if target is available
        if create_pipeline(target).is_none() {
            let msg = format!("SKIPPED: {} not available\n\n", label);
            print!("{msg}");
            write!(report, "{msg}").unwrap();
            continue;
        }

        writeln!(report, "--- {} ---", label).unwrap();
        println!("--- {} ---", label);

        bench_single_shot(target, &mut report);
        bench_streaming(target, true, &mut report);
        bench_streaming(target, false, &mut report);

        writeln!(report).unwrap();
        println!();
    }

    // Save report
    fs::create_dir_all("tests/results").ok();
    fs::write("tests/results/benchmark_report.txt", &report).expect("Failed to save report");
    println!("Report saved to tests/results/benchmark_report.txt");
}
