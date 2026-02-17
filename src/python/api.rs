use pyo3::prelude::*;

use super::capture::Capture;
use super::frame::CapturedFrame;

/// One-liner screenshot: capture monitor or window
///
/// Internally creates and destroys pipeline, cold start ~79ms.
/// For multiple screenshots, use capture class to reuse the pipeline.
///
/// Args:
///     monitor: Monitor index, defaults to 0
///     window: Process name for window capture (e.g., "notepad.exe")
///     window_index: Window index for processes with the same name, defaults to 0
///     mode: Capture mode — "auto", "hdr", or "sdr"
///     headless: Crop title bar and borders for window capture, defaults to true
///
/// Returns:
///     CapturedFrame: Frame container, can save() or convert to numpy
#[pyfunction]
#[pyo3(signature = (monitor=0, window=None, window_index=None, mode="auto", headless=true))]
pub(crate) fn screenshot(
    py: Python<'_>,
    monitor: usize,
    window: Option<&str>,
    window_index: Option<usize>,
    mode: &str,
    headless: bool,
) -> PyResult<CapturedFrame> {
    // Reuse the exact same capture workflow as `capture` class methods:
    // create -> capture one frame -> close.
    let mut cap = match window {
        Some(process_name) => Capture::window(
            py,
            Some(process_name.to_string()),
            None,
            None,
            window_index,
            mode,
            headless,
        )?,
        None => Capture::monitor(py, monitor, mode)?,
    };

    let result = cap.capture(py);
    cap.close(py);
    result
}
