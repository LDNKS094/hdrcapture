use windows::Graphics::DirectX::DirectXPixelFormat;

/// Capture policy selected by caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePolicy {
    #[default]
    Auto,
    ForceSdr,
}

impl From<bool> for CapturePolicy {
    fn from(force_sdr: bool) -> Self {
        if force_sdr {
            Self::ForceSdr
        } else {
            Self::Auto
        }
    }
}

/// Decide WGC FramePool pixel format from capture policy.
///
/// Current implementation keeps existing behavior: always request BGRA8.
pub fn resolve_pixel_format(_policy: CapturePolicy) -> DirectXPixelFormat {
    DirectXPixelFormat::B8G8R8A8UIntNormalized
}
