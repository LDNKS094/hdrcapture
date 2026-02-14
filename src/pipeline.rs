// Capture pipeline: target resolution → WGC initialization → frame capture → texture readback
//
// Provides two frame retrieval modes:
// - capture(): drain backlog and wait for fresh frame, suitable for screenshots (guarantees frame is generated after call)
// - grab(): drain backlog and take last frame, suitable for continuous capture (lower latency)
// Frame lifetime covers CopyResource, ensuring DWM won't overwrite the surface being read.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
};

use crate::capture::wgc::{CaptureTarget, WGCCapture};
pub use crate::capture::CapturePolicy;
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::color::white_level;
use crate::color::{self, ColorFrame, ColorPixelFormat, ToneMapPass};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};
use crate::memory::ElasticBufferPool;

/// First frame wait timeout
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// Short timeout for waiting for new frame (~3 VSyncs at 60Hz)
/// When screen is active, new frame arrives within 1 VSync; timeout indicates static screen, fallback should be used.
const FRESH_FRAME_TIMEOUT: Duration = Duration::from_millis(50);

/// Single frame capture result
#[derive(Clone)]
pub struct CapturedFrame {
    /// Pixel data (shared, read-only), length = width * height * bytes_per_pixel
    pub data: Arc<SharedFrameData>,
    /// Frame width (pixels)
    pub width: u32,
    /// Frame height (pixels)
    pub height: u32,
    /// Frame timestamp (seconds), relative to system boot time (QPC)
    pub timestamp: f64,
    /// Pixel format of `data`
    pub format: ColorPixelFormat,
}

impl CapturedFrame {
    pub fn bytes_per_pixel(&self) -> usize {
        match self.format {
            ColorPixelFormat::Bgra8 => 4,
            ColorPixelFormat::Rgba16f => 8,
        }
    }

    /// Save frame to file.
    ///
    /// Format is determined by file extension:
    /// - `.png` `.bmp` `.jpg` `.tiff` — standard formats (BGRA8 only)
    /// - `.jxr` — JPEG XR (both BGRA8 and RGBA16F)
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        crate::image::save(
            path.as_ref(),
            self.data.as_slice(),
            self.width,
            self.height,
            self.format,
        )
    }
}

pub struct SharedFrameData {
    bytes: Vec<u8>,
    pool: Arc<ElasticBufferPool>,
    group_idx: usize,
}

impl SharedFrameData {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl std::ops::Deref for SharedFrameData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl Drop for SharedFrameData {
    fn drop(&mut self) {
        let bytes = std::mem::take(&mut self.bytes);
        self.pool.release_recycled(self.group_idx, bytes);
    }
}

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
}

struct RawFrame {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    timestamp: f64,
    format: ColorPixelFormat,
}

impl CapturePipeline {
    fn frame_bytes(width: u32, height: u32, format: ColorPixelFormat) -> usize {
        let bpp = match format {
            ColorPixelFormat::Bgra8 => 4,
            ColorPixelFormat::Rgba16f => 8,
        };
        width as usize * height as usize * bpp
    }

