// Integration test: Screenshot via Pipeline API

use hdrcapture::pipeline::{CapturePipeline, CapturePolicy};

#[test]
fn test_pipeline_monitor_capture() {
    let mut pipeline =
        CapturePipeline::monitor(0, CapturePolicy::Auto).expect("Failed to create pipeline");

    let frame = pipeline.capture().expect("Failed to capture frame");

    assert!(frame.width > 0 && frame.height > 0, "Invalid dimensions");
    assert_eq!(
        frame.data.len(),
        (frame.width * frame.height * 4) as usize,
        "Data size mismatch"
    );
    assert!(frame.data.iter().any(|&b| b != 0), "Captured all black");

    println!(
        "Pipeline capture: {}x{}, {} bytes",
        frame.width,
        frame.height,
        frame.data.len()
    );
    frame
        .save("tests/results/pipeline_monitor_0.png")
        .expect("Failed to save");
}

#[test]
fn test_pipeline_consecutive_frames() {
    let mut pipeline =
        CapturePipeline::monitor(0, CapturePolicy::Auto).expect("Failed to create pipeline");

    // Consecutively capture 3 frames, verify drain strategy and buffer reuse
    for i in 0..3 {
        let frame = pipeline
            .capture()
            .unwrap_or_else(|e| panic!("Frame {} failed: {}", i, e));

        assert!(frame.data.iter().any(|&b| b != 0), "Frame {} all black", i);
        println!(
            "Frame {}: {}x{}, {} bytes",
            i,
            frame.width,
            frame.height,
            frame.data.len()
        );
    }
}
