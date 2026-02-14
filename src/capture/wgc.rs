// Windows Graphics Capture core implementation
//
// Capture pixel format is selected by policy.
// Uses FrameArrived event + WaitForSingleObject for zero-latency frame waiting.

use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::IDXGIOutput6;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use super::policy::CapturePolicy;
use crate::d3d11::D3D11Context;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Capture target type
#[derive(Debug, Clone, Copy)]
pub enum CaptureTarget {
    /// Monitor capture
    Monitor(HMONITOR),
    /// Window capture
    Window(HWND),
}

// ---------------------------------------------------------------------------
// WGC capture session
// ---------------------------------------------------------------------------

/// WGC capture session
pub struct WGCCapture {
    /// Holds ownership, stops capture on drop
    _item: GraphicsCaptureItem,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// FrameArrived callback token (for unregistering on drop)
    frame_arrived_token: i64,
    /// FrameArrived signal event (kernel object, for WaitForSingleObject)
    frame_event: HANDLE,
    /// Indicates teardown has started (callback should stop signaling)
    shutting_down: Arc<AtomicBool>,
    /// Initial size of capture target (for pre-creating Staging Texture)
    target_width: u32,
    target_height: u32,
    /// Whether the target monitor has HDR enabled (detected once at init)
    target_hdr: bool,
}

impl WGCCapture {
    /// Start capture
    pub fn start(&self) -> Result<()> {
        self.session.StartCapture()?;
        Ok(())
    }

    /// Initial size of capture target
    pub fn target_size(&self) -> (u32, u32) {
        (self.target_width, self.target_height)
    }

    /// Whether the target monitor has HDR enabled.
    pub fn is_hdr(&self) -> bool {
        self.target_hdr
    }

    /// Try to get a frame from FramePool (non-blocking)
    ///
    /// Returns the raw `Direct3D11CaptureFrame`, caller controls its lifetime.
    /// Must complete access to the underlying surface (e.g., CopyResource) before frame is dropped.
    pub fn try_get_next_frame(&self) -> Result<Direct3D11CaptureFrame> {
        Ok(self.frame_pool.TryGetNextFrame()?)
    }

    /// Wait for next frame arrival (blocking, with timeout)
    ///
    /// Uses kernel event waiting, no CPU consumption, wake latency ~0ms.
    /// Call `try_get_next_frame()` to get the frame after return.
    pub fn wait_for_frame(&self, timeout_ms: u32) -> Result<()> {
        // SAFETY: frame_event is created in init_capture, lifetime covers entire WGCCapture
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

    /// Extract `ID3D11Texture2D` from `Direct3D11CaptureFrame`
    ///
    /// frame must not be dropped until the returned texture is no longer needed.
    pub fn frame_to_texture(frame: &Direct3D11CaptureFrame) -> Result<ID3D11Texture2D> {
        let surface: IDirect3DSurface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;

        // SAFETY: GetInterface is Win32 COM interop call
        // access obtained successfully from cast() above, guaranteed valid
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
        self.shutting_down.store(true, Ordering::Relaxed);

        let _ = self.frame_pool.RemoveFrameArrived(self.frame_arrived_token);

        if !self.frame_event.is_invalid() {
            // SAFETY: frame_event is a valid handle we created, only close once
            unsafe {
                let _ = CloseHandle(self.frame_event);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Capture initialization
// ---------------------------------------------------------------------------

/// Create GraphicsCaptureItem from monitor handle
fn create_capture_item_for_monitor(hmonitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    // SAFETY: factory function call, failure may mean system not supported or COM not initialized
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

/// Create GraphicsCaptureItem from window handle
fn create_capture_item_for_window(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    // SAFETY: factory function call, same as above
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

/// Initialize WGC capture session
///
/// Uses policy-resolved pixel format to create the frame pool.
/// Registers FrameArrived event callback, implements zero-latency frame waiting via kernel event.
///
/// # Arguments
/// * `d3d_ctx` - D3D11 device context
/// * `target` - Capture target (monitor or window)
pub fn init_capture(
    d3d_ctx: &D3D11Context,
    target: CaptureTarget,
    policy: CapturePolicy,
) -> Result<WGCCapture> {
    // 1. Create GraphicsCaptureItem based on target type
    let item = match target {
        CaptureTarget::Monitor(monitor) => create_capture_item_for_monitor(monitor)?,
        CaptureTarget::Window(hwnd) => create_capture_item_for_window(hwnd)?,
    };

    let size = item.Size()?;

    // 2. Create FramePool format.
    // Sdr: always BGRA8. Hdr: always R16G16B16A16_FLOAT.
    // Auto: follow target monitor HDR state.
    let is_hdr = target_is_hdr(d3d_ctx, target).unwrap_or(false);
    let pixel_format = match (policy, is_hdr) {
        (CapturePolicy::Sdr, _) => DirectXPixelFormat::B8G8R8A8UIntNormalized,
        (CapturePolicy::Hdr, _) => DirectXPixelFormat::R16G16B16A16Float,
        (CapturePolicy::Auto, true) => DirectXPixelFormat::R16G16B16A16Float,
        (CapturePolicy::Auto, false) => DirectXPixelFormat::B8G8R8A8UIntNormalized,
    };
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d_ctx.direct3d_device,
        pixel_format,
        2, // Buffer count
        size,
    )?;

    // 3. Create kernel event (auto-reset, initially non-signaled)
    // SAFETY: CreateEventW creates anonymous event object
    let frame_event =
        unsafe { CreateEventW(None, false, false, None).context("Failed to create frame event")? };

    // 4. Register FrameArrived callback: only SetEvent, no D3D operations
    // Convert HANDLE to usize for closure, bypassing Send restriction.
    // SAFETY: Kernel event handles are thread-safe, SetEvent can be called from any thread.
    let shutting_down = Arc::new(AtomicBool::new(false));
    let shutting_down_cb = Arc::clone(&shutting_down);
    let event_ptr = frame_event.0 as usize;
    let frame_arrived_token = frame_pool.FrameArrived(&TypedEventHandler::<
        Direct3D11CaptureFramePool,
        windows::core::IInspectable,
    >::new(move |_, _| {
        if !shutting_down_cb.load(Ordering::Relaxed) {
            unsafe {
                if SetEvent(HANDLE(event_ptr as *mut _)).is_err() {
                    eprintln!("hdrcapture: SetEvent failed in FrameArrived callback");
                }
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
        frame_arrived_token,
        frame_event,
        shutting_down,
        target_width: size.Width as u32,
        target_height: size.Height as u32,
        target_hdr: is_hdr,
    })
}

fn target_is_hdr(d3d_ctx: &D3D11Context, target: CaptureTarget) -> Result<bool> {
    let target_monitor = match target {
        CaptureTarget::Monitor(hmonitor) => hmonitor,
        CaptureTarget::Window(hwnd) => unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) },
    };
    if target_monitor.is_invalid() {
        return Ok(false);
    }

    let adapter = unsafe { d3d_ctx.dxgi_device.GetAdapter()? };

    let mut i = 0;
    while let Ok(output) = unsafe { adapter.EnumOutputs(i) } {
        let desc = unsafe { output.GetDesc()? };
        if desc.Monitor == target_monitor {
            let output6: IDXGIOutput6 = match output.cast() {
                Ok(v) => v,
                Err(_) => return Ok(false),
            };
            let desc1 = unsafe { output6.GetDesc1()? };
            return Ok(desc1.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
        }
        i += 1;
    }

    Ok(false)
}
