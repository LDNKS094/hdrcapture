use std::sync::{mpsc, Mutex};
use std::thread::JoinHandle;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use super::frame::CapturedFrame;
use super::helpers::{parse_mode, warn_mode_mismatch};
use super::worker::{spawn_worker, Command, Response};
use crate::pipeline;

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
pub(crate) struct Capture {
    cmd_tx: Option<mpsc::Sender<Command>>,
    resp_rx: Option<Mutex<mpsc::Receiver<Response>>>,
    handle: Option<JoinHandle<()>>,
}

impl Capture {
    fn warn(py: Python<'_>, message: &str) -> PyResult<()> {
        let warnings = py.import("warnings")?;
        warnings.call_method1("warn", (message,))?;
        Ok(())
    }

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
    pub(crate) fn monitor(py: Python<'_>, index: usize, mode: &str) -> PyResult<Self> {
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

    /// Create window capture pipeline by window selector inputs.
    ///
    /// Args:
    ///     process: Process name (e.g., "notepad.exe")
    ///     pid: Target process id
    ///     hwnd: Target window handle
    ///     index: Ranked window index within candidate windows
    ///     mode: Capture mode — "auto", "hdr", or "sdr"
    ///     headless: Crop title bar and borders, defaults to true
    #[staticmethod]
    #[pyo3(signature = (process=None, *, pid=None, hwnd=None, index=None, mode="auto", headless=true))]
    pub(crate) fn window(
        py: Python<'_>,
        process: Option<String>,
        pid: Option<u32>,
        hwnd: Option<isize>,
        index: Option<usize>,
        mode: &str,
        headless: bool,
    ) -> PyResult<Self> {
        let policy = parse_mode(mode)?;

        if hwnd.is_none() && pid.is_none() && process.is_none() {
            return Err(PyRuntimeError::new_err(
                "window requires one of: hwnd, pid, process",
            ));
        }

        if hwnd.is_some() && (pid.is_some() || process.is_some()) {
            Self::warn(py, "'hwnd' provided; ignoring 'pid' and 'process'.")?;
        } else if pid.is_some() && process.is_some() {
            Self::warn(py, "'pid' provided; ignoring 'process'.")?;
        }

        let process_for_worker = process;

        let (cmd_tx, resp_rx, handle) = spawn_worker(Box::new(move || {
            pipeline::CapturePipeline::window(
                process_for_worker.as_deref(),
                pid,
                hwnd,
                index,
                policy,
                headless,
            )
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
    pub(crate) fn capture(&self, py: Python<'_>) -> PyResult<CapturedFrame> {
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
    pub(crate) fn close(&mut self, py: Python<'_>) {
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
