// HDR (scRGB R16G16B16A16_FLOAT) -> SDR (B8G8R8A8_UNORM) tone-mapping.
//
// DWM-equivalent logic for scRGB input:
//   1. Normalize scRGB by SDR white level (SDR white -> 1.0)
//   2. Hard clip to [0, 1]
//   3. sRGB gamma encode (piecewise: linear segment + power curve)

Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

cbuffer ToneMapParams : register(b0)
{
    float sdr_white_nits;
    float3 _pad;
};

// sRGB OETF: linear -> sRGB nonlinear (piecewise)
float srgb_encode(float u)
{
    return (u <= 0.0031308) ? (u * 12.92) : (1.055 * pow(u, 1.0 / 2.4) - 0.055);
}

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    float4 rgba = InputTexture[id.xy];

    // 1. Normalize scRGB to SDR reference white
    //    scRGB 1.0 = 80 nits; SDR content lives at sdr_white_nits/80.
    float multiplier = 80.0 / max(sdr_white_nits, 1.0);
    rgba.rgb *= multiplier;

    // 2. Hard clip
    rgba.rgb = saturate(rgba.rgb);

    // 3. sRGB gamma encode (linear -> display)
    rgba.rgb = float3(
        srgb_encode(rgba.r),
        srgb_encode(rgba.g),
        srgb_encode(rgba.b));

    OutputTexture[id.xy] = rgba;
}
