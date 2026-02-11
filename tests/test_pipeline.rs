// 集成测试：通过 Pipeline API 截图

use hdrcapture::pipeline::CapturePipeline;

#[test]
fn test_pipeline_monitor_capture() {
    let mut pipeline = CapturePipeline::monitor(0).expect("Failed to create pipeline");

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
    let mut pipeline = CapturePipeline::monitor(0).expect("Failed to create pipeline");

    // 连续捕获 3 帧，验证排空策略和 buffer 复用
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
