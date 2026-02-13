use anyhow::Result;

use crate::capture::CapturePolicy;

use super::ColorFrame;

/// HDR/SDR tone-mapping stage.
///
/// Current stage is intentionally pass-through.
pub fn process(frame: ColorFrame, _policy: CapturePolicy) -> Result<ColorFrame> {
    Ok(frame)
}
