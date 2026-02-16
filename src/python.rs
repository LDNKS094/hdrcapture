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

use std::sync::{mpsc, Mutex};
use std::thread::{self, JoinHandle};

use half::f16;
use numpy::ndarray::Array3;
use numpy::{IntoPyArray, PyArrayMethods};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::color::ColorPixelFormat;
use crate::pipeline;

// ---------------------------------------------------------------------------
// Worker thread protocol
// ---------------------------------------------------------------------------

enum Command {
    Capture,
    Grab,
    IsHdr,
    Close,
}

enum Response {
    Frame(Result<pipeline::CapturedFrame, String>),
    Bool(bool),
    Closed,
}

type WorkerHandle = (
    mpsc::Sender<Command>,
    mpsc::Receiver<Response>,
    JoinHandle<()>,
);

/// Spawn a worker thread that owns a CapturePipeline and processes commands.
///
/// The worker initializes COM (MTA) before creating the pipeline, ensuring
/// D3D11/WinRT calls succeed on the dedicated thread.
/// Returns (sender, receiver, join_handle) on success, or an error string if
/// pipeline creation itself failed.
fn spawn_worker(
    init: Box<dyn FnOnce() -> anyhow::Result<pipeline::CapturePipeline> + Send>,
) -> Result<WorkerHandle, String> {
    // Channel for init result: worker sends back Ok(()) or Err(msg) once pipeline is ready.
    let (init_tx, init_rx) = mpsc::channel::<Result<(), String>>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (resp_tx, resp_rx) = mpsc::channel::<Response>();

    let handle = thread::Builder::new()
        .name("hdrcapture-worker".into())
        .spawn(move || {
            // SAFETY: CoInitializeEx initializes COM on this thread.
            // MTA is required for D3D11 + WinRT interop (CreateDirect3D11DeviceFromDXGIDevice).
            // We call CoUninitialize on thread exit via _com_guard drop.
            let _com_guard = unsafe {
                use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                if let Err(e) = CoInitializeEx(None, COINIT_MULTITHREADED).ok() {
                    let _ = init_tx.send(Err(format!("COM init failed: {e}")));
                    return;
                }
                ComGuard
            };

            let mut pipeline = match init() {
                Ok(p) => {
                    let _ = init_tx.send(Ok(()));
                    p
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e.to_string()));
                    return;
                }
            };

            // Event loop: process commands until Close or channel disconnect.
            while let Ok(cmd) = cmd_rx.recv() {
                let resp = match cmd {
                    Command::Capture => {
                        Response::Frame(pipeline.capture().map_err(|e| e.to_string()))
                    }
                    Command::Grab => Response::Frame(pipeline.grab().map_err(|e| e.to_string())),
                    Command::IsHdr => Response::Bool(pipeline.is_hdr()),
                    Command::Close => {
                        drop(pipeline);
                        let _ = resp_tx.send(Response::Closed);
                        return;
                    }
                };
                if resp_tx.send(resp).is_err() {
                    // Python side dropped the receiver; shut down.
                    return;
                }
            }
            // cmd_tx dropped (Capture dropped without close) — just exit, pipeline drops here.
        })
        .map_err(|e| format!("Failed to spawn worker thread: {e}"))?;

    // Wait for pipeline init result.
    match init_rx.recv() {
        Ok(Ok(())) => Ok((cmd_tx, resp_rx, handle)),
        Ok(Err(msg)) => {
            let _ = handle.join();
            Err(msg)
        }
        Err(_) => {
            let _ = handle.join();
            Err("Worker thread exited before initialization".into())
        }
    }
}

/// RAII guard for COM uninitialization.
struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: Paired with CoInitializeEx at thread start.
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: parse mode, warn mismatch
// ---------------------------------------------------------------------------

fn parse_mode(mode: &str) -> PyResult<pipeline::CapturePolicy> {
    pipeline::CapturePolicy::from_mode(mode).ok_or_else(|| {
        PyRuntimeError::new_err(format!(
            "invalid mode '{}': expected 'auto', 'hdr', or 'sdr'",
            mode
        ))
    })
}

