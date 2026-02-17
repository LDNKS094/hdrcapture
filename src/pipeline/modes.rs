use super::*;

const STABLE_FRAME_TIMEOUT_ERR: &str = "Timeout waiting for stable frame after resize";

impl CapturePipeline {
    fn wait_stable_and_process(&mut self, mark_grab_sync: bool) -> Result<CapturedFrame> {
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        let raw = self
            .resolve_frame_after_resize(frame, FIRST_FRAME_TIMEOUT, mark_grab_sync)?
            .ok_or_else(|| anyhow::anyhow!(STABLE_FRAME_TIMEOUT_ERR))?;
        self.process_and_cache(raw)
    }

    fn cached_or_wait_stable(&mut self, mark_grab_sync: bool) -> Result<CapturedFrame> {
        if self.cached_frame.is_some() {
            return self.build_cached_frame();
        }
        self.wait_stable_and_process(mark_grab_sync)
    }

    /// Shared first-call logic for both capture() and grab().
    fn handle_first_call(&mut self, mark_grab_sync: bool) -> Result<CapturedFrame> {
        self.first_call = false;
        let frame = self.hard_wait_frame(FIRST_FRAME_TIMEOUT)?;
        if let Some(result) = self.resolve_or_cache(frame, FRESH_FRAME_TIMEOUT, mark_grab_sync)? {
            return Ok(result);
        }
        self.wait_stable_and_process(mark_grab_sync)
    }

    /// Screenshot mode: capture a fresh frame
    ///
    /// Drain backlog -> wait for DWM to push new frame, guarantees returned frame is generated after the call.
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

        // Timeout - try fallback
        if let Some(fb) = fallback {
            if let Some(result) = self.resolve_or_cache(fb, FRESH_FRAME_TIMEOUT, false)? {
                return Ok(result);
            }
        }

        self.cached_or_wait_stable(false)
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

        // Got a buffered frame - use it
        if let Some(f) = latest {
            if let Some(result) = self.resolve_or_cache(f, FRESH_FRAME_TIMEOUT, true)? {
                return Ok(result);
            }
        }

        // Pool empty - try short wait for new frame
        if let Some(fresh) = self.soft_wait_frame(FRESH_FRAME_TIMEOUT)? {
            if let Some(result) = self.resolve_or_cache(fresh, FRESH_FRAME_TIMEOUT, true)? {
                return Ok(result);
            }
        }

        self.cached_or_wait_stable(true)
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
