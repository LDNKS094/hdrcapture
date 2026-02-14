// Image encoding module.
//
// Unified save() entry point dispatches by file extension.
// - Standard formats (png, bmp, jpg, tiff): `image` crate, BGRA8 only
// - JPEG XR (.jxr): WIC COM API, supports both BGRA8 and RGBA16F

pub mod jxr;

use std::path::Path;

use anyhow::{bail, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

use crate::color::ColorPixelFormat;

/// Save pixel data to file. Format is determined by extension.
///
/// Supported extensions:
/// - `.png` — PNG (lossless, BGRA8 only)
/// - `.bmp` — BMP (lossless, BGRA8 only)
/// - `.jpg` / `.jpeg` — JPEG (lossy, BGRA8 only)
/// - `.tiff` / `.tif` — TIFF (lossless, BGRA8 only)
/// - `.jxr` — JPEG XR (lossless, BGRA8 and RGBA16F)
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
        "png" => save_sdr(path, data, width, height, format, SdrFormat::Png),
        "bmp" => save_sdr(path, data, width, height, format, SdrFormat::Bmp),
        "jpg" | "jpeg" => save_sdr(path, data, width, height, format, SdrFormat::Jpeg),
        "tiff" | "tif" => save_sdr(path, data, width, height, format, SdrFormat::Tiff),
        _ => bail!(
            "unsupported extension '.{}'; supported: .png .bmp .jpg .tiff (SDR), .jxr (HDR/SDR)",
            ext
        ),
    }
}

/// SDR format variants handled by the `image` crate.
enum SdrFormat {
    Png,
    Bmp,
    Jpeg,
    Tiff,
}

/// Save BGRA8 frame using the `image` crate. Errors on RGBA16F input.
fn save_sdr(
    path: &Path,
    data: &[u8],
    width: u32,
    height: u32,
    format: ColorPixelFormat,
    sdr_fmt: SdrFormat,
) -> Result<()> {
    if format != ColorPixelFormat::Bgra8 {
        bail!(
            "{} only supports BGRA8 (SDR) frames; this frame is {:?}. Use .jxr for HDR data.",
            path.extension().unwrap_or_default().to_string_lossy(),
            format
        );
    }

    // BGRA → RGBA
    let mut rgba = data.to_vec();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);

    match sdr_fmt {
        SdrFormat::Png => {
            let encoder =
                PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
            encoder.write_image(&rgba, width, height, ExtendedColorType::Rgba8)?;
        }
        _ => {
            let img_fmt = match sdr_fmt {
                SdrFormat::Bmp => ImageFormat::Bmp,
                SdrFormat::Jpeg => ImageFormat::Jpeg,
                SdrFormat::Tiff => ImageFormat::Tiff,
                SdrFormat::Png => unreachable!(),
            };
            image::write_buffer_with_format(
                &mut writer,
                &rgba,
                width,
                height,
                ExtendedColorType::Rgba8,
                img_fmt,
            )?;
        }
    }

    Ok(())
}
