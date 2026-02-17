// Integration test: Capture specified window by process name
//
// Modify TARGET_PROCESS to specify the process to capture.
// If target window doesn't exist, test will gracefully skip.

use hdrcapture::capture::{find_window, WindowSelector};
use hdrcapture::pipeline::{CapturePipeline, CapturePolicy};

// ---------------------------------------------------------------------------
// Configuration: modify here to specify target window
// ---------------------------------------------------------------------------

/// Target process name (e.g., "notepad.exe", "chrome.exe")
const TARGET_PROCESS: &str = "notepad.exe";

/// Window index (when process has multiple windows, 0 = first)
const TARGET_INDEX: usize = 0;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_capture_target_window() {
    if find_window(
        WindowSelector::Process(TARGET_PROCESS.to_string()),
        Some(TARGET_INDEX),
    )
    .is_err()
    {
        println!(
            "SKIPPED: no window found for \"{}\" index {}",
            TARGET_PROCESS, TARGET_INDEX
        );
        return;
    }

    let mut pipeline = CapturePipeline::window(
        Some(TARGET_PROCESS),
        None,
        None,
        Some(TARGET_INDEX),
        CapturePolicy::Auto,
        true,
    )
    .unwrap();
    let frame = pipeline.capture().unwrap();

    assert!(frame.width > 0 && frame.height > 0);
    assert!(
        frame.data.iter().any(|&b| b != 0),
        "Window capture is all black"
    );
    println!("Window: {}x{}", frame.width, frame.height);

    frame.save("tests/results/window_capture.png").unwrap();
}
