// Image encoding module.
//
// Unified save() entry point dispatches by file extension:
// - Standard formats (png, bmp, jpg, tiff): `basic` submodule via `image` crate, BGRA8 only
// - JPEG XR (.jxr): `jxr` submodule via WIC COM API, supports both BGRA8 and RGBA16F

pub mod basic;
pub mod exr;
pub mod jxr;

use std::path::Path;

use anyhow::{bail, Result};

use crate::color::ColorPixelFormat;

/// Save pixel data to file. Format is determined by extension.
///
/// Supported extensions:
/// - `.png` — PNG (lossless, BGRA8 only)
/// - `.bmp` — BMP (lossless, BGRA8 only)
/// - `.jpg` / `.jpeg` — JPEG (lossy, BGRA8 only)
/// - `.tiff` / `.tif` — TIFF (lossless, BGRA8 only)
/// - `.jxr` — JPEG XR (lossless, BGRA8 and RGBA16F)
/// - `.exr` — OpenEXR (lossless, BGRA8 and RGBA16F)
pub fn save(
    path: &Path,
    data: &[u8],
    width: u32,
    height: u32,
    format: ColorPixelFormat,
) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "jxr" => jxr::save_jxr(path, data, width, height, format),
        "exr" => exr::save_exr(path, data, width, height, format),
        "png" | "bmp" | "jpg" | "jpeg" | "tiff" | "tif" => {
            basic::save(path, data, width, height, format)
        }
        _ => bail!(
            "unsupported extension '.{}'; supported: .png .bmp .jpg .tiff (SDR), .jxr .exr (HDR/SDR)",
            ext
        ),
    }
}
