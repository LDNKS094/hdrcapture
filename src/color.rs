pub mod tone_map;
pub mod white_level;

use anyhow::Result;

use crate::capture::CapturePolicy;

/// Pixel format used by color pipeline input/output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPixelFormat {
    Bgra8,
    Rgba16f,
}

/// Frame container passed through color pipeline.
pub struct ColorFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: f64,
    pub format: ColorPixelFormat,
}

/// Whether the frame needs color processing for the selected policy.
pub fn requires_processing(format: ColorPixelFormat, policy: CapturePolicy) -> bool {
    match (policy, format) {
        (CapturePolicy::ForceSdr, _) => false,
        (CapturePolicy::Auto, ColorPixelFormat::Bgra8) => false,
        (CapturePolicy::Auto, ColorPixelFormat::Rgba16f) => true,
    }
}

/// Unified color-processing entry.
///
/// Current implementation is a no-op pass-through.
pub fn process_frame(frame: ColorFrame, policy: CapturePolicy) -> Result<ColorFrame> {
    tone_map::process(frame, policy)
}
