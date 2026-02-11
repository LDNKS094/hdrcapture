// Windows Graphics Capture 核心实现
//
// 统一使用 BGRA8 格式捕获，DWM 自动处理 HDR→SDR 色调映射。

use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::d3d11::D3D11Context;

// ---------------------------------------------------------------------------
// 公共类型
// ---------------------------------------------------------------------------

/// 捕获目标类型
#[derive(Debug, Clone, Copy)]
pub enum CaptureTarget {
    /// 显示器捕获
    Monitor(HMONITOR),
    /// 窗口捕获
    Window(HWND),
}

// ---------------------------------------------------------------------------
// WGC 捕获会话
// ---------------------------------------------------------------------------

/// WGC 捕获会话
pub struct WGCCapture {
    /// 持有所有权，drop 时停止捕获
    _item: GraphicsCaptureItem,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
}

impl WGCCapture {
    /// 启动捕获
    pub fn start(&self) -> Result<()> {
        self.session.StartCapture()?;
        Ok(())
    }

    /// 尝试从 FramePool 取出一帧（非阻塞）
    ///
    /// 返回原始 `Direct3D11CaptureFrame`，调用方控制其生命周期。
    /// 必须在 frame 被 drop 之前完成对底层 surface 的访问（如 CopyResource）。
    pub fn try_get_next_frame(&self) -> Result<Direct3D11CaptureFrame> {
        self.frame_pool
            .TryGetNextFrame()
            .context("TryGetNextFrame failed")
    }

    /// 从 `Direct3D11CaptureFrame` 中提取 `ID3D11Texture2D`
    ///
    /// frame 必须在返回的 texture 被使用完毕后才能 drop。
    pub fn frame_to_texture(frame: &Direct3D11CaptureFrame) -> Result<ID3D11Texture2D> {
        let surface: IDirect3DSurface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;

        // SAFETY: GetInterface 是 Win32 COM 互操作调用
        // access 由上方 cast() 成功获取，保证有效
        let texture: ID3D11Texture2D = unsafe {
            access
                .GetInterface()
                .context("Failed to get ID3D11Texture2D interface")?
        };

        Ok(texture)
    }
}

// ---------------------------------------------------------------------------
// 捕获初始化
// ---------------------------------------------------------------------------

/// 从显示器句柄创建 GraphicsCaptureItem
fn create_capture_item_for_monitor(hmonitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    // SAFETY: 工厂函数调用，失败可能意味着系统不支持或 COM 未初始化
    unsafe {
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .context("Failed to get IGraphicsCaptureItemInterop factory")?;

        let item = interop
            .CreateForMonitor(hmonitor)
            .context("Failed to create CaptureItem for monitor")?;

        Ok(item)
    }
}

/// 从窗口句柄创建 GraphicsCaptureItem
fn create_capture_item_for_window(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    // SAFETY: 工厂函数调用，同上
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
/// 统一使用 BGRA8 格式，DWM 自动处理 HDR→SDR 色调映射。
///
/// # Arguments
/// * `d3d_ctx` - D3D11 设备上下文
/// * `target` - 捕获目标（显示器或窗口）
pub fn init_capture(d3d_ctx: &D3D11Context, target: CaptureTarget) -> Result<WGCCapture> {
    // 1. 根据目标类型创建 GraphicsCaptureItem
    let item = match target {
        CaptureTarget::Monitor(monitor) => create_capture_item_for_monitor(monitor)?,
        CaptureTarget::Window(hwnd) => create_capture_item_for_window(hwnd)?,
    };

    let size = item.Size()?;

    // 2. 创建 FramePool（固定 BGRA8，DWM 自动处理 HDR→SDR）
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d_ctx.direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2, // 缓冲区数量
        size,
    )?;

    let session = frame_pool.CreateCaptureSession(&item)?;
    session.SetIsBorderRequired(false)?;

    Ok(WGCCapture {
        _item: item,
        frame_pool,
        session,
    })
}
