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

mod types;
mod build;
mod frame_sync;
mod crop;

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

impl CapturePipeline {
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

    /// Try to resolve a frame (handling resize), process it, or fall back to cache.
    /// Returns None only when neither resolve nor cache succeeds.
    fn resolve_or_cache(
        &mut self,
        frame: windows::Graphics::Capture::Direct3D11CaptureFrame,
        timeout: Duration,
        mark_grab_sync: bool,
    ) -> Result<Option<CapturedFrame>> {
        if let Some(raw) = self.resolve_frame_after_resize(frame, timeout, mark_grab_sync)? {
            return self.process_and_cache(raw).map(Some);
        }
        if self.cached_frame.is_some() {
            return self.build_cached_frame().map(Some);
        }
        Ok(None)
    }

    /// Shared first-call logic for both capture() and grab().
    fn handle_first_call(&mut self, mark_grab_sync: bool) -> Result<CapturedFrame> {
        self.first_call = false;
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        if let Some(result) = self.resolve_or_cache(frame, FRESH_FRAME_TIMEOUT, mark_grab_sync)? {
            return Ok(result);
        }
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self
            .resolve_frame_after_resize(frame, FIRST_FRAME_TIMEOUT, mark_grab_sync)?
            .ok_or_else(|| anyhow::anyhow!("Timeout waiting for stable frame after resize"))?;
        self.process_and_cache(raw)
    }

    /// Screenshot mode: capture a fresh frame
    ///
    /// Drain backlog → wait for DWM to push new frame, guarantees returned frame is generated after the call.
    /// Skip drain on first call (first frame is naturally fresh).
    /// Use fallback when screen is static to avoid long blocking.
    ///
    /// Suitable for screenshot scenarios, latency ~1 VSync.
    pub fn capture(&mut self) -> Result<CapturedFrame> {
        if self.first_call {
            return self.handle_first_call(false);
        }

        // Drain pool, keep last frame as fallback
        let mut fallback = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            fallback = Some(f);
        }

        // Try to get a fresh frame with short timeout
        if let Some(fresh) = self.soft_wait_frame(FRESH_FRAME_TIMEOUT)? {
            if let Some(result) = self.resolve_or_cache(fresh, FRESH_FRAME_TIMEOUT, false)? {
                return Ok(result);
            }
        }

        // Timeout — try fallback
        if let Some(fb) = fallback {
            if let Some(result) = self.resolve_or_cache(fb, FRESH_FRAME_TIMEOUT, false)? {
                return Ok(result);
            }
        }

        // Use cached data
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }

        // No cache — full timeout
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self
            .resolve_frame_after_resize(frame, FIRST_FRAME_TIMEOUT, false)?
            .ok_or_else(|| anyhow::anyhow!("Timeout waiting for stable frame after resize"))?;
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
        // If previous resize was observed in grab path, force one fresh-sync call
        // before consuming backlog frames again.
        if self.force_fresh {
            self.force_fresh = false;

            if let Some(fresh) = self.soft_wait_frame(FRESH_FRAME_TIMEOUT)? {
                if let Some(result) = self.resolve_or_cache(fresh, FRESH_FRAME_TIMEOUT, true)? {
                    return Ok(result);
                }
            }

            if self.cached_frame.is_some() {
                return self.build_cached_frame();
            }
        }

        if self.first_call {
            return self.handle_first_call(true);
        }

        // Drain pool, keep last frame
        let mut latest = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            latest = Some(f);
        }

        // Got a buffered frame — use it
        if let Some(f) = latest {
            if let Some(result) = self.resolve_or_cache(f, FRESH_FRAME_TIMEOUT, true)? {
                return Ok(result);
            }
        }

        // Pool empty — try short wait for new frame
        if let Some(fresh) = self.soft_wait_frame(FRESH_FRAME_TIMEOUT)? {
            if let Some(result) = self.resolve_or_cache(fresh, FRESH_FRAME_TIMEOUT, true)? {
                return Ok(result);
            }
        }

        // Timeout — use cached data
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }

        // No cache — full timeout
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self
            .resolve_frame_after_resize(frame, FIRST_FRAME_TIMEOUT, true)?
            .ok_or_else(|| anyhow::anyhow!("Timeout waiting for stable frame after resize"))?;
        self.process_and_cache(raw)
    }

    /// Whether the target monitor has HDR enabled.
    pub fn is_hdr(&self) -> bool {
        self.target_hdr
    }

    /// Buffer pool statistics (for diagnostics / benchmarks).
    pub fn pool_stats(&self) -> crate::memory::PoolStats {
        self.output_pool.stats()
    }
}
