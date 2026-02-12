// Windows Graphics Capture 核心实现
//
// 统一使用 BGRA8 格式捕获，DWM 自动处理 HDR→SDR 色调映射。
// 使用 FrameArrived 事件 + WaitForSingleObject 实现零延迟帧等待。

use anyhow::{bail, Context, Result};
use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
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
    /// FrameArrived 信号事件（内核对象，WaitForSingleObject 用）
    frame_event: HANDLE,
    /// 捕获目标初始尺寸（用于预创建 Staging Texture）
    target_width: u32,
    target_height: u32,
}

impl WGCCapture {
    /// 启动捕获
    pub fn start(&self) -> Result<()> {
        self.session.StartCapture()?;
        Ok(())
    }

    /// 捕获目标的初始尺寸
    pub fn target_size(&self) -> (u32, u32) {
        (self.target_width, self.target_height)
    }

    /// 尝试从 FramePool 取出一帧（非阻塞）
    ///
    /// 返回原始 `Direct3D11CaptureFrame`，调用方控制其生命周期。
    /// 必须在 frame 被 drop 之前完成对底层 surface 的访问（如 CopyResource）。
    pub fn try_get_next_frame(&self) -> Result<Direct3D11CaptureFrame> {
        Ok(self.frame_pool.TryGetNextFrame()?)
    }

    /// 等待下一帧到达（阻塞，带超时）
    ///
    /// 使用内核事件等待，不消耗 CPU，唤醒延迟 ~0ms。
    /// 返回后调用 `try_get_next_frame()` 获取帧。
    pub fn wait_for_frame(&self, timeout_ms: u32) -> Result<()> {
        // SAFETY: frame_event 在 init_capture 中创建，生命周期覆盖整个 WGCCapture
        let result = unsafe { WaitForSingleObject(self.frame_event, timeout_ms) };
        if result.0 != 0 {
            // WAIT_TIMEOUT = 0x102, WAIT_FAILED = 0xFFFFFFFF
            bail!(
                "WaitForSingleObject returned 0x{:X} (timeout: {}ms)",
                result.0,
                timeout_ms
            );
        }
        Ok(())
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

impl Drop for WGCCapture {
    fn drop(&mut self) {
        if !self.frame_event.is_invalid() {
            // SAFETY: frame_event 是我们创建的有效句柄，只关闭一次
            unsafe {
                let _ = CloseHandle(self.frame_event);
            }
        }
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
/// 注册 FrameArrived 事件回调，通过内核事件实现零延迟帧等待。
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

    // 3. 创建内核事件（自动复位，初始无信号）
    // SAFETY: CreateEventW 创建匿名事件对象
    let frame_event =
        unsafe { CreateEventW(None, false, false, None).context("Failed to create frame event")? };

    // 4. 注册 FrameArrived 回调：仅 SetEvent，不做任何 D3D 操作
    // 将 HANDLE 转为 usize 传入闭包，绕过 Send 限制。
    // SAFETY: 内核事件句柄是线程安全的，可从任意线程 SetEvent。
    let event_ptr = frame_event.0 as usize;
    frame_pool.FrameArrived(&TypedEventHandler::<
        Direct3D11CaptureFramePool,
        windows::core::IInspectable,
    >::new(move |_, _| {
        unsafe {
            if SetEvent(HANDLE(event_ptr as *mut _)).is_err() {
                eprintln!("hdrcapture: SetEvent failed in FrameArrived callback");
            }
        }
        Ok(())
    }))?;

    let session = frame_pool.CreateCaptureSession(&item)?;
    session.SetIsBorderRequired(false)?;

    Ok(WGCCapture {
        _item: item,
        frame_pool,
        session,
        frame_event,
        target_width: size.Width as u32,
        target_height: size.Height as u32,
    })
}
