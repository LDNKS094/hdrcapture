// 捕获引擎模块

pub mod hdr_detection;
pub mod monitor;
pub mod types;
pub mod wgc;

// 重新导出常用类型和函数
pub use hdr_detection::{clear_hdr_cache, enumerate_monitors, is_monitor_hdr};
pub use monitor::{enable_dpi_awareness, get_window_monitor};
pub use types::{CaptureTarget, MonitorInfo};
pub use wgc::{init_capture, WGCCapture};
