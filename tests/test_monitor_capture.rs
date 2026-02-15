// Integration test: monitor capture + multi-format save (SDR & HDR)
//
// Tests:
// 1. Capture each available monitor by index (with timing)
// 2. Consecutive frame capture (drain strategy + buffer reuse)
// 3. SDR: save to all supported formats with timing
// 4. HDR: save to HDR-capable formats (jxr, exr) with timing
//
// Results are saved to tests/results/test_report.txt

use std::fmt::Write as FmtWrite;
use std::fs;
use std::time::Instant;

use hdrcapture::capture::find_monitor;
use hdrcapture::pipeline::{CapturePipeline, CapturePolicy};

/// Shared report buffer, written to file at the end of each test.
fn save_report(name: &str, report: &str) {
    fs::create_dir_all("tests/results").ok();
    let path = format!("tests/results/{}.txt", name);
    fs::write(&path, report).unwrap();
    println!("Report saved to {}", path);
}

fn capture_monitor(index: usize, report: &mut String) {
    let t = Instant::now();
    let mut pipeline = CapturePipeline::monitor(index, CapturePolicy::Auto).unwrap();
    let pipeline_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let frame = pipeline.capture().unwrap();
    let capture_ms = t.elapsed().as_secs_f64() * 1000.0;

    assert!(frame.width > 0 && frame.height > 0);
    assert!(
        frame.data.iter().any(|&b| b != 0),
        "Monitor {} captured all black",
        index
    );

    let t = Instant::now();
    let path = format!("tests/results/monitor_{}.png", index);
    frame.save(&path).unwrap();
    let save_ms = t.elapsed().as_secs_f64() * 1000.0;

    let line = format!(
        "Monitor {}: {}x{}, format={:?}, pipeline={:.2}ms, capture={:.2}ms, save_png={:.2}ms\n",
        index, frame.width, frame.height, frame.format, pipeline_ms, capture_ms, save_ms
    );
    print!("{}", line);
    write!(report, "{}", line).unwrap();
}

// ---------------------------------------------------------------------------
// Per-monitor capture
// ---------------------------------------------------------------------------

#[test]
fn test_capture_monitor_0() {
    let mut report = String::from("=== Monitor 0 Capture ===\n\n");
    capture_monitor(0, &mut report);
    save_report("test_monitor_0", &report);
}

#[test]
fn test_capture_monitor_1() {
    if find_monitor(1).is_err() {
        println!("SKIPPED: only one monitor detected");
        return;
    }
    let mut report = String::from("=== Monitor 1 Capture ===\n\n");
    capture_monitor(1, &mut report);
    save_report("test_monitor_1", &report);
}

#[test]
fn test_capture_monitor_2() {
    if find_monitor(2).is_err() {
        println!("SKIPPED: only two monitors detected");
        return;
    }
    let mut report = String::from("=== Monitor 2 Capture ===\n\n");
    capture_monitor(2, &mut report);
    save_report("test_monitor_2", &report);
}

// ---------------------------------------------------------------------------
// Consecutive frames (drain strategy + buffer reuse)
// ---------------------------------------------------------------------------

#[test]
fn test_consecutive_frames() {
    let mut report = String::from("=== Consecutive Frames ===\n\n");

    let mut pipeline =
        CapturePipeline::monitor(0, CapturePolicy::Auto).expect("Failed to create pipeline");

    for i in 0..3 {
        let t = Instant::now();
        let frame = pipeline
            .capture()
            .unwrap_or_else(|e| panic!("Frame {} failed: {}", i, e));
        let ms = t.elapsed().as_secs_f64() * 1000.0;

        assert!(frame.data.iter().any(|&b| b != 0), "Frame {} all black", i);

        let line = format!(
            "Frame {}: {}x{}, {} bytes, {:.2}ms\n",
            i,
            frame.width,
            frame.height,
            frame.data.len(),
            ms
        );
        print!("{}", line);
        write!(report, "{}", line).unwrap();
    }

    save_report("test_consecutive_frames", &report);
}

// ---------------------------------------------------------------------------
// SDR: multi-format save with timing
// ---------------------------------------------------------------------------