fn warn_mode_mismatch(
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
// CapturedFrame — frame container (unchanged)
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

    /// Pixel format string ("bgra8" or "rgba16f")
    #[getter]
    fn format(&self) -> &'static str {
        match self.inner.format {
            ColorPixelFormat::Bgra8 => "bgra8",
            ColorPixelFormat::Rgba16f => "rgba16f",
        }
    }

    /// Save frame to file (format determined by extension).
    ///
    /// Supported formats:
    ///   - .png .bmp .jpg .tiff — standard formats (BGRA8 / SDR only)
    ///   - .jxr — JPEG XR (both BGRA8 and RGBA16F / HDR)
    ///   - .exr — OpenEXR (both BGRA8 and RGBA16F / HDR)
    ///
    /// Releases GIL during encoding, doesn't block other Python threads.
    fn save(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        let inner = &self.inner;
        let path = path.to_string();
        py.detach(|| inner.save(&path))
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// Convert to numpy array.
    ///
    /// Returns:
    ///     numpy.ndarray: shape (H, W, 4).
    ///       - bgra8: dtype uint8, BGRA channel order
    ///       - rgba16f: dtype float16, RGBA channel order
    fn ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.to_ndarray(py)
    }

    /// numpy __array__ protocol, enables np.array(frame) to work automatically
    #[pyo3(signature = (dtype=None, copy=None))]
    fn __array__<'py>(
        &self,
        py: Python<'py>,
        dtype: Option<Bound<'py, PyAny>>,
        copy: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _ = (dtype, copy);
        self.to_ndarray(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "CapturedFrame({}x{}, format={}, timestamp={:.3}s)",
            self.inner.width,
            self.inner.height,
            self.format(),
            self.inner.timestamp
        )
    }
}

impl CapturedFrame {
    /// Internal shared numpy conversion logic.
    ///
    /// - bgra8 → (H, W, 4) uint8
    /// - rgba16f → (H, W, 4) float16
    fn to_ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let data = self.inner.data.as_slice();