    fn color_format(format: DXGI_FORMAT) -> Result<ColorPixelFormat> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Ok(ColorPixelFormat::Bgra8),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(ColorPixelFormat::Rgba16f),
            _ => bail!("Unsupported DXGI_FORMAT for color pipeline: {:?}", format),
        }
    }

    /// Create capture pipeline by monitor index
    ///
    /// Indices are ordered by system enumeration, not guaranteed that `0` is the primary monitor.
    pub fn monitor(index: usize, policy: CapturePolicy) -> Result<Self> {
        enable_dpi_awareness();
        let hmonitor = find_monitor(index)?;
        let sdr_white_nits = white_level::query_sdr_white_level(hmonitor);
        Self::new(CaptureTarget::Monitor(hmonitor), policy, sdr_white_nits)
    }

    /// Create window capture pipeline by process name
    ///
    /// `index` is the window index for processes with the same name, defaults to 0 (first matching window).
    pub fn window(process_name: &str, index: Option<usize>, policy: CapturePolicy) -> Result<Self> {
        enable_dpi_awareness();
        let hwnd = find_window(process_name, index)?;
        let hmonitor = unsafe {
            windows::Win32::Graphics::Gdi::MonitorFromWindow(
                hwnd,
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            )
        };
        let sdr_white_nits = white_level::query_sdr_white_level(hmonitor);
        Self::new(CaptureTarget::Window(hwnd), policy, sdr_white_nits)
    }

    fn new(target: CaptureTarget, policy: CapturePolicy, sdr_white_nits: f32) -> Result<Self> {
        let d3d_ctx = create_d3d11_device()?;
        let capture = init_capture(&d3d_ctx, target, policy)?;
        let target_hdr = capture.is_hdr();
        capture.start()?;
        // Create reader after start() to let DWM start preparing first frame as early as possible
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // Pre-create Staging Texture to avoid ~11ms creation overhead on first frame readback.
        // Hdr mode outputs R16G16B16A16_FLOAT (8 bpp); Auto/Sdr output BGRA8 (4 bpp).
        let (w, h) = capture.target_size();
        let (staging_format, bpp) = if policy == CapturePolicy::Hdr {
            (DXGI_FORMAT_R16G16B16A16_FLOAT, 8)
        } else {
            (DXGI_FORMAT_B8G8R8A8_UNORM, 4)
        };
        reader.ensure_staging_texture(w, h, staging_format)?;
        let output_frame_bytes = w as usize * h as usize * bpp;
        let output_pool = ElasticBufferPool::new(output_frame_bytes);

        // Create tone-map pass only for Auto (may need HDR→SDR conversion)
        let tone_map_pass = if policy == CapturePolicy::Auto {
            Some(ToneMapPass::new(&d3d_ctx.device, &d3d_ctx.context)?)
        } else {
            None
        };

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            policy,
            capture,
            reader,
            output_pool,
            output_frame_bytes,
            first_call: true,
            cached_frame: None,
            tone_map_pass,
            sdr_white_nits,
            target_hdr,
        })
    }

    /// Wait for the next frame from the pool, with timeout.
    /// Returns None on timeout instead of error.
    fn try_wait_frame(
        &self,
        timeout: Duration,
    ) -> Result<Option<windows::Graphics::Capture::Direct3D11CaptureFrame>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(f) = self.capture.try_get_next_frame() {
                return Ok(Some(f));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let timeout_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
            if self.capture.wait_for_frame(timeout_ms).is_err() {
                return Ok(None);
            }
        }
    }

    /// Wait for the next frame, returning error on timeout.
    fn wait_frame(
        &self,
        timeout: Duration,
    ) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        self.try_wait_frame(timeout)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Timeout waiting for capture frame ({}ms)",
                timeout.as_millis()
            )
        })
    }

    /// Extract texture and metadata from WGC frame.
    fn read_raw_frame(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    ) -> Result<RawFrame> {
        // Extract timestamp (SystemRelativeTime, 100ns precision, converted to seconds)
        let timestamp = frame.SystemRelativeTime()?.Duration as f64 / 10_000_000.0;

        let texture = WGCCapture::frame_to_texture(frame)?;

        let (width, height, format) = unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            (desc.Width, desc.Height, desc.Format)
        };
        let color_format = Self::color_format(format)?;

        Ok(RawFrame {
            texture,
            width,
            height,
            timestamp,
            format: color_format,
        })
    }

    /// Run color pipeline once and cache the final output for fallback.
    fn process_and_cache(&mut self, raw: RawFrame) -> Result<CapturedFrame> {
        let processed = color::process_frame(
            ColorFrame {
                texture: raw.texture,
                width: raw.width,
                height: raw.height,
                timestamp: raw.timestamp,
                format: raw.format,
            },
            self.policy,
            self.tone_map_pass.as_mut(),
            self.sdr_white_nits,
        )?;

        let ColorFrame {
            texture,
            width,
            height,
            timestamp,
            format,
        } = processed;
        let required_len = Self::frame_bytes(width, height, format);

        // Rebuild output pool when processed frame size grows (e.g. format/resolution change).
        // Existing published frames keep old pool alive via Arc and are recycled independently.
        if required_len > self.output_frame_bytes {
            self.output_frame_bytes = required_len;
            self.output_pool = ElasticBufferPool::new(self.output_frame_bytes);
        }

        let mut pooled = self.output_pool.acquire();
        let written = self
            .reader
            .read_texture_into(&texture, pooled.as_mut_slice())?;
        let (mut dst_vec, group_idx, pool) = pooled.into_parts();
        dst_vec.truncate(written);

        let output = CapturedFrame {
            data: Arc::new(SharedFrameData {
                bytes: dst_vec,
                pool,
                group_idx,
            }),
            width,
            height,
            timestamp,
            format,
        };
        self.cached_frame = Some(output.clone());
        Ok(output)
    }

    /// Build a CapturedFrame from the cached processed output.
    /// Only called on the fallback path (static screen, no new frames available).
    fn build_cached_frame(&self) -> Result<CapturedFrame> {
        self.cached_frame
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No cached frame data available"))
    }

    /// Screenshot mode: capture a fresh frame
    ///
    /// Drain backlog → wait for DWM to push new frame, guarantees returned frame is generated after the call.
    /// Skip drain on first call (first frame is naturally fresh).
    /// Use fallback when screen is static to avoid long blocking.
    ///
    /// Suitable for screenshot scenarios, latency ~1 VSync.
    pub fn capture(&mut self) -> Result<CapturedFrame> {
        // First call: first frame after StartCapture() is naturally fresh, just capture it
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
            let raw = self.read_raw_frame(&frame)?;
            return self.process_and_cache(raw);
        }

        // Drain pool, keep last frame as fallback
        let mut fallback = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            fallback = Some(f);
        }

        // Try to get a fresh frame with short timeout
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
            // New frame arrived — use it (discard fallback if any)
            let raw = self.read_raw_frame(&fresh)?;
            return self.process_and_cache(raw);
        }

        // Timeout — screen is likely static
        if let Some(fb) = fallback {
            // Use the last drained frame (most recent available)
            let raw = self.read_raw_frame(&fb)?;
            return self.process_and_cache(raw);
        }

        // Pool was empty AND no new frame — use cached data
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }

        // No cache either — true cold start edge case, full timeout
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self.read_raw_frame(&frame)?;
        self.process_and_cache(raw)
    }

    /// Continuous capture mode: grab latest available frame
    ///
    /// Drain backlog, keep last frame; wait for new frame when pool is empty.
    /// Returned frame may have been generated before the call, but with lower latency.
    /// Use fallback when screen is static to avoid long blocking.
    ///
    /// Suitable for high-frequency continuous capture scenarios.
    pub fn grab(&mut self) -> Result<CapturedFrame> {
        // First call: directly capture first frame
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
            let raw = self.read_raw_frame(&frame)?;
            return self.process_and_cache(raw);
        }

        // Drain pool, keep last frame
        let mut latest = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            latest = Some(f);
        }

        // Got a buffered frame — use it
        if let Some(f) = latest {
            let raw = self.read_raw_frame(&f)?;
            return self.process_and_cache(raw);
        }

        // Pool empty — try short wait for new frame
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
            let raw = self.read_raw_frame(&fresh)?;
            return self.process_and_cache(raw);
        }

        // Timeout — screen is likely static, use cached data
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }

        // No cache — full timeout (should not happen in normal usage)
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self.read_raw_frame(&frame)?;
        self.process_and_cache(raw)
    }

    /// Whether the target monitor has HDR enabled.
    pub fn is_hdr(&self) -> bool {
        self.target_hdr
    }
}
