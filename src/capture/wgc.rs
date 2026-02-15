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
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_BOX};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::IDXGIOutput6;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic};

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
    /// Current pool size (updated on Recreate)
    pool_width: u32,
    pool_height: u32,
    /// Whether the target monitor has HDR enabled (detected once at init)
    target_hdr: bool,
    /// Window handle for client area cropping (None for monitor capture)
    window_handle: Option<HWND>,
    /// Stored for frame pool Recreate()
    direct3d_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice,
    pixel_format: DirectXPixelFormat,
}

impl WGCCapture {
    /// Start capture
    pub fn start(&self) -> Result<()> {
        self.session.StartCapture()?;
        Ok(())
    }

    /// Current pool size (may change after resize detection)
    pub fn pool_size(&self) -> (u32, u32) {
        (self.pool_width, self.pool_height)
    }

    /// Check if the frame's content size differs from the pool size.
    /// If so, recreate the frame pool. Call once per frame before extracting the texture.
    pub fn check_resize(&mut self, frame: &Direct3D11CaptureFrame) -> Result<()> {
        let content_size = frame.ContentSize()?;
        let new_w = content_size.Width as u32;
        let new_h = content_size.Height as u32;

        if new_w != self.pool_width || new_h != self.pool_height {
            self.frame_pool.Recreate(
                &self.direct3d_device,
                self.pixel_format,
                2,
                content_size,
            )?;
            self.pool_width = new_w;
            self.pool_height = new_h;
        }
        Ok(())
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

    /// Compute the client area crop box for window capture.
    ///
    /// Returns `Some(D3D11_BOX)` describing the client area region within the
    /// captured texture, or `None` for monitor capture / minimized windows /
    /// if the calculation fails.
    ///
    /// Uses the OBS approach: `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`
    /// for the actual window rect (excludes invisible shadow padding), then
    /// `ClientToScreen` to find the client area offset within that rect.
    pub fn get_client_box(&self, texture_width: u32, texture_height: u32) -> Option<D3D11_BOX> {
        let hwnd = self.window_handle?;

        // SAFETY: Win32 API calls with valid HWND. IsIconic, GetClientRect,
        // DwmGetWindowAttribute, ClientToScreen all read window state atomically.
        unsafe {
            // Skip if minimized (check twice, ABA unlikely)
            if IsIconic(hwnd).as_bool() {
                return None;
            }

            let mut client_rect = RECT::default();
            if GetClientRect(hwnd, &mut client_rect).is_err() {
                return None;
            }

            if IsIconic(hwnd).as_bool() {
                return None;
            }

            if client_rect.right <= 0 || client_rect.bottom <= 0 {
                return None;
            }

            let mut window_rect = RECT::default();
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut window_rect as *mut _ as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_err()
            {
                return None;
            }

            let mut upper_left = POINT { x: 0, y: 0 };
            if !windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut upper_left).as_bool() {
                return None;
            }

            let left = if upper_left.x > window_rect.left {
                (upper_left.x - window_rect.left) as u32
            } else {
                0
            };

            let top = if upper_left.y > window_rect.top {
                (upper_left.y - window_rect.top) as u32
            } else {
                0
            };

            let texture_w = if texture_width > left {
                (texture_width - left).min(client_rect.right as u32)
            } else {
                1
            };

            let texture_h = if texture_height > top {
                (texture_height - top).min(client_rect.bottom as u32)
            } else {
                1
            };

            let right = left + texture_w;
            let bottom = top + texture_h;

            // Validate box fits within texture
            if right > texture_width || bottom > texture_height {
                return None;
            }

            Some(D3D11_BOX {
                left,
                top,
                front: 0,
                right,
                bottom,
                back: 1,
            })
        }
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

    let window_handle = match target {
        CaptureTarget::Window(hwnd) => Some(hwnd),
        CaptureTarget::Monitor(_) => None,
    };

    Ok(WGCCapture {
        _item: item,
        frame_pool,
        session,
        frame_arrived_token,
        frame_event,
        shutting_down,
        pool_width: size.Width as u32,
        pool_height: size.Height as u32,
        target_hdr: is_hdr,
        window_handle,
        direct3d_device: d3d_ctx.direct3d_device.clone(),
        pixel_format,
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
