// Capture engine module

pub mod policy;
pub mod target;
pub mod wgc;

// Re-export commonly used types and functions
pub use policy::CapturePolicy;
pub use target::{enable_dpi_awareness, find_monitor, find_window, WindowSelector};
pub use wgc::{init_capture, CaptureTarget, WGCCapture};
