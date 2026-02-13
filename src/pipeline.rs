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
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
};

use crate::capture::wgc::{CaptureTarget, WGCCapture};
pub use crate::capture::CapturePolicy;
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::color::{self, ColorFrame, ColorPixelFormat};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};
use crate::memory::{ElasticBufferPool, PooledBuffer};

/// One-shot capture source (high-level input before OS handle resolution).
pub enum CaptureSource<'a> {
    Monitor(usize),
    Window {
        process_name: &'a str,
        window_index: Option<usize>,
    },
}

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
}

impl CapturedFrame {
    /// Save as PNG file (fast compression)
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let file = std::fs::File::create(path.as_ref())?;
        let writer = std::io::BufWriter::new(file);
        let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);

        // BGRA → RGBA
        let mut rgba = self.data.as_slice().to_vec();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        encoder.write_image(&rgba, self.width, self.height, ExtendedColorType::Rgba8)?;
        Ok(())
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
    _policy: CapturePolicy,
    capture: WGCCapture,
    reader: TextureReader,
    output_pool: Arc<ElasticBufferPool>,
    /// First call flag. First frame after StartCapture() is naturally fresh,
    /// no drain-discard needed, direct capture saves ~1 VSync.
    first_call: bool,
    /// Last successful processed frame, for static-screen fallback.
    cached_frame: Option<CapturedFrame>,
}

struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: f64,
    format: ColorPixelFormat,
}

impl CapturePipeline {
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
        Self::new(CaptureTarget::Monitor(hmonitor), policy)
    }

    /// Create window capture pipeline by process name
    ///
    /// `index` is the window index for processes with the same name, defaults to 0 (first matching window).
    pub fn window(process_name: &str, index: Option<usize>, policy: CapturePolicy) -> Result<Self> {
        enable_dpi_awareness();
        let hwnd = find_window(process_name, index)?;
        Self::new(CaptureTarget::Window(hwnd), policy)
    }

    fn new(target: CaptureTarget, policy: CapturePolicy) -> Result<Self> {
        let d3d_ctx = create_d3d11_device()?;
        let capture = init_capture(&d3d_ctx, target, policy)?;
        capture.start()?;
        // Create reader after start() to let DWM start preparing first frame as early as possible
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // Pre-create Staging Texture to avoid ~11ms creation overhead on first frame readback
        let (w, h) = capture.target_size();
        reader.ensure_staging_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        let output_pool = ElasticBufferPool::new(w as usize * h as usize * 4);

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            _policy: policy,
            capture,
            reader,
            output_pool,
            first_call: true,
            cached_frame: None,
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

    /// Extract texture from WGC frame and read back raw pixel data.
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

        let data = self.reader.read_texture(&texture)?;

        Ok(RawFrame {
            data,
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
                data: raw.data,
                width: raw.width,
                height: raw.height,
                timestamp: raw.timestamp,
                format: raw.format,
            },
            self._policy,
        )?;

        let ColorFrame {
            data: src,
            width,
            height,
            timestamp,
            format: _,
        } = processed;
        let src_len = src.len();
        let pooled = self.output_pool.acquire();
        let (mut dst_vec, group_idx, pool) = Self::write_into_pooled_buffer(pooled, src, src_len)?;
        dst_vec.truncate(src_len);

        let output = CapturedFrame {
            data: Arc::new(SharedFrameData {
                bytes: dst_vec,
                pool,
                group_idx,
            }),
            width,
            height,
            timestamp,
        };
        self.cached_frame = Some(output.clone());
        Ok(output)
    }

    fn write_into_pooled_buffer(
        mut pooled: PooledBuffer,
        src: Vec<u8>,
        src_len: usize,
    ) -> Result<(Vec<u8>, usize, Arc<ElasticBufferPool>)> {
        let dst = pooled.as_mut_slice();
        if dst.len() < src_len {
            bail!(
                "Output pool frame too small: dst={}, src={}",
                dst.len(),
                src_len
            );
        }
        dst[..src_len].copy_from_slice(&src);
        Ok(pooled.into_parts())
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
}

/// One-liner screenshot: create pipeline → capture frame → return
///
/// Suitable for scenarios where only one screenshot is needed. Internally creates and destroys pipeline,
/// cold start ~79ms (includes D3D11 device creation + WGC session creation + first frame wait).
///
/// For multiple screenshots, use `CapturePipeline` to reuse the pipeline.
pub fn screenshot(source: CaptureSource<'_>, policy: CapturePolicy) -> Result<CapturedFrame> {
    let mut pipeline = match source {
        CaptureSource::Monitor(index) => CapturePipeline::monitor(index, policy)?,
        CaptureSource::Window {
            process_name,
            window_index,
        } => CapturePipeline::window(process_name, window_index, policy)?,
    };
    pipeline.capture()
}
