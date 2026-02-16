// PyO3 Python binding layer
//
// Two PyClasses:
// - CapturedFrame: frame container, holds pixel data, provides save() and numpy conversion
// - Capture: reusable pipeline, delegates to a dedicated worker thread via channels
//
// Worker thread architecture:
// - All D3D11/COM/WGC resources live on a single worker thread (thread-affine)
// - Python-facing Capture holds only channel endpoints (Send + Sync)
// - This eliminates the unsendable panic: Capture can be freely shared across Python threads,
//   passed to atexit handlers, or dropped from any thread without triggering PyO3 assertions.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use self::capture::Capture;
use self::frame::CapturedFrame;
use crate::pipeline;

mod api;
mod capture;
mod frame;
mod helpers;
mod worker;

// ---------------------------------------------------------------------------
// Helper: parse mode, warn mismatch
// ---------------------------------------------------------------------------

pub(super) fn parse_mode(mode: &str) -> PyResult<pipeline::CapturePolicy> {
    pipeline::CapturePolicy::from_mode(mode).ok_or_else(|| {
        PyRuntimeError::new_err(format!(
            "invalid mode '{}': expected 'auto', 'hdr', or 'sdr'",
            mode
        ))
    })
}

pub(super) fn warn_mode_mismatch(
    py: Python<'_>,
    policy: pipeline::CapturePolicy,
    is_hdr: bool,
) -> PyResult<()> {
    let msg = match (policy, is_hdr) {
        (pipeline::CapturePolicy::Hdr, false) => Some(
            "mode='hdr' requested but the target monitor is SDR; \
             capture will proceed but output will not contain real HDR data",
        ),
        (pipeline::CapturePolicy::Sdr, true) => Some(
            "mode='sdr' requested but the target monitor is HDR; \
             HDR content will be clipped to SDR range without tone-mapping",
        ),
        _ => None,
    };

    if let Some(msg) = msg {
        let warnings = py.import("warnings")?;
        warnings.call_method1("warn", (msg,))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

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
fn screenshot(
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
        Some(process_name) => Capture::window(py, process_name, window_index, mode, headless)?,
        None => Capture::monitor(py, monitor, mode)?,
    };

    let result = cap.capture(py);
    cap.close(py);
    result
}

/// HDR-aware screen capture library for Windows
#[pymodule]
fn hdrcapture(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CapturedFrame>()?;
    m.add_class::<Capture>()?;
    m.add_function(wrap_pyfunction!(screenshot, m)?)?;
    Ok(())
}
