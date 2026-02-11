// 捕获引擎模块

pub mod target;
pub mod wgc;

// 重新导出常用类型和函数
pub use target::{enable_dpi_awareness, find_monitor, find_window};
pub use wgc::{init_capture, CaptureTarget, WGCCapture};
