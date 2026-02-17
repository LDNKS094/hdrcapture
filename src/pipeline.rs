// Capture pipeline: target resolution → WGC initialization → frame capture → texture readback
//
// Provides two frame retrieval modes:
// - capture(): drain backlog and wait for fresh frame, suitable for screenshots (guarantees frame is generated after call)
// - grab(): drain backlog and take last frame, suitable for continuous capture (lower latency)
// Frame lifetime covers CopyResource, ensuring DWM won't overwrite the surface being read.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
};

use crate::capture::wgc::{CaptureTarget, WGCCapture, WindowGeometry};
pub use crate::capture::CapturePolicy;
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::color::white_level;
use crate::color::{self, ColorFrame, ColorPixelFormat, ToneMapPass};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};
use crate::memory::ElasticBufferPool;

mod build;
mod crop;
mod frame_sync;
mod modes;
mod process;
mod types;

pub use types::{CapturedFrame, SharedFrameData};
use types::{CropCache, RawFrame};

/// First frame wait timeout
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// Short timeout for waiting for new frame (~3 VSyncs at 60Hz)
/// When screen is active, new frame arrives within 1 VSync; timeout indicates static screen, fallback should be used.
const FRESH_FRAME_TIMEOUT: Duration = Duration::from_millis(50);

/// Maximum retries when resize keeps changing during transition.
const RESIZE_RETRY_LIMIT: usize = 3;

/// Capture pipeline
///
/// Wraps D3D11 device, WGC capture session, and texture reader, providing one-liner screenshot capability.
///
/// # Examples
/// ```no_run
/// # use hdrcapture::pipeline::{CapturePipeline, CapturePolicy};
/// let mut pipeline = CapturePipeline::monitor(0, CapturePolicy::Auto).unwrap();
/// let frame = pipeline.capture().unwrap();
/// println!("{}x{}, {} bytes", frame.width, frame.height, frame.data.len());
/// ```
pub struct CapturePipeline {
    _d3d_ctx: D3D11Context,
    policy: CapturePolicy,
    capture: WGCCapture,
    reader: TextureReader,
    output_pool: Arc<ElasticBufferPool>,
    output_frame_bytes: usize,
    /// First call flag. First frame after StartCapture() is naturally fresh,
    /// no drain-discard needed, direct capture saves ~1 VSync.
    first_call: bool,
    /// Last successful processed frame, for static-screen fallback.
    cached_frame: Option<CapturedFrame>,
    /// GPU tone-map pass (Some when Auto policy may produce Rgba16f).
    tone_map_pass: Option<ToneMapPass>,
    /// SDR white level in nits, queried at pipeline creation.
    sdr_white_nits: f32,
    /// Whether the target monitor has HDR enabled (detected once at init).
    target_hdr: bool,
    /// Crop to client area in window capture (remove title bar / borders).
    headless: bool,
    /// Cached crop texture for client area cropping (window capture only).
    /// Rebuilt when dimensions or format change.
    crop_texture: Option<CropCache>,
    /// One-shot guard for grab(): when resize is observed, force next call to
    /// wait for a fresh frame before using backlog frames.
    force_fresh: bool,
    /// Prevent Send + Sync: pipeline holds thread-affine COM resources
    /// (ID3D11DeviceContext) that must not cross thread boundaries.
    _not_send_sync: PhantomData<*const ()>,
}
