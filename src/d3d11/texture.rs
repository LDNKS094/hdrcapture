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
