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

use pyo3::prelude::*;

use self::api::screenshot;
use self::capture::Capture;
use self::frame::CapturedFrame;

mod api;
mod capture;
mod frame;
mod helpers;
mod worker;

/// HDR-aware screen capture library for Windows
#[pymodule]
fn hdrcapture(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CapturedFrame>()?;
    m.add_class::<Capture>()?;
    m.add_function(wrap_pyfunction!(screenshot, m)?)?;
    Ok(())
}
