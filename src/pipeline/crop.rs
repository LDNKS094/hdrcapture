use super::*;

impl CapturePipeline {
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
    pub(super) fn read_raw_frame(
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
}
