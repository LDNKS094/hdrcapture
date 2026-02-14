/// Embedded HLSL source for HDR tone-mapping stage.
///
/// Three strategies available for A/B testing:
/// - `HDR_TONEMAP_HLSL`:          DWM-equivalent (hard clip + sRGB encode)
/// - `HDR_TONEMAP_SHOULDER_HLSL`:  Simple hybrid (linear + shoulder rolloff)
/// - `HDR_TONEMAP_EETF_HLSL`:     BT.2390 EETF (PQ-space Hermite spline)
pub const HDR_TONEMAP_HLSL: &str = include_str!("shader/hdr_tonemap.hlsl");
pub const HDR_TONEMAP_SHOULDER_HLSL: &str = include_str!("shader/hdr_tonemap_shoulder.hlsl");
pub const HDR_TONEMAP_EETF_HLSL: &str = include_str!("shader/hdr_tonemap_eetf.hlsl");
