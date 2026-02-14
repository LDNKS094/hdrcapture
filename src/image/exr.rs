// OpenEXR (.exr) encoding via the `exr` crate.
//
// Supports both BGRA8 (SDR) and RGBA16F (HDR) pixel data.
// EXR is the industry standard for HDR imagery in VFX, compositing,
// and professional editing tools (Photoshop, DaVinci Resolve, Blender, Nuke).

use std::path::Path;

use anyhow::{Context, Result};
use exr::prelude::*;

use crate::color::ColorPixelFormat;

/// Save pixel data as OpenEXR (.exr) file.
///
/// - `Bgra8`: converted to `f32` RGBA channels (0.0–1.0).
/// - `Rgba16f`: written as `f16` RGBA channels (native half-float).
pub fn save_exr(
    path: &Path,
    data: &[u8],
    width: u32,
    height: u32,
    format: ColorPixelFormat,
) -> Result<()> {
    let (w, h) = (width as usize, height as usize);

    match format {
        ColorPixelFormat::Bgra8 => save_bgra8(path, data, w, h),
        ColorPixelFormat::Rgba16f => save_rgba16f(path, data, w, h),
    }
}

/// Write BGRA8 data as f32 RGBA EXR.
fn save_bgra8(path: &Path, data: &[u8], w: usize, h: usize) -> Result<()> {
    let channels = SpecificChannels::rgba(|Vec2(x, y)| {
        let offset = (y * w + x) * 4;
        let b = data[offset] as f32 / 255.0;
        let g = data[offset + 1] as f32 / 255.0;
        let r = data[offset + 2] as f32 / 255.0;
        let a = data[offset + 3] as f32 / 255.0;
        (r, g, b, a)
    });

    let image = Image::from_channels((w, h), channels);
    image
        .write()
        .to_file(path)
        .context("failed to write EXR (BGRA8)")?;

    Ok(())
}

/// Write RGBA16F data as f16 RGBA EXR.
fn save_rgba16f(path: &Path, data: &[u8], w: usize, h: usize) -> Result<()> {
    // Reinterpret byte slice as f16 (2 bytes each, 4 channels = 8 bytes per pixel)
    let pixels: &[f16] = bytemuck_cast_f16(data);

    let channels = SpecificChannels::rgba(|Vec2(x, y)| {
        let offset = (y * w + x) * 4;
        let r = pixels[offset];
        let g = pixels[offset + 1];
        let b = pixels[offset + 2];
        let a = pixels[offset + 3];
        (r, g, b, a)
    });

    let image = Image::from_channels((w, h), channels);
    image
        .write()
        .to_file(path)
        .context("failed to write EXR (RGBA16F)")?;

    Ok(())
}

/// Reinterpret a `&[u8]` as `&[f16]`.
///
/// # Panics
/// Panics if the byte slice length is not a multiple of 2 or is misaligned.
fn bytemuck_cast_f16(data: &[u8]) -> &[f16] {
    assert!(data.len() % 2 == 0, "byte slice length must be even for f16 cast");
    // SAFETY: f16 is 2 bytes, repr(transparent) over u16. The exr crate re-exports `half::f16`.
    // GPU-allocated buffers from D3D11 are at least 16-byte aligned.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const f16, data.len() / 2) }
}
