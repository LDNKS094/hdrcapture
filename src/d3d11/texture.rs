// 纹理创建与回读工具函数

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// 纹理读取器：负责将 GPU 纹理数据回读到 CPU
pub struct TextureReader {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging_texture: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
}

impl TextureReader {
    /// 创建新的纹理读取器
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        Self {
            device,
            context,
            staging_texture: None,
            width: 0,
            height: 0,
        }
    }

    /// 确保 Staging Texture 存在且尺寸匹配
    fn ensure_staging_texture(&mut self, width: u32, height: u32) -> Result<()> {
        if self.staging_texture.is_some() && self.width == width && self.height == height {
            return Ok(());
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
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
        }

        Ok(())
    }

    /// 从 GPU 纹理读取数据到 CPU
    ///
    /// 返回的数据是 R16G16B16A16_FLOAT 格式的字节流
    pub fn read_texture(&mut self, source_texture: &ID3D11Texture2D) -> Result<Vec<u8>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            source_texture.GetDesc(&mut desc);
        }

        // 1. 准备 Staging Texture
        self.ensure_staging_texture(desc.Width, desc.Height)?;
        let staging = self.staging_texture.as_ref().unwrap();

        unsafe {
            // 2. 将数据从源纹理拷贝到 Staging 纹理
            // CopyResource 适用于整个资源的拷贝，源和目标必须尺寸格式一致
            self.context.CopyResource(staging, source_texture);

            // 3. 映射内存供 CPU 读取
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Failed to map staging texture")?;

            // 4. 读取数据
            // 计算数据大小：宽 * 高 * 8字节 (16bit * 4 channel)
            let row_pitch = mapped.RowPitch as usize; // 每行字节数（包含填充）
            let data_size = (desc.Height * row_pitch as u32) as usize;

            let mut buffer = Vec::with_capacity(data_size);

            // 注意：直接从 mapped.pData 拷贝
            // 这里的实现简单粗暴，直接把整块内存（包含每行末尾可能的填充）都拷贝了
            // 在 Python 端处理时需要注意 RowPitch
            std::ptr::copy_nonoverlapping(
                mapped.pData as *const u8,
                buffer.as_mut_ptr(),
                data_size,
            );
            buffer.set_len(data_size);

            // 5. 解除映射
            self.context.Unmap(staging, 0);

            Ok(buffer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d11::create_d3d11_device;

    #[test]
    fn test_texture_readback_logic() {
        let d3d_ctx = create_d3d11_device().unwrap();
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // 1. 创建一个 2x2 的源纹理 (Default Usage)
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

        // 2. 准备初始化数据
        // 每个像素 4 个 f16 (RGBA)，2x2 共 4 个像素
        // 假设全红：(1.0, 0.0, 0.0, 1.0)
        // f16::from_f32(1.0) -> 0x3C00
        // f16::from_f32(0.0) -> 0x0000
        let pixel_red: [u16; 4] = [0x3C00, 0x0000, 0x0000, 0x3C00];
        let mut init_data = Vec::new();
        for _ in 0..4 {
            init_data.extend_from_slice(&pixel_red); // 4 个像素
        }

        // 重要：创建 Initialize Data 描述
        // SysMemPitch: 每行字节数 = 2 像素 * 8 字节 = 16
        let subresource_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: init_data.as_ptr() as *const _,
            SysMemPitch: 16,
            SysMemSlicePitch: 0,
        };

        unsafe {
            let mut texture = None;
            d3d_ctx
                .device
                .CreateTexture2D(&desc, Some(&subresource_data), Some(&mut texture))
                .unwrap();
            let texture = texture.unwrap();

            // 3. 读取数据
            let data = reader.read_texture(&texture).unwrap();

            // 4. 验证
            // 注意：data 可能包含 RowPitch 填充，所以不能直接比较 Vec
            // 2x2 纹理通常太小，显卡可能会按 256 字节对齐
            println!("Readback data size: {}", data.len());

            // 验证第一个像素
            let u16_data = std::slice::from_raw_parts(data.as_ptr() as *const u16, data.len() / 2);
            assert_eq!(u16_data[0], 0x3C00); // R
            assert_eq!(u16_data[1], 0x0000); // G
            assert_eq!(u16_data[2], 0x0000); // B
            assert_eq!(u16_data[3], 0x3C00); // A
        }
    }
}
