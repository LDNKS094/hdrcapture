use super::*;

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

    pub(super) fn resolve_frame_after_resize(
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

                if let Some(next) = self.soft_wait_frame(timeout)? {
                    current = next;
                    continue;
                }

                return Ok(None);
            }

            // Post-recreate: skip this frame (likely stale), fetch next.
            if drop_next {
                drop_next = false;
                if let Some(next) = self.soft_wait_frame(timeout)? {
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
    pub(super) fn soft_wait_frame(
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
    pub(super) fn hard_wait_frame(
        &self,
        timeout: Duration,
    ) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        self.soft_wait_frame(timeout)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Timeout waiting for capture frame ({}ms)",
                timeout.as_millis()
            )
        })
    }
}
