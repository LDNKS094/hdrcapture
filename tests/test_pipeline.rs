// 集成测试：通过 Pipeline API 截图

use hdrcapture::pipeline::CapturePipeline;
use image::{ImageBuffer, Rgba};

fn save_bgra8_png(data: &[u8], width: u32, height: u32, path: &str) {
    let mut img = ImageBuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let (b, g, r, a) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
            img.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    img.save(path)
        .unwrap_or_else(|e| panic!("Failed to save {}: {}", path, e));
    println!("Saved: {} ({}x{})", path, width, height);
}

#[test]
fn test_pipeline_monitor_capture() {
    let mut pipeline = CapturePipeline::monitor(0).expect("Failed to create pipeline");

    let frame = pipeline.capture_frame().expect("Failed to capture frame");

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
    save_bgra8_png(
        frame.data,
        frame.width,
        frame.height,
        "tests/results/pipeline_monitor_0.png",
    );
}

#[test]
fn test_pipeline_consecutive_frames() {
    let mut pipeline = CapturePipeline::monitor(0).expect("Failed to create pipeline");

    // 连续捕获 3 帧，验证排空策略和 buffer 复用
    for i in 0..3 {
        let frame = pipeline
            .capture_frame()
            .unwrap_or_else(|e| panic!("Frame {} failed: {}", i, e));

        assert!(frame.data.iter().any(|&b| b != 0), "Frame {} all black", i);
        println!(
            "Frame {}: {}x{}, {} bytes",
            i,
            frame.width,
            frame.height,
            frame.data.len()
        );

        // 短暂等待，让 DWM 推新帧
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
