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
///     pid: Process id for window capture
///     hwnd: Window handle for window capture
///     index: Ranked window index within candidate windows
///     mode: Capture mode — "auto", "hdr", or "sdr"
///     headless: Crop title bar and borders for window capture, defaults to true
///
/// Returns:
///     CapturedFrame: Frame container, can save() or convert to numpy
#[pyfunction]
#[pyo3(signature = (monitor=0, window=None, pid=None, hwnd=None, index=None, mode="auto", headless=true))]
pub(crate) fn screenshot(
    py: Python<'_>,
    monitor: usize,
    window: Option<&str>,
    pid: Option<u32>,
    hwnd: Option<isize>,
    index: Option<usize>,
    mode: &str,
    headless: bool,
) -> PyResult<CapturedFrame> {
    // Reuse the exact same capture workflow as `capture` class methods:
    // create -> capture one frame -> close.
    let mut cap = if window.is_some() || pid.is_some() || hwnd.is_some() {
        Capture::window(
            py,
            window.map(str::to_string),
            pid,
            hwnd,
            index,
            mode,
            headless,
        )?
    } else {
        Capture::monitor(py, monitor, mode)?
    };

    let result = cap.capture(py);
    cap.close(py);
    result
}
