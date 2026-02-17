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
    /// Check if frame pool needs recreation due to size change.
    ///
    /// For window targets, uses pre-queried geometry when available to avoid
    /// redundant Win32 API calls. For monitor targets, uses frame ContentSize.
    fn needs_recreate(
        &self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
        geometry: Option<&WindowGeometry>,
    ) -> Result<Option<(u32, u32)>> {
        if self.capture.is_window_target() {
            if let Some(geo) = geometry {
                let (pool_w, pool_h) = self.capture.pool_size();
                if geo.frame_width != pool_w || geo.frame_height != pool_h {
                    return Ok(Some((geo.frame_width, geo.frame_height)));
                }
            }
            return Ok(None);
        }

        let content_size = frame.ContentSize()?;
        let new_w = content_size.Width as u32;
        let new_h = content_size.Height as u32;

        if new_w == 0 || new_h == 0 {
            return Ok(None);
        }

        let (pool_w, pool_h) = self.capture.pool_size();
        if new_w != pool_w || new_h != pool_h {
            return Ok(Some((new_w, new_h)));
        }

        Ok(None)
    }

    fn resolve_frame_after_resize(
        &mut self,
        frame: windows::Graphics::Capture::Direct3D11CaptureFrame,
        timeout: Duration,
        mark_grab_sync: bool,
    ) -> Result<Option<RawFrame>> {
        let mut current = frame;
        let mut drop_next = false;

        for _ in 0..RESIZE_RETRY_LIMIT {
            // Query window geometry once per iteration (used for both resize check and crop).
            let (pool_w, pool_h) = self.capture.pool_size();
            let geometry = self.capture.window_geometry(pool_w, pool_h);

            if let Some((new_w, new_h)) = self.needs_recreate(&current, geometry.as_ref())? {
                if mark_grab_sync {
                    self.force_fresh = true;
                }
                self.capture.recreate_frame_pool(new_w, new_h)?;
                // Drop the first frame after recreate to avoid stale content.
                drop_next = true;

                if let Some(next) = self.try_wait_frame(timeout)? {
                    current = next;
                    continue;
                }

                return Ok(None);
            }

            // Post-recreate: skip this frame (likely stale), fetch next.
            if drop_next {
                drop_next = false;
                if let Some(next) = self.try_wait_frame(timeout)? {
                    current = next;
                    continue;
                }
                return Ok(None);
            }

            let client_box = if self.headless {
                geometry.and_then(|g| g.client_box)
            } else {
                None
            };
            return self.read_raw_frame(&current, client_box).map(Some);
        }

        Ok(None)
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

    /// Ensure a crop texture exists with the given dimensions and format.
    /// Reuses the cached texture if dimensions and format match.
    fn ensure_crop_texture(
        &mut self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11Texture2D> {
        if let Some(ref cache) = self.crop_texture {
            if cache.width == width && cache.height == height && cache.format == format {
                return Ok(cache.texture.clone());
            }
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        // SAFETY: desc is fully initialized; CreateTexture2D allocates a GPU resource.
        let texture = unsafe {
            let mut tex = None;
            self._d3d_ctx
                .device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .context("Failed to create crop texture")?;
            tex.unwrap()
        };

        self.crop_texture = Some(CropCache {
            texture: texture.clone(),
            width,
            height,
            format,
        });

        Ok(texture)
    }

    /// Extract texture and metadata from WGC frame.
    ///
    /// For window capture, crops to client area (removes title bar and borders)
    /// using `CopySubresourceRegion` on the GPU.
    /// `client_box` is pre-computed from `window_geometry()` to avoid redundant Win32 queries.
    fn read_raw_frame(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
        client_box: Option<D3D11_BOX>,
    ) -> Result<RawFrame> {
        let timestamp = frame.SystemRelativeTime()?.Duration as f64 / 10_000_000.0;

        let source_texture = WGCCapture::frame_to_texture(frame)?;

        let (src_width, src_height, format) = unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source_texture.GetDesc(&mut desc);
            (desc.Width, desc.Height, desc.Format)
        };
        let color_format = Self::color_format(format)?;

        // For window capture: crop to client area (remove title bar / borders)
        if let Some(client_box) = client_box {
            let crop_w = client_box.right - client_box.left;
            let crop_h = client_box.bottom - client_box.top;

            let cropped = self.ensure_crop_texture(crop_w, crop_h, format)?;

            // SAFETY: Both textures are valid D3D11 resources with compatible formats.
            // CopySubresourceRegion copies the client_box region from source to (0,0) of dest.
            unsafe {
                self._d3d_ctx.context.CopySubresourceRegion(
                    &cropped,
                    0,
                    0,
                    0,
                    0,
                    &source_texture,
                    0,
                    Some(&client_box),
                );
            }

            return Ok(RawFrame {
                texture: cropped,
                width: crop_w,
                height: crop_h,
                timestamp,
                format: color_format,
            });
        }

        Ok(RawFrame {
            texture: source_texture,
            width: src_width,
            height: src_height,
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
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
        if let Some(result) = self.resolve_or_cache(frame, FRESH_FRAME_TIMEOUT, mark_grab_sync)? {
            return Ok(result);
        }
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
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
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
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
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
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

            if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
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
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
            if let Some(result) = self.resolve_or_cache(fresh, FRESH_FRAME_TIMEOUT, true)? {
                return Ok(result);
            }
        }

        // Timeout — use cached data
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }

        // No cache — full timeout
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
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
