use anyhow::Result;

use crate::capture::CapturePolicy;

use super::{ColorFrame, ColorPixelFormat};

/// HDR/SDR tone-mapping stage.
///
/// Current stage is pass-through for all paths.
///
/// Matching is kept here so policy/format decision remains in color layer.
pub fn process(frame: ColorFrame, policy: CapturePolicy) -> Result<ColorFrame> {
    match (policy, frame.format) {
        (CapturePolicy::ForceSdr, _) => Ok(frame),
        (CapturePolicy::Auto, ColorPixelFormat::Bgra8) => Ok(frame),
        (CapturePolicy::Auto, ColorPixelFormat::Rgba16f) => Ok(frame),
    }
}
