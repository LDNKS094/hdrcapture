// 捕获模块公共类型定义

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HMONITOR;

/// 捕获目标类型
#[derive(Debug, Clone, Copy)]
pub enum CaptureTarget {
    /// 显示器捕获
    Monitor(HMONITOR),
    /// 窗口捕获
    Window(HWND),
}

/// 显示器信息
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// 显示器句柄（存储为 isize 以支持跨线程传递）
    handle_ptr: isize,
    /// 显示器名称（如 "\\\\.\\DISPLAY1"）
    pub name: String,
    /// 是否为主显示器
    pub is_primary: bool,
    /// 宽度（像素）
    pub width: u32,
    /// 高度（像素）
    pub height: u32,
    /// 是否支持并开启 HDR
    pub is_hdr: bool,
}

impl MonitorInfo {
    /// 创建新的显示器信息
    pub fn new(
        handle: HMONITOR,
        name: String,
        is_primary: bool,
        width: u32,
        height: u32,
        is_hdr: bool,
    ) -> Self {
        Self {
            handle_ptr: handle.0 as isize,
            name,
            is_primary,
            width,
            height,
            is_hdr,
        }
    }

    /// 获取显示器句柄
    pub fn handle(&self) -> HMONITOR {
        HMONITOR(self.handle_ptr as *mut _)
    }
}