#[test]
fn test_save_sdr_formats() {
    let mut report = String::from("=== SDR Format Save Benchmark ===\n\n");

    let mut pipeline =
        CapturePipeline::monitor(0, CapturePolicy::Sdr).expect("Failed to create SDR pipeline");
    let frame = pipeline.capture().expect("Failed to capture SDR frame");

    let header = format!(
        "Resolution: {}x{}, format: {:?}\n\n",
        frame.width, frame.height, frame.format
    );
    print!("{}", header);
    write!(report, "{}", header).unwrap();

    let col_header = format!("{:<8} {:>10} {:>12}\n", "format", "time(ms)", "size(bytes)");
    let separator = format!("{}\n", "-".repeat(34));
    print!("{}{}", col_header, separator);
    write!(report, "{}{}", col_header, separator).unwrap();

    let extensions = ["png", "bmp", "jpg", "tiff", "jxr", "exr"];

    for ext in &extensions {
        let path = format!("tests/results/sdr_test.{}", ext);

        let t = Instant::now();
        frame.save(&path).unwrap_or_else(|e| {
            panic!("Failed to save as .{}: {}", ext, e);
        });
        let ms = t.elapsed().as_secs_f64() * 1000.0;

        let meta = fs::metadata(&path).unwrap();
        assert!(meta.len() > 0, ".{} file is empty", ext);

        let line = format!("{:<8} {:>10.2} {:>12}\n", ext, ms, meta.len());
        print!("{}", line);
        write!(report, "{}", line).unwrap();
    }

    save_report("test_sdr_formats", &report);
}

// ---------------------------------------------------------------------------
// HDR: multi-format save with timing (skip if monitor not HDR)
// ---------------------------------------------------------------------------

#[test]
fn test_save_hdr_formats() {
    // Try creating HDR pipeline; skip if monitor doesn't support HDR
    let mut pipeline = match CapturePipeline::monitor(0, CapturePolicy::Hdr) {
        Ok(p) => p,
        Err(e) => {
            println!("SKIPPED: HDR not available on monitor 0: {}", e);
            return;
        }
    };

    if !pipeline.is_hdr() {
        println!("SKIPPED: monitor 0 is not in HDR mode");
        return;
    }

    let frame = pipeline.capture().expect("Failed to capture HDR frame");

    let mut report = String::from("=== HDR Format Save Benchmark ===\n\n");

    let header = format!(
        "Resolution: {}x{}, format: {:?}\n\n",
        frame.width, frame.height, frame.format
    );
    print!("{}", header);
    write!(report, "{}", header).unwrap();

    let col_header = format!("{:<8} {:>10} {:>12}\n", "format", "time(ms)", "size(bytes)");
    let separator = format!("{}\n", "-".repeat(34));
    print!("{}{}", col_header, separator);
    write!(report, "{}{}", col_header, separator).unwrap();

    // HDR-capable formats
    let hdr_extensions = ["jxr", "exr"];

    for ext in &hdr_extensions {
        let path = format!("tests/results/hdr_test.{}", ext);

        let t = Instant::now();
        frame.save(&path).unwrap_or_else(|e| {
            panic!("Failed to save HDR as .{}: {}", ext, e);
        });
        let ms = t.elapsed().as_secs_f64() * 1000.0;

        let meta = fs::metadata(&path).unwrap();
        assert!(meta.len() > 0, ".{} file is empty", ext);

        let line = format!("{:<8} {:>10.2} {:>12}\n", ext, ms, meta.len());
        print!("{}", line);
        write!(report, "{}", line).unwrap();
    }

    // Verify SDR-only formats correctly reject HDR data
    let sdr_extensions = ["png", "bmp", "jpg", "tiff"];
    writeln!(report).unwrap();
    println!();

    let reject_header = "SDR format rejection (expected errors):\n";
    print!("{}", reject_header);
    write!(report, "{}", reject_header).unwrap();

    for ext in &sdr_extensions {
        let path = format!("tests/results/hdr_reject_test.{}", ext);
        let result = frame.save(&path);
        assert!(result.is_err(), ".{} should reject HDR data", ext);

        let line = format!("  .{}: correctly rejected ({})\n", ext, result.unwrap_err());
        print!("{}", line);
        write!(report, "{}", line).unwrap();
    }

    save_report("test_hdr_formats", &report);
}
