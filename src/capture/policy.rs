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
