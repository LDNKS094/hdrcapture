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
    use crate::d3d11::create_d3d11_device;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFO, MONITORINFOEXW,
    };

    #[test]
    fn test_wgc_capture_pipeline() {
        use std::thread;
        use std::time::Duration;
        use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

        // 1. 准备环境
        let d3d_ctx = create_d3d11_device().unwrap();
        let item = setup_test_capture_item();

        // 2. 初始化捕获会话
        let capture = init_capture(&d3d_ctx, item).unwrap();
        println!("✅ WGC 会话初始化成功");

        // 3. 启动捕获
        capture.start().unwrap();
        println!("✅ 捕获已启动，等待帧...");

        // 4. 等待一帧准备好 (100ms 足够大多数情况)
        thread::sleep(Duration::from_millis(100));

        // 5. 捕获一帧
        let texture = capture.capture_frame().unwrap();
        println!("✅ 成功获取帧");

        // 6. 验证纹理格式 (关键步骤)
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);

            println!("📊 纹理信息:");
            println!("   格式: {:?} (预期: R16G16B16A16_FLOAT)", desc.Format);
            println!("   尺寸: {}x{}", desc.Width, desc.Height);
            println!("   MipLevels: {}", desc.MipLevels);

            assert_eq!(
                desc.Format, DXGI_FORMAT_R16G16B16A16_FLOAT,
                "纹理格式必须是 FP16"
            );
            assert!(desc.Width > 0);
            assert!(desc.Height > 0);
            assert_eq!(desc.MipLevels, 1, "截图纹理不应有 Mipmaps");
        }

        println!("🎉 WGC 捕获管线测试通过！");
    }

    // --- 测试辅助函数 ---

    /// 测试辅助函数：创建测试用的 CaptureItem
    fn setup_test_capture_item() -> GraphicsCaptureItem {
        print_all_monitors();
        let monitor = get_primary_monitor().expect("无法获取显示器句柄");
        create_capture_item_for_monitor(monitor).unwrap()
    }

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
        // 确保在枚举前启用 DPI 感知
        enable_dpi_awareness();

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

    /// 获取主显示器句柄
    fn get_primary_monitor() -> Option<HMONITOR> {
        let monitors = enumerate_monitors();
        monitors
            .into_iter()
            .find(|m| m.is_primary)
            .map(|m| m.handle)
    }

    /// 打印所有显示器信息
    fn print_all_monitors() {
        let monitors = enumerate_monitors();
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
        }
        println!();
    }
}
