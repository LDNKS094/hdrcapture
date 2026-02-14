// JPEG XR (.jxr) encoding via Windows Imaging Component (WIC).
//
// Supports both BGRA8 and RGBA16F pixel data.
// JPEG XR (HD Photo) is the only widely-supported HDR image format on Windows,
// natively viewable in Photos app and supported by all WIC-based tools.

use std::path::Path;

use anyhow::{bail, Context, Result};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatWmp, GUID_WICPixelFormat32bppBGRA,
    GUID_WICPixelFormat64bppRGBAHalf, IWICBitmapFrameEncode, IWICImagingFactory,
    WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag2;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use crate::color::ColorPixelFormat;

/// GENERIC_WRITE access flag (0x40000000).
/// Defined here to avoid pulling in Win32_Storage_FileSystem feature.
const GENERIC_WRITE: u32 = 0x40000000;

/// Save pixel data as JPEG XR (.jxr) file.
///
/// Supports both `Bgra8` (32bpp) and `Rgba16f` (64bpp half-float) formats.
/// Uses WIC COM API; COM is initialized per-call (safe if already initialized).
pub fn save_jxr(
    path: &Path,
    data: &[u8],
    width: u32,
    height: u32,
    format: ColorPixelFormat,
) -> Result<()> {
    let (pixel_format, stride) = match format {
        ColorPixelFormat::Bgra8 => (GUID_WICPixelFormat32bppBGRA, width * 4),
        ColorPixelFormat::Rgba16f => (GUID_WICPixelFormat64bppRGBAHalf, width * 8),
    };

    let expected_len = stride as usize * height as usize;
    if data.len() < expected_len {
        bail!(
            "pixel data too short: expected {} bytes ({}x{}x{}bpp), got {}",
            expected_len,
            width,
            height,
            stride / width,
            data.len()
        );
    }

    // SAFETY: All WIC calls operate on COM objects created in this scope.
    // COM is initialized per-call; CoInitializeEx returns S_FALSE if already init'd.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .context("Failed to create WIC imaging factory")?;

        // Create output stream
        let stream = factory.CreateStream()?;
        let wide_path = to_wide(path);
        stream.InitializeFromFilename(PCWSTR(wide_path.as_ptr()), GENERIC_WRITE)?;

        // Create JPEG XR encoder
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatWmp, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;

        // Create frame
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut props: Option<IPropertyBag2> = None;
        encoder.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.context("WIC CreateNewFrame returned null")?;

        // Initialize frame with default properties
        if let Some(ref props) = props {
            frame.Initialize(props)?;
        }

        // Set frame dimensions and pixel format
        frame.SetSize(width, height)?;
        let mut fmt: GUID = pixel_format;
        frame.SetPixelFormat(&mut fmt)?;

        // Verify WIC accepted our format (it may silently convert)
        if fmt != pixel_format {
            bail!(
                "WIC rejected pixel format for JXR encoding; \
                 requested {:?}, got {:?}",
                pixel_format,
                fmt
            );
        }

        // Write pixel data
        frame.WritePixels(height, stride, data)?;

        // Commit frame and encoder
        frame.Commit()?;
        encoder.Commit()?;
    }

    Ok(())
}

/// Convert a Path to a null-terminated UTF-16 string for Win32 APIs.
fn to_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
