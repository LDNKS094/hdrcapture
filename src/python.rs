// PyO3 Python binding layer
//
// Two PyClasses:
// - CapturedFrame: frame container, holds pixel data, provides save() and numpy conversion
// - Capture: reusable pipeline, wraps CapturePipeline
//
// Pipeline remains pure Rust, no dependency on pyo3/numpy.
// This module handles cross-language bridging and error mapping.

use std::panic::AssertUnwindSafe;

use numpy::ndarray::Array3;
use numpy::{IntoPyArray, PyArray3};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::pipeline;

/// Release the GIL, execute a pure-Rust closure, then re-acquire the GIL.
///
/// Equivalent to `py.detach()` but bypasses the `Ungil` (Send) bound.
/// `CapturePipeline` holds Win32 HANDLE and COM pointers (not Send),
/// but `Capture` is marked `unsendable`, guaranteeing single-thread usage.
///
/// Panics inside the closure are caught and converted to Python RuntimeError,
/// ensuring the GIL is always restored.
fn detach_gil<T>(py: Python<'_>, f: impl FnOnce() -> T) -> PyResult<T> {
    // SAFETY: We release the GIL and re-acquire it on the same thread.
    // The closure executes synchronously; no non-Send types cross threads.
    let save = unsafe { pyo3::ffi::PyEval_SaveThread() };
    let result = std::panic::catch_unwind(AssertUnwindSafe(f));
    unsafe { pyo3::ffi::PyEval_RestoreThread(save) };
    let _ = py;
    result.map_err(|panic| {
        let msg = if let Some(s) = panic.downcast_ref::<&str>() {
            format!("internal error: {}", s)
        } else if let Some(s) = panic.downcast_ref::<String>() {
            format!("internal error: {}", s)
        } else {
            "internal error: unknown panic".to_string()
        };
        PyRuntimeError::new_err(msg)
    })
}

// ---------------------------------------------------------------------------
// CapturedFrame — frame container
// ---------------------------------------------------------------------------

/// Single frame capture result
///
/// Holds BGRA8 pixel data, provides save and numpy conversion functionality.
/// `save()` writes directly to disk on the Rust side, bypassing Python, for optimal performance.
#[pyclass]
struct CapturedFrame {
    inner: pipeline::CapturedFrame,
}

#[pymethods]
impl CapturedFrame {
    /// Frame width (pixels)
    #[getter]
    fn width(&self) -> u32 {
        self.inner.width
    }

    /// Frame height (pixels)
    #[getter]
    fn height(&self) -> u32 {
        self.inner.height
    }

    /// Frame timestamp (seconds), relative to system boot time
    #[getter]
    fn timestamp(&self) -> f64 {
        self.inner.timestamp
    }

    /// Save as image file (format determined by extension, e.g., .png, .bmp, .jpg)
    ///
    /// Rust side directly performs BGRA→RGBA conversion and writes to disk, bypassing Python memory.
    /// Releases GIL during encoding, doesn't block other Python threads.
    fn save(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        let inner = &self.inner;
        let path = path.to_string();
        detach_gil(py, || inner.save(&path))?.map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// Convert to ndarray
    ///
    /// Returns:
    ///     numpy.ndarray: shape (H, W, 4), dtype uint8, BGRA channel order
    fn ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        self.to_ndarray(py)
    }

    /// numpy __array__ protocol, enables np.array(frame) to work automatically
    #[pyo3(signature = (dtype=None, copy=None))]
    fn __array__<'py>(
        &self,
        py: Python<'py>,
        dtype: Option<Bound<'py, PyAny>>,
        copy: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let _ = (dtype, copy);
        self.to_ndarray(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "CapturedFrame({}x{}, timestamp={:.3}s)",
            self.inner.width, self.inner.height, self.inner.timestamp
        )
    }
}

impl CapturedFrame {
    /// Internal shared numpy conversion logic
    fn to_ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let array = Array3::from_shape_vec((h, w, 4), self.inner.data.clone())
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(array.into_pyarray(py))
    }
}

// ---------------------------------------------------------------------------
// Capture — reusable pipeline
// ---------------------------------------------------------------------------

/// Screen/window capture pipeline
///
/// Construct via class methods:
///   cap = Capture.monitor(0)
///   cap = Capture.window("notepad.exe")
///
/// Supports context manager:
///   with Capture.monitor(0) as cap:
///       frame = cap.capture()
#[pyclass(unsendable)]
struct Capture {
    pipeline: Option<pipeline::CapturePipeline>,
}

