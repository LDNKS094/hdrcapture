// 性能基准测试：测量 capture_frame() 延迟
//
// 测试场景：
// 1. 单帧截图：新建 pipeline → capture_frame()，测端到端延迟
// 2. 连续取帧：pipeline 热启动后连续 capture_frame()，测稳态延迟
//
// 分别对显示器和窗口执行，结果保存到 tests/results/
//
// 用法：cargo run --release --example benchmark

use std::fmt::Write as FmtWrite;
use std::fs;
use std::time::Instant;

use hdrcapture::pipeline::CapturePipeline;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 单帧截图测试次数
const SINGLE_SHOT_ROUNDS: usize = 20;

/// 连续取帧预热帧数
const WARMUP_FRAMES: usize = 10;

/// 连续取帧测试帧数
const STREAMING_FRAMES: usize = 100;

/// 目标窗口进程名（不存在则跳过）
const WINDOW_PROCESS: &str = "Endfield.exe";

// ---------------------------------------------------------------------------
// 统计工具
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

// ---------------------------------------------------------------------------
// 测试场景
// ---------------------------------------------------------------------------

enum Target {
    Monitor(usize),
    Window(&'static str),
}

fn create_pipeline(target: &Target) -> Option<CapturePipeline> {
    match target {
        Target::Monitor(idx) => {
            Some(CapturePipeline::monitor(*idx).expect("Failed to create monitor pipeline"))
        }
        Target::Window(name) => CapturePipeline::window(name, Some(0)).ok(),
    }
}

fn target_label(target: &Target) -> String {
    match target {
        Target::Monitor(idx) => format!("monitor_{}", idx),
        Target::Window(name) => format!("window_{}", name.replace('.', "_")),
    }
}

/// 单帧截图：每次新建 pipeline → capture_frame()，测端到端延迟
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
        let frame = pipeline.capture_frame().unwrap();
        let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
        durations.push(elapsed_ms);

        if i == 0 {
            resolution = format!("{}x{}", frame.width, frame.height);
            // 保存一张截图验证画面
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

/// 连续取帧：pipeline 热启动后连续 capture_frame()
fn bench_streaming(target: &Target, fresh: bool, report: &mut String) {
    let label = target_label(target);
    let mode = if fresh { "fresh" } else { "drain" };

    let mut pipeline = match create_pipeline(target) {
        Some(p) => p,
        None => return,
    };
    pipeline.fresh = fresh;

    // 预热
    for _ in 0..WARMUP_FRAMES {
        pipeline.capture_frame().unwrap();
    }

    let mut durations = Vec::with_capacity(STREAMING_FRAMES);
    let mut resolution = String::new();

    for i in 0..STREAMING_FRAMES {
        let t = Instant::now();
        let frame = pipeline.capture_frame().unwrap();
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
    let s = format_stats(
        &format!("{} streaming {}", label, mode),
        &resolution,
        STREAMING_FRAMES,
        &stats,
    );
    print!("{s}");
    write!(report, "{s}").unwrap();
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() {
    let mut report = String::new();
    writeln!(report, "=== HDR_CAP Benchmark ===").unwrap();
    writeln!(report).unwrap();

    let targets = [Target::Monitor(0), Target::Window(WINDOW_PROCESS)];

    for target in &targets {
        let label = target_label(target);

        // 检查目标是否可用
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

    // 保存报告
    fs::create_dir_all("tests/results").ok();
    fs::write("tests/results/benchmark_report.txt", &report).expect("Failed to save report");
    println!("Report saved to tests/results/benchmark_report.txt");
}