        match self.inner.format {
            ColorPixelFormat::Bgra8 => {
                let array = Array3::from_shape_vec((h, w, 4), data.to_vec())
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
                let pyarray = array.into_pyarray(py);
                pyarray
                    .try_readwrite()
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
                    .make_nonwriteable();
                Ok(pyarray.into_any())
            }
            ColorPixelFormat::Rgba16f => {
                // SAFETY: f16 is #[repr(transparent)] over u16 (2 bytes).
                // data length is guaranteed to be h * w * 8 by the capture pipeline.
                let f16_slice: &[f16] = unsafe {
                    std::slice::from_raw_parts(data.as_ptr() as *const f16, data.len() / 2)
                };
                let array = Array3::from_shape_vec((h, w, 4), f16_slice.to_vec())
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
                let pyarray = array.into_pyarray(py);
                pyarray
                    .try_readwrite()
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
                    .make_nonwriteable();
                Ok(pyarray.into_any())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Capture — worker-thread-based pipeline
// ---------------------------------------------------------------------------

/// Screen/window capture pipeline
///
/// Construct via class methods:
///   cap = capture.monitor(0)
///   cap = capture.window("notepad.exe")
///
/// Supports context manager:
///   with capture.monitor(0) as cap:
///       frame = cap.capture()
///
/// Thread-safe: can be shared across Python threads, passed to atexit handlers,
/// or dropped from any thread without panic.
#[pyclass(name = "capture")]
struct Capture {
    cmd_tx: Option<mpsc::Sender<Command>>,
    resp_rx: Option<Mutex<mpsc::Receiver<Response>>>,
    handle: Option<JoinHandle<()>>,
}

impl Capture {
    /// Send a command and unwrap the response, erroring if already closed.
    ///
    /// Releases the GIL before acquiring the Mutex to prevent deadlock:
    /// without this, thread A (holds Mutex, waits for GIL) and thread B
    /// (holds GIL, waits for Mutex) would deadlock.
    fn call(&self, py: Python<'_>, cmd: Command) -> PyResult<Response> {
        let tx = self
            .cmd_tx
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Capture is closed"))?;
        let rx_mutex = self
            .resp_rx
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Capture is closed"))?;

        // Release GIL before acquiring Mutex — consistent lock ordering prevents deadlock.
        let (send_ok, recv_result) = py.detach(|| {
            let rx = rx_mutex.lock();
            match rx {
                Ok(rx) => match tx.send(cmd) {
                    Ok(()) => (true, rx.recv().ok()),
                    Err(_) => (false, None),
                },
                Err(_) => (false, None),
            }
        });

        if !send_ok {
            return Err(PyRuntimeError::new_err("Capture is closed"));
        }
        recv_result.ok_or_else(|| PyRuntimeError::new_err("Worker thread exited unexpectedly"))
    }

    /// Shut down the worker thread, optionally waiting for it to finish.
    fn shutdown(&mut self, join: bool) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Command::Close);
        }
        // Drop receiver so worker can detect disconnect if Close wasn't processed.
        self.resp_rx.take();
        if join {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Don't join — Drop may run under GIL (e.g. GC, atexit) and join could
        // block if WGC session teardown is slow. Worker exits on its own.
        self.shutdown(false);
    }
}

#[pymethods]
impl Capture {
    /// Create capture pipeline by monitor index
    ///
    /// Args:
    ///     index: Monitor index, defaults to 0
    ///     mode: Capture mode — "auto", "hdr", or "sdr"
    #[staticmethod]
    #[pyo3(signature = (index=0, mode="auto"))]
    fn monitor(py: Python<'_>, index: usize, mode: &str) -> PyResult<Self> {
        let policy = parse_mode(mode)?;

        let (cmd_tx, resp_rx, handle) = spawn_worker(Box::new(move || {
            pipeline::CapturePipeline::monitor(index, policy)
        }))
        .map_err(PyRuntimeError::new_err)?;

        // Query is_hdr for mode mismatch warning.
        let cap = Capture {
            cmd_tx: Some(cmd_tx),
            resp_rx: Some(Mutex::new(resp_rx)),
            handle: Some(handle),
        };
        if let Ok(Response::Bool(is_hdr)) = cap.call(py, Command::IsHdr) {
            warn_mode_mismatch(py, policy, is_hdr)?;
        }
        Ok(cap)
    }

    /// Create window capture pipeline by process name
    ///
    /// Args:
    ///     process_name: Process name (e.g., "notepad.exe")
    ///     index: Window index for processes with the same name, defaults to 0
    ///     mode: Capture mode — "auto", "hdr", or "sdr"
    ///     headless: Crop title bar and borders, defaults to true
    #[staticmethod]
    #[pyo3(signature = (process_name, index=None, mode="auto", headless=true))]
    fn window(
        py: Python<'_>,
        process_name: &str,
        index: Option<usize>,
        mode: &str,
        headless: bool,
    ) -> PyResult<Self> {
        let policy = parse_mode(mode)?;
        let name = process_name.to_string();

        let (cmd_tx, resp_rx, handle) = spawn_worker(Box::new(move || {
            pipeline::CapturePipeline::window(&name, index, policy, headless)
        }))
        .map_err(PyRuntimeError::new_err)?;

        let cap = Capture {
            cmd_tx: Some(cmd_tx),
            resp_rx: Some(Mutex::new(resp_rx)),
            handle: Some(handle),
        };
        if let Ok(Response::Bool(is_hdr)) = cap.call(py, Command::IsHdr) {
            warn_mode_mismatch(py, policy, is_hdr)?;
        }
        Ok(cap)
    }

    /// Whether the target monitor has HDR enabled.
    #[getter]
    fn is_hdr(&self, py: Python<'_>) -> PyResult<bool> {
        match self.call(py, Command::IsHdr)? {
            Response::Bool(v) => Ok(v),
            _ => Err(PyRuntimeError::new_err("Unexpected worker response")),
        }
    }

