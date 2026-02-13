pub mod tone_map;
pub mod white_level;

use anyhow::Result;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::capture::CapturePolicy;

/// Pixel format used by color pipeline input/output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPixelFormat {
    Bgra8,
    Rgba16f,
}

/// Frame container passed through color pipeline.
pub struct ColorFrame {
    pub texture: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    pub timestamp: f64,
    pub format: ColorPixelFormat,
}

/// Unified color-processing entry.
///
/// Current implementation is a no-op pass-through.
pub fn process_frame(frame: ColorFrame, policy: CapturePolicy) -> Result<ColorFrame> {
    tone_map::process(frame, policy)
}
