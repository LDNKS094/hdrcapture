// 纹理创建与回读工具函数

use anyhow::{bail, Context, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// 根据 DXGI_FORMAT 返回每像素字节数
fn bytes_per_pixel(format: DXGI_FORMAT) -> Result<usize> {
    match format {
        DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(8), // 4 × f16
        DXGI_FORMAT_B8G8R8A8_UNORM => Ok(4),     // 4 × u8
        _ => bail!("Unsupported DXGI_FORMAT: {:?}", format),
    }
}

/// 纹理读取器：负责将 GPU 纹理数据回读到 CPU
///
/// Staging texture 按需创建并缓存复用，尺寸/格式变化时自动重建。
/// 返回的 buffer 已剥离 RowPitch 填充，可直接按 `width * bpp` 索引。
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

    /// 确保 Staging Texture 存在且尺寸/格式匹配
    fn ensure_staging_texture(
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

        // 预分配 buffer（尺寸变化时自动调整，不缩小）
        let required = width as usize * height as usize * bytes_per_pixel(format)?;
        if self.buffer.len() < required {
            self.buffer.resize(required, 0);
        }

        Ok(())
    }

    /// 从 GPU 纹理读取数据到 CPU
    ///
    /// 返回拥有所有权的 `Vec<u8>`，已剥离 RowPitch 填充，每行恰好 `width * bytes_per_pixel` 字节。
    /// 内部复用 scratch buffer 避免重复分配，返回时执行一次 memcpy 交付所有权。
    pub fn read_texture(&mut self, source_texture: &ID3D11Texture2D) -> Result<Vec<u8>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            source_texture.GetDesc(&mut desc);
        }

        let bpp = bytes_per_pixel(desc.Format)?;

        self.ensure_staging_texture(desc.Width, desc.Height, desc.Format)?;
        let staging = self.staging_texture.as_ref().unwrap();

        unsafe {
            // GPU → Staging 拷贝
            self.context.CopyResource(staging, source_texture);

            // 映射内存供 CPU 读取
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Failed to map staging texture")?;

            let row_pitch = mapped.RowPitch as usize;
            let row_bytes = desc.Width as usize * bpp;
            let height = desc.Height as usize;

            // 逐行拷贝到 scratch buffer，剥离 RowPitch 末尾填充
            let src = mapped.pData as *const u8;
            for y in 0..height {
                // SAFETY: src 指向 mapped GPU 内存，row_pitch * y + row_bytes 不超过映射范围；
                //         self.buffer 已在 ensure_staging_texture 中预分配足够空间。
                std::ptr::copy_nonoverlapping(
                    src.add(y * row_pitch),
                    self.buffer.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }

            self.context.Unmap(staging, 0);

            // 交付所有权：从 scratch buffer 拷贝一份返回
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

        // 2x2 R16G16B16A16_FLOAT，全红像素
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

            // 剥离填充后，数据大小应恰好 = 2 × 2 × 8 = 32 字节
            assert_eq!(data.len(), 32, "Stripped buffer should be exactly 32 bytes");

            // 验证第一个像素
            let u16_data = std::slice::from_raw_parts(data.as_ptr() as *const u16, data.len() / 2);
            assert_eq!(u16_data[0], 0x3C00); // R
            assert_eq!(u16_data[1], 0x0000); // G
            assert_eq!(u16_data[2], 0x0000); // B
            assert_eq!(u16_data[3], 0x3C00); // A

            // 验证第二行第一个像素（offset = row_bytes = 16 bytes = 8 u16）
            assert_eq!(u16_data[8], 0x3C00); // R
            assert_eq!(u16_data[9], 0x0000); // G
        }
    }
}