    /// Screenshot mode: capture a fresh frame
    ///
    /// Drain backlog and wait for DWM to push new frame, guarantees returned frame is generated after the call.
    /// Releases GIL during wait and readback, doesn't block other Python threads.
    fn capture(&self, py: Python<'_>) -> PyResult<CapturedFrame> {
        match self.call(py, Command::Capture)? {
            Response::Frame(Ok(frame)) => Ok(CapturedFrame { inner: frame }),
            Response::Frame(Err(e)) => Err(PyRuntimeError::new_err(e)),
            _ => Err(PyRuntimeError::new_err("Unexpected worker response")),
        }
    }

    /// Continuous capture mode: grab latest available frame
    ///
    /// Drain backlog and keep last frame, wait for new frame when pool is empty. Lower latency.
    /// Releases GIL during wait and readback, doesn't block other Python threads.
    fn grab(&self, py: Python<'_>) -> PyResult<CapturedFrame> {
        match self.call(py, Command::Grab)? {
            Response::Frame(Ok(frame)) => Ok(CapturedFrame { inner: frame }),
            Response::Frame(Err(e)) => Err(PyRuntimeError::new_err(e)),
            _ => Err(PyRuntimeError::new_err("Unexpected worker response")),
        }
    }

    /// Release capture resources
    fn close(&mut self, py: Python<'_>) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Command::Close);
        }
        self.resp_rx.take();
        if let Some(h) = self.handle.take() {
            // Release GIL while waiting for worker teardown (WGC session stop may be slow).
            py.detach(|| {
                let _ = h.join();
            });
        }
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        self.close(py);
        false // Don't swallow exceptions
    }

    fn __repr__(&self) -> String {
        if self.cmd_tx.is_some() {
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
    let policy = parse_mode(mode)?;
    let window_name = window.map(|s| s.to_string());

    // Spawn worker, create pipeline, capture one frame, shut down.
    let (cmd_tx, resp_rx, handle) = spawn_worker(Box::new(move || match window_name {
        Some(name) => pipeline::CapturePipeline::window(&name, window_index, policy, headless),
        None => pipeline::CapturePipeline::monitor(monitor, policy),
    }))
    .map_err(PyRuntimeError::new_err)?;

    // Note: manual GIL release here because mpsc::Receiver is !Sync,
    // so &Receiver doesn't satisfy the Ungil bound required by allow_threads().
    // The operations inside (channel send/recv) never panic, so GIL is always restored.
    cmd_tx
        .send(Command::IsHdr)
        .map_err(|_| PyRuntimeError::new_err("Worker thread is gone"))?;
    // Release GIL while waiting for is_hdr response.
    let is_hdr_resp = unsafe {
        let save = pyo3::ffi::PyEval_SaveThread();
        let r = resp_rx.recv();
        pyo3::ffi::PyEval_RestoreThread(save);
        r
    };
    if let Ok(Response::Bool(is_hdr)) = is_hdr_resp {
        warn_mode_mismatch(py, policy, is_hdr)?;
    }

    // Capture frame (releases GIL during wait + readback).
    cmd_tx
        .send(Command::Capture)
        .map_err(|_| PyRuntimeError::new_err("Worker thread is gone"))?;
    let capture_resp = unsafe {
        let save = pyo3::ffi::PyEval_SaveThread();
        let r = resp_rx.recv();
        pyo3::ffi::PyEval_RestoreThread(save);
        r
    };

    // Shut down worker.
    let _ = cmd_tx.send(Command::Close);
    drop(cmd_tx);
    drop(resp_rx);
    let _ = handle.join();

    match capture_resp {
        Ok(Response::Frame(Ok(frame))) => Ok(CapturedFrame { inner: frame }),
        Ok(Response::Frame(Err(e))) => Err(PyRuntimeError::new_err(e)),
        _ => Err(PyRuntimeError::new_err("Unexpected worker response")),
    }
}

/// HDR-aware screen capture library for Windows
#[pymodule]
fn hdrcapture(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CapturedFrame>()?;
    m.add_class::<Capture>()?;
    m.add_function(wrap_pyfunction!(screenshot, m)?)?;
    Ok(())
}