impl Capture {
    /// Get pipeline reference, errors after close()
    fn get_pipeline(&mut self) -> PyResult<&mut pipeline::CapturePipeline> {
        self.pipeline
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("Capture is closed"))
    }
}

#[pymethods]
impl Capture {
    /// Create capture pipeline by monitor index
    ///
    /// Args:
    ///     index: Monitor index, defaults to 0
    ///     force_sdr: Force SDR-compatible capture path
    #[staticmethod]
    #[pyo3(signature = (index=0, force_sdr=false))]
    fn monitor(index: usize, force_sdr: bool) -> PyResult<Self> {
        let policy = pipeline::CapturePolicy::from(force_sdr);
        let pipeline = pipeline::CapturePipeline::monitor(index, policy)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self {
            pipeline: Some(pipeline),
        })
    }

    /// Create window capture pipeline by process name
    ///
    /// Args:
    ///     process_name: Process name (e.g., "notepad.exe")
    ///     index: Window index for processes with the same name, defaults to 0
    ///     force_sdr: Force SDR-compatible capture path
    #[staticmethod]
    #[pyo3(signature = (process_name, index=None, force_sdr=false))]
    fn window(process_name: &str, index: Option<usize>, force_sdr: bool) -> PyResult<Self> {
        let policy = pipeline::CapturePolicy::from(force_sdr);
        let pipeline = pipeline::CapturePipeline::window(process_name, index, policy)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self {
            pipeline: Some(pipeline),
        })
    }

    /// Screenshot mode: capture a fresh frame
    ///
    /// Drain backlog and wait for DWM to push new frame, guarantees returned frame is generated after the call.
    /// Releases GIL during wait and readback, doesn't block other Python threads.
    fn capture(&mut self, py: Python<'_>) -> PyResult<CapturedFrame> {
        let p = self.get_pipeline()?;
        let frame =
            detach_gil(py, || p.capture())?.map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(CapturedFrame { inner: frame })
    }

    /// Continuous capture mode: grab latest available frame
    ///
    /// Drain backlog and keep last frame, wait for new frame when pool is empty. Lower latency.
    /// Releases GIL during wait and readback, doesn't block other Python threads.
    fn grab(&mut self, py: Python<'_>) -> PyResult<CapturedFrame> {
        let p = self.get_pipeline()?;
        let frame =
            detach_gil(py, || p.grab())?.map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(CapturedFrame { inner: frame })
    }

    /// Release capture resources
    fn close(&mut self) {
        self.pipeline = None;
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        self.close();
        false // Don't swallow exceptions
    }

    fn __repr__(&self) -> String {
        if self.pipeline.is_some() {
            "Capture(active)".to_string()
        } else {
            "Capture(closed)".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

/// One-liner screenshot: capture monitor or window
///
/// Internally creates and destroys pipeline, cold start ~79ms.
/// For multiple screenshots, use Capture class to reuse the pipeline.
///
/// Args:
///     monitor: Monitor index, defaults to 0
///     window: Process name for window capture (e.g., "notepad.exe")
///     window_index: Window index for processes with the same name, defaults to 0
///     force_sdr: Force SDR-compatible capture path
///
/// Returns:
///     CapturedFrame: Frame container, can save() or convert to numpy
#[pyfunction]
#[pyo3(signature = (monitor=0, window=None, window_index=None, force_sdr=false))]
fn screenshot(
    py: Python<'_>,
    monitor: usize,
    window: Option<&str>,
    window_index: Option<usize>,
    force_sdr: bool,
) -> PyResult<CapturedFrame> {
    let policy = pipeline::CapturePolicy::from(force_sdr);
    let frame = detach_gil(py, || match window {
        Some(process_name) => pipeline::screenshot(
            pipeline::CaptureSource::Window {
                process_name,
                window_index,
            },
            policy,
        ),
        None => pipeline::screenshot(pipeline::CaptureSource::Monitor(monitor), policy),
    })?
    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(CapturedFrame { inner: frame })
}

/// HDR-aware screen capture library for Windows
#[pymodule]
fn hdrcapture(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CapturedFrame>()?;
    m.add_class::<Capture>()?;
    m.add_function(wrap_pyfunction!(screenshot, m)?)?;
    Ok(())
}
