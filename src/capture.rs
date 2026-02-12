// Capture engine module

pub mod target;
pub mod wgc;

// Re-export commonly used types and functions
pub use target::{enable_dpi_awareness, find_monitor, find_window};
pub use wgc::{init_capture, CaptureTarget, WGCCapture};
