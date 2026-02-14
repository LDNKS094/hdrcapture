// HDR (scRGB R16G16B16A16_FLOAT) -> SDR (B8G8R8A8_UNORM) tone-mapping.
//
// Simple hybrid: linear pass-through + shoulder rolloff.
//   [0, S]:  identity (matches DWM for SDR content)
//   (S, ∞):  local Reinhard shoulder, asymptotically approaches 1.0
//
// Lighter than BT.2390 EETF, no PQ conversion needed.

Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

cbuffer ToneMapParams : register(b0)
{
    float sdr_white_nits;
    float3 _pad;
};

float srgb_encode(float u)
{
    return (u <= 0.0031308) ? (u * 12.92) : (1.055 * pow(u, 1.0 / 2.4) - 0.055);
}

// Per-channel: linear below S, Reinhard shoulder above S.
// Maps [0, ∞) -> [0, 1.0)
float shoulder(float x, float S, float R)
{
    x = max(x, 0.0);
    if (x <= S)
        return x;
    float excess = x - S;
    return S + R * excess / (excess + R);
}

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    float4 rgba = InputTexture[id.xy];

    // 1. Normalize scRGB to SDR reference white
    float multiplier = 80.0 / max(sdr_white_nits, 1.0);
    rgba.rgb *= multiplier;

    // 2. Shoulder tone curve
    //    S = 0.8: 80% of SDR range untouched, 20% headroom for HDR
    static const float S = 0.8;
    static const float R = 1.0 - S;

    rgba.r = shoulder(rgba.r, S, R);
    rgba.g = shoulder(rgba.g, S, R);
    rgba.b = shoulder(rgba.b, S, R);

    // 3. sRGB encode
    rgba.rgb = float3(
        srgb_encode(rgba.r),
        srgb_encode(rgba.g),
        srgb_encode(rgba.b));

    OutputTexture[id.xy] = rgba;
}
