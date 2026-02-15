// Standard image format encoding via the `image` crate.
//
// Supports BGRA8 (SDR) frames only:
// - PNG  (lossless)
// - BMP  (lossless)
// - JPEG (lossy)
// - TIFF (lossless)

use std::path::Path;

use anyhow::{bail, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

use crate::color::ColorPixelFormat;

/// SDR format variants handled by the `image` crate.
enum SdrFormat {
    Png,
    Bmp,
    Jpeg,
    Tiff,
}

/// Save a BGRA8 frame using the `image` crate.
///
/// The target format is inferred from the file extension.
/// Errors if the pixel format is not BGRA8.
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

    let sdr_fmt = match ext.as_str() {
        "png" => SdrFormat::Png,
        "bmp" => SdrFormat::Bmp,
        "jpg" | "jpeg" => SdrFormat::Jpeg,
        "tiff" | "tif" => SdrFormat::Tiff,
        _ => bail!("basic: unsupported extension '.{}'", ext),
    };

    if format != ColorPixelFormat::Bgra8 {
        bail!(
            "{} only supports BGRA8 (SDR) frames; this frame is {:?}. Use .jxr for HDR data.",
            ext,
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
        SdrFormat::Jpeg => {
            // JPEG doesn't support alpha; strip to RGB
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|px| &px[..3])
                .copied()
                .collect();
            image::write_buffer_with_format(
                &mut writer,
                &rgb,
                width,
                height,
                ExtendedColorType::Rgb8,
                ImageFormat::Jpeg,
            )?;
        }
        _ => {
            let img_fmt = match sdr_fmt {
                SdrFormat::Bmp => ImageFormat::Bmp,
                SdrFormat::Tiff => ImageFormat::Tiff,
                SdrFormat::Png | SdrFormat::Jpeg => unreachable!(),
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
