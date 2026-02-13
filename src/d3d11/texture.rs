// Texture creation and readback utility functions

use anyhow::{bail, Context, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Returns bytes per pixel for the given DXGI_FORMAT
fn bytes_per_pixel(format: DXGI_FORMAT) -> Result<usize> {
    match format {
        DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(8), // 4 × f16
        DXGI_FORMAT_B8G8R8A8_UNORM => Ok(4),     // 4 × u8
        _ => bail!("Unsupported DXGI_FORMAT: {:?}", format),
    }
}

/// Texture reader: responsible for reading GPU texture data back to CPU
///
/// Staging texture is created on demand and cached for reuse, automatically rebuilt when size/format changes.
/// The returned buffer has RowPitch padding stripped and can be indexed directly by `width * bpp`.
pub struct TextureReader {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging_texture: Option<ID3D11Texture2D>,
    buffer: Vec<u8>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl TextureReader {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        Self {
            device,
            context,
            staging_texture: None,
            buffer: Vec::new(),
            width: 0,
            height: 0,
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
        }
    }

    /// Ensure Staging Texture exists and matches size/format
    pub fn ensure_staging_texture(
        &mut self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<()> {
        if self.staging_texture.is_some()
            && self.width == width
            && self.height == height
            && self.format == format
        {
            return Ok(());
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        // SAFETY: `desc` is fully initialized with valid fields and `self.device` is a live D3D11 device;
        // CreateTexture2D writes to local `texture` only and returns a COM-owned object on success.
        unsafe {
            let mut texture = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .context("Failed to create staging texture")?;

            self.staging_texture = Some(texture.unwrap());
            self.width = width;
            self.height = height;
            self.format = format;
        }

        // Pre-allocate buffer (auto-adjusted on size change, never shrinks)
        let required = width as usize * height as usize * bytes_per_pixel(format)?;
        if self.buffer.len() < required {
            self.buffer.resize(required, 0);
        }

        Ok(())
    }

    /// Read data from GPU texture to CPU
    ///
    /// Returns an owned `Vec<u8>` with RowPitch padding stripped, each row exactly `width * bytes_per_pixel` bytes.
    /// Internally reuses scratch buffer to avoid repeated allocations, performs one memcpy to transfer ownership on return.
    pub fn read_texture(&mut self, source_texture: &ID3D11Texture2D) -> Result<Vec<u8>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            source_texture.GetDesc(&mut desc);
        }

        let bpp = bytes_per_pixel(desc.Format)?;

        self.ensure_staging_texture(desc.Width, desc.Height, desc.Format)?;
        let staging = self.staging_texture.as_ref().unwrap();

        unsafe {
            // GPU → Staging copy
            self.context.CopyResource(staging, source_texture);

            // Map memory for CPU read access
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Failed to map staging texture")?;

            let row_pitch = mapped.RowPitch as usize;
            let row_bytes = desc.Width as usize * bpp;
            let height = desc.Height as usize;

            // Copy row by row to scratch buffer, stripping RowPitch trailing padding
            let src = mapped.pData as *const u8;
            for y in 0..height {
                // SAFETY: src points to mapped GPU memory, row_pitch * y + row_bytes is within mapped range;
                //         self.buffer has been pre-allocated with sufficient space in ensure_staging_texture.
                std::ptr::copy_nonoverlapping(
                    src.add(y * row_pitch),
                    self.buffer.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }

            self.context.Unmap(staging, 0);

            // Transfer ownership: copy from scratch buffer and return
            Ok(self.buffer[..row_bytes * height].to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d11::create_d3d11_device;

    #[test]
    fn test_texture_readback_row_stripped() {
        let d3d_ctx = create_d3d11_device().unwrap();
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // 2x2 R16G16B16A16_FLOAT, all red pixels
        // f16: 1.0 = 0x3C00, 0.0 = 0x0000
        let pixel_red: [u16; 4] = [0x3C00, 0x0000, 0x0000, 0x3C00];
        let mut init_data = Vec::new();
        for _ in 0..4 {
            init_data.extend_from_slice(&pixel_red);
        }

        let init_bytes: Vec<u8> = init_data.iter().flat_map(|v| v.to_ne_bytes()).collect();

        let desc = D3D11_TEXTURE2D_DESC {
            Width: 2,
            Height: 2,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let subresource_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: init_bytes.as_ptr() as *const _,
            SysMemPitch: 16, // 2 pixels × 8 bytes
            SysMemSlicePitch: 0,
        };

        unsafe {
            let mut texture = None;
            d3d_ctx
                .device
                .CreateTexture2D(&desc, Some(&subresource_data), Some(&mut texture))
                .unwrap();
            let texture = texture.unwrap();

            let data = reader.read_texture(&texture).unwrap();

            // After stripping padding, data size should be exactly 2 × 2 × 8 = 32 bytes
            assert_eq!(data.len(), 32, "Stripped buffer should be exactly 32 bytes");

            // Verify first pixel
            let u16_data = std::slice::from_raw_parts(data.as_ptr() as *const u16, data.len() / 2);
            assert_eq!(u16_data[0], 0x3C00); // R
            assert_eq!(u16_data[1], 0x0000); // G
            assert_eq!(u16_data[2], 0x0000); // B
            assert_eq!(u16_data[3], 0x3C00); // A

            // Verify first pixel of second row (offset = row_bytes = 16 bytes = 8 u16)
            assert_eq!(u16_data[8], 0x3C00); // R
            assert_eq!(u16_data[9], 0x0000); // G
        }
    }
}
