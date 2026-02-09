// Windows Graphics Capture 实现

use anyhow::{Context, Result};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::d3d11::D3D11Context;

/// WGC 捕获会话
pub struct WGCCapture {
    pub item: GraphicsCaptureItem,
    pub frame_pool: Direct3D11CaptureFramePool,
    pub session: GraphicsCaptureSession,
}

/// 从显示器句柄创建 GraphicsCaptureItem
pub fn create_capture_item_for_monitor(hmonitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    unsafe {
        // 获取 IGraphicsCaptureItemInterop 接口
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;

        // 调用 CreateForMonitor
        let item = interop.CreateForMonitor(hmonitor)?;

        Ok(item)
    }
}

/// 从窗口句柄创建 GraphicsCaptureItem
pub fn create_capture_item_for_window(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    unsafe {
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item = interop.CreateForWindow(hwnd)?;
        Ok(item)
    }
}

/// 初始化 WGC 捕获会话
pub fn init_capture(d3d_ctx: &D3D11Context, item: GraphicsCaptureItem) -> Result<WGCCapture> {
    let size = item.Size()?;

    println!("📐 捕获目标尺寸: {}x{}", size.Width, size.Height);

    // 创建 FramePool（关键：使用 R16G16B16A16Float 格式捕获 HDR 数据）
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d_ctx.direct3d_device,
        DirectXPixelFormat::R16G16B16A16Float, // 16-bit HDR 格式
        2,                                     // 缓冲区数量
        size,
    )
    .context("CreateFreeThreaded 失败")?;

    let session = frame_pool.CreateCaptureSession(&item)?;
    session.SetIsBorderRequired(false)?;

    println!("✅ WGC 捕获会话初始化成功");

    Ok(WGCCapture {
        item,
        frame_pool,
        session,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d11::create_d3d11_device;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC};

    // 获取主显示器句柄（用于测试）
    fn get_primary_monitor() -> Option<HMONITOR> {
        unsafe {
            let mut monitor = None;

            let _ = EnumDisplayMonitors(
                Some(HDC::default()),
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut monitor as *mut _ as isize),
            );

            monitor
        }
    }

    unsafe extern "system" fn monitor_enum_proc(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitor_ptr = lparam.0 as *mut Option<HMONITOR>;
        *monitor_ptr = Some(hmonitor);
        BOOL(0) // 返回 false 停止枚举（只获取第一个）
    }

    #[test]
    fn test_create_capture_item() {
        let monitor = get_primary_monitor().expect("无法获取显示器句柄");
        let item = create_capture_item_for_monitor(monitor).expect("创建 CaptureItem 失败");

        // 验证可以获取尺寸
        let size = item.Size().expect("无法获取尺寸");
        assert!(size.Width > 0);
        assert!(size.Height > 0);

        println!("✅ CaptureItem 创建成功: {}x{}", size.Width, size.Height);
    }

    #[test]
    fn test_init_capture() {
        let d3d_ctx = create_d3d11_device().expect("D3D11 设备创建失败");
        let monitor = get_primary_monitor().expect("无法获取显示器句柄");
        let item = create_capture_item_for_monitor(monitor).expect("创建 CaptureItem 失败");

        let capture = init_capture(&d3d_ctx, item).expect("初始化捕获失败");

        // 验证会话已创建
        assert!(capture.session.IsCursorCaptureEnabled().is_ok());

        println!("✅ WGC 捕获会话测试通过");
    }
}
