/// Capture mode selected by caller.
///
/// - `Auto`: HDR environment → tone-map to 8-bit SDR; SDR environment → direct 8-bit.
/// - `Hdr`: Force 16-bit float capture, pass through raw HDR data (no tone-map).
/// - `Sdr`: Force 8-bit capture regardless of environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePolicy {
    #[default]
    Auto,
    Hdr,
    Sdr,
}

impl CapturePolicy {
    /// Parse from a mode string ("auto", "hdr", "sdr").
    pub fn from_mode(mode: &str) -> Option<Self> {
        match mode {
            "auto" => Some(Self::Auto),
            "hdr" => Some(Self::Hdr),
            "sdr" => Some(Self::Sdr),
            _ => None,
        }
    }
}
