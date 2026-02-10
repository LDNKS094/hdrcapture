// Windows Graphics Capture 核心实现

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

// Local Modules
use super::hdr_detection::is_monitor_hdr;
use super::monitor::get_window_monitor;
use super::types::CaptureTarget;
use crate::d3d11::D3D11Context;

/// WGC 捕获会话
pub struct WGCCapture {
    pub item: GraphicsCaptureItem,
    pub frame_pool: Direct3D11CaptureFramePool,
    pub session: GraphicsCaptureSession,
    pub target: CaptureTarget, // 捕获目标
    pub is_hdr: bool,          // 是否为 HDR 显示器
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
///
/// # Arguments
/// * `d3d_ctx` - D3D11 设备上下文
/// * `target` - 捕获目标（显示器或窗口）
///
/// # Returns
/// * `WGCCapture` - 捕获会话，包含 HDR 状态信息
pub fn init_capture(d3d_ctx: &D3D11Context, target: CaptureTarget) -> Result<WGCCapture> {
    // 1. 根据目标类型创建 GraphicsCaptureItem
    let item = match target {
        CaptureTarget::Monitor(monitor) => create_capture_item_for_monitor(monitor)?,
        CaptureTarget::Window(hwnd) => create_capture_item_for_window(hwnd)?,
    };

    let size = item.Size()?;

    // 2. 获取目标所在的显示器句柄（用于 HDR 检测）
    let monitor = match target {
        CaptureTarget::Monitor(m) => m,
        CaptureTarget::Window(hwnd) => get_window_monitor(hwnd),
    };

    // 3. 检测显示器 HDR 状态
    let is_hdr = is_monitor_hdr(monitor).unwrap_or(false);

    // 4. 根据 HDR 状态动态选择格式
    let format = if is_hdr {
        DirectXPixelFormat::R16G16B16A16Float // HDR: 16-bit float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized // SDR: 8-bit
    };

    println!(
        "🎨 捕获目标: {:?} | 显示器模式: {} | 格式: {:?}",
        target,
        if is_hdr { "HDR" } else { "SDR" },
        format
    );

    // 5. 创建 FramePool
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d_ctx.direct3d_device,
        format,
        2, // 缓冲区数量
        size,
    )?;

    let session = frame_pool.CreateCaptureSession(&item)?;

    session.SetIsBorderRequired(false)?;

    Ok(WGCCapture {
        item,
        frame_pool,
        session,
        target,
        is_hdr,
    })
}
