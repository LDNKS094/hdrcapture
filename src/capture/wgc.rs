// Windows Graphics Capture 实现

// 1. External Crates
use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

// 2. Local Modules
use crate::d3d11::D3D11Context;

/// WGC 捕获会话
pub struct WGCCapture {
    pub item: GraphicsCaptureItem,
    pub frame_pool: Direct3D11CaptureFramePool,
    pub session: GraphicsCaptureSession,
}

impl WGCCapture {
    /// 启动捕获
    pub fn start(&self) -> Result<()> {
        self.session.StartCapture()?;
        Ok(())
    }

    /// 捕获一帧并返回 ID3D11Texture2D
    pub fn capture_frame(&self) -> Result<ID3D11Texture2D> {
        // 从 FramePool 获取帧
        let frame = self.frame_pool.TryGetNextFrame()?;

        // 从 Frame 获取 IDirect3DSurface
        let surface: IDirect3DSurface = frame.Surface()?;

        // 通过 COM 互操作获取底层 ID3D11Texture2D
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;

        // SAFETY: IDirect3DDxgiInterfaceAccess::GetInterface 是 unsafe 的 Win32 API 调用
        let texture: ID3D11Texture2D = unsafe {
            access
                .GetInterface()
                .context("Failed to get ID3D11Texture2D interface")?
        };

        Ok(texture)
    }
}

/// 从显示器句柄创建 GraphicsCaptureItem
pub fn create_capture_item_for_monitor(hmonitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    unsafe {
        // 获取 IGraphicsCaptureItemInterop 接口
        // SAFETY: 工厂函数调用，失败可能意味着系统不支持或 COM 未初始化
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .context("Failed to get IGraphicsCaptureItemInterop factory")?;

        // 调用 CreateForMonitor
        let item = interop
            .CreateForMonitor(hmonitor)
            .context("Failed to create CaptureItem for monitor")?;

        Ok(item)
    }
}

/// 从窗口句柄创建 GraphicsCaptureItem
pub fn create_capture_item_for_window(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    unsafe {
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .context("Failed to get IGraphicsCaptureItemInterop factory")?;

        let item = interop
            .CreateForWindow(hwnd)
            .context("Failed to create CaptureItem for window")?;
        Ok(item)
    }
}

/// 初始化 WGC 捕获会话
pub fn init_capture(d3d_ctx: &D3D11Context, item: GraphicsCaptureItem) -> Result<WGCCapture> {
    let size = item.Size()?;

    // 创建 FramePool（关键：使用 R16G16B16A16Float 格式捕获 HDR 数据）
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d_ctx.direct3d_device,
        DirectXPixelFormat::R16G16B16A16Float, // 16-bit HDR 格式
        2,                                     // 缓冲区数量
        size,
    )?;

    let session = frame_pool.CreateCaptureSession(&item)?;

    session.SetIsBorderRequired(false)?;

    Ok(WGCCapture {
        item,
        frame_pool,
        session,
    })
}

/// 启用 DPI 感知（仅用于测试或需要强制开启的场景）
pub fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        // SAFETY: 这是一个 best-effort 调用。
        // 如果进程已经设置了 DPI 感知模式（例如被 GUI 框架设置过），
        // 此调用会返回 FALSE (E_ACCESSDENIED)。我们显式忽略此错误，
        // 因为我们的目标只是确保它被开启，而不是必须由我们开启。
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFO, MONITORINFOEXW,
    };

    /// 单元测试：验证显示器枚举功能
    /// 这是一个调试辅助测试，用于验证系统能正确检测显示器
    #[test]
    fn test_monitor_enumeration() {
        enable_dpi_awareness();

        let monitors = enumerate_monitors();

        // 验证至少有一个显示器
        assert!(!monitors.is_empty(), "应该至少检测到一个显示器");

        // 验证有且仅有一个主显示器
        let primary_count = monitors.iter().filter(|m| m.is_primary).count();
        assert_eq!(primary_count, 1, "应该有且仅有一个主显示器");

        // 打印显示器信息（用于调试）
        println!("\n🖥️  检测到 {} 个显示器:", monitors.len());
        for (i, monitor) in monitors.iter().enumerate() {
            println!(
                "  [{}] {} {}x{} {}",
                i,
                monitor.name,
                monitor.width,
                monitor.height,
                if monitor.is_primary {
                    "⭐ 主显示器"
                } else {
                    ""
                }
            );

            // 验证分辨率合理
            assert!(monitor.width > 0, "显示器宽度必须大于 0");
            assert!(monitor.height > 0, "显示器高度必须大于 0");
        }
    }

    // --- 测试辅助结构和函数 ---

    /// 显示器信息
    #[derive(Debug)]
    struct MonitorInfo {
        handle: HMONITOR,
        name: String,
        is_primary: bool,
        width: i32,
        height: i32,
    }

    /// 枚举所有显示器
    fn enumerate_monitors() -> Vec<MonitorInfo> {
        unsafe {
            let mut monitors = Vec::new();

            let _ = EnumDisplayMonitors(
                Some(HDC::default()),
                None,
                Some(enum_monitors_proc),
                LPARAM(&mut monitors as *mut _ as isize),
            );

            monitors
        }
    }

    unsafe extern "system" fn enum_monitors_proc(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &mut *(lparam.0 as *mut Vec<MonitorInfo>);

        // 获取显示器信息
        let mut monitor_info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        if GetMonitorInfoW(hmonitor, &mut monitor_info.monitorInfo as *mut _ as *mut _).as_bool() {
            let name = String::from_utf16_lossy(&monitor_info.szDevice)
                .trim_end_matches('\0')
                .to_string();

            let is_primary = (monitor_info.monitorInfo.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY = 1

            let width =
                monitor_info.monitorInfo.rcMonitor.right - monitor_info.monitorInfo.rcMonitor.left;
            let height =
                monitor_info.monitorInfo.rcMonitor.bottom - monitor_info.monitorInfo.rcMonitor.top;

            monitors.push(MonitorInfo {
                handle: hmonitor,
                name,
                is_primary,
                width,
                height,
            });
        }

        BOOL(1) // 继续枚举
    }
}
