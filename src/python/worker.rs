use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crate::pipeline;

pub(crate) enum Command {
    Capture,
    Grab,
    IsHdr,
    Close,
}

pub(crate) enum Response {
    Frame(Result<pipeline::CapturedFrame, String>),
    Bool(bool),
    Closed,
}

pub(crate) type WorkerHandle = (
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
pub(crate) fn spawn_worker(
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
