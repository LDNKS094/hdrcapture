use super::*;

impl CapturePipeline {
    /// Run color pipeline once and cache the final output for fallback.
    pub(super) fn process_and_cache(&mut self, raw: RawFrame) -> Result<CapturedFrame> {
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
    pub(super) fn build_cached_frame(&self) -> Result<CapturedFrame> {
        self.cached_frame
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No cached frame data available"))
    }

    /// Try to resolve a frame (handling resize), process it, or fall back to cache.
    /// Returns None only when neither resolve nor cache succeeds.
    pub(super) fn resolve_or_cache(
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
}
