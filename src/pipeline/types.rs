use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use crate::color::ColorPixelFormat;
use crate::memory::ElasticBufferPool;

/// Single frame capture result
#[derive(Clone)]
pub struct CapturedFrame {
    /// Pixel data (shared, read-only), length = width * height * bytes_per_pixel
    pub data: Arc<SharedFrameData>,
    /// Frame width (pixels)
    pub width: u32,
    /// Frame height (pixels)
    pub height: u32,
    /// Frame timestamp (seconds), relative to system boot time (QPC)
    pub timestamp: f64,
    /// Pixel format of `data`
    pub format: ColorPixelFormat,
}

impl CapturedFrame {
    pub fn bytes_per_pixel(&self) -> usize {
        match self.format {
            ColorPixelFormat::Bgra8 => 4,
            ColorPixelFormat::Rgba16f => 8,
        }
    }

    /// Save frame to file.
    ///
    /// Format is determined by file extension:
    /// - `.png` `.bmp` `.jpg` `.tiff` - standard formats (BGRA8 only)
    /// - `.jxr` - JPEG XR (both BGRA8 and RGBA16F)
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        crate::image::save(
            path.as_ref(),
            self.data.as_slice(),
            self.width,
            self.height,
            self.format,
        )
    }
}

pub struct SharedFrameData {
    pub(super) bytes: Vec<u8>,
    pub(super) pool: Arc<ElasticBufferPool>,
    pub(super) group_idx: usize,
}

impl SharedFrameData {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl std::ops::Deref for SharedFrameData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl Drop for SharedFrameData {
    fn drop(&mut self) {
        let bytes = std::mem::take(&mut self.bytes);
        self.pool.release_recycled(self.group_idx, bytes);
    }
}

pub(super) struct CropCache {
    pub(super) texture: ID3D11Texture2D,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) format: DXGI_FORMAT,
}

pub(super) struct RawFrame {
    pub(super) texture: ID3D11Texture2D,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) timestamp: f64,
    pub(super) format: ColorPixelFormat,
}
