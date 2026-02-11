// 捕获引擎模块

pub mod monitor;
pub mod wgc;

// 重新导出常用类型和函数
pub use monitor::enable_dpi_awareness;
pub use wgc::{init_capture, CaptureTarget, WGCCapture};
