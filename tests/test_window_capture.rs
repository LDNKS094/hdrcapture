// 集成测试：按进程名截取指定窗口
//
// 修改 TARGET_PROCESS 来指定要截取的进程。
// 如果目标窗口不存在，测试会优雅地跳过。

use hdrcapture::capture::find_window;
use hdrcapture::pipeline::CapturePipeline;

// ---------------------------------------------------------------------------
// 配置：修改这里来指定目标窗口
// ---------------------------------------------------------------------------

/// 目标进程名（如 "notepad.exe"、"chrome.exe"）
const TARGET_PROCESS: &str = "notepad.exe";

/// 窗口索引（同一进程有多个窗口时，0 = 第一个）
const TARGET_INDEX: usize = 0;

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn test_capture_target_window() {
    if find_window(TARGET_PROCESS, Some(TARGET_INDEX)).is_err() {
        println!(
            "SKIPPED: no window found for \"{}\" index {}",
            TARGET_PROCESS, TARGET_INDEX
        );
        return;
    }

    let mut pipeline = CapturePipeline::window(TARGET_PROCESS, Some(TARGET_INDEX)).unwrap();
    let frame = pipeline.capture().unwrap();

    assert!(frame.width > 0 && frame.height > 0);
    assert!(
        frame.data.iter().any(|&b| b != 0),
        "Window capture is all black"
    );
    println!("Window: {}x{}", frame.width, frame.height);

    frame.save("tests/results/window_capture.png").unwrap();
}
