// HDR (scRGB R16G16B16A16_FLOAT) -> SDR (B8G8R8A8_UNORM) tone-mapping.
//
// Pipeline: normalize by SDR white level -> Rec.709->Rec.2020 ->
//           Reinhard compress -> gamma 2.4 -> sRGB EOTF -> Rec.2020->Rec.709 ->
//           output as BGRA8.
//
// Matches OBS PSDrawMultiplyTonemap (opaque.effect) logic.

Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

// sdr_white_nits passed via constant buffer (e.g. 240.0)
cbuffer ToneMapParams : register(b0)
{
    float sdr_white_nits;
    float3 _pad;
};

// --- Color space matrices (OBS color.effect) ---

float3 rec709_to_rec2020(float3 v)
{
    float r = dot(v, float3(0.62740389593469903, 0.32928303837788370, 0.043313065687417225));
    float g = dot(v, float3(0.069097289358232075, 0.91954039507545871, 0.011362315566309178));
    float b = dot(v, float3(0.016391438875150280, 0.088013307877225749, 0.89559525324762401));
    return float3(r, g, b);
}

float3 rec2020_to_rec709(float3 v)
{
    float r = dot(v, float3( 1.6604910021084345, -0.58764113878854951, -0.072849863319884883));
    float g = dot(v, float3(-0.12455047452159074,  1.1328998971259603, -0.0083494226043694768));
    float b = dot(v, float3(-0.018150763354905303, -0.10057889800800739, 1.1187296613629127));
    return float3(r, g, b);
}

// --- sRGB transfer functions ---

float srgb_linear_to_nonlinear_channel(float u)
{
    return (u <= 0.0031308) ? (u * 12.92) : ((1.055 * pow(u, 1.0 / 2.4)) - 0.055);
}

float srgb_nonlinear_to_linear_channel(float u)
{
    return (u <= 0.04045) ? (u / 12.92) : pow((u + 0.055) / 1.055, 2.4);
}

float3 srgb_nonlinear_to_linear(float3 v)
{
    return float3(
        srgb_nonlinear_to_linear_channel(v.r),
        srgb_nonlinear_to_linear_channel(v.g),
        srgb_nonlinear_to_linear_channel(v.b));
}

// --- Reinhard tone-map (OBS color.effect) ---

float3 reinhard(float3 rgb)
{
    rgb = max(rgb, 0.0);
    rgb /= (rgb + float3(1.0, 1.0, 1.0));
    rgb = saturate(rgb);
    rgb = pow(rgb, float3(1.0 / 2.4, 1.0 / 2.4, 1.0 / 2.4));
    rgb = srgb_nonlinear_to_linear(rgb);
    return rgb;
}

// --- Main ---

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    float4 rgba = InputTexture[id.xy];

    // 1. Normalize scRGB to 80-nit reference white
    float multiplier = 80.0 / max(sdr_white_nits, 1.0);
    rgba.rgb *= multiplier;

    // 2. Gamut: Rec.709 -> Rec.2020 (Reinhard works better in wider gamut)
    rgba.rgb = rec709_to_rec2020(rgba.rgb);

    // 3. Tone-map: Reinhard compress + gamma encode + sRGB EOTF
    rgba.rgb = reinhard(rgba.rgb);

    // 4. Gamut: Rec.2020 -> Rec.709
    rgba.rgb = rec2020_to_rec709(rgba.rgb);

    // 5. Clamp and write (D3D11 handles BGRA channel mapping automatically)
    rgba.rgb = saturate(rgba.rgb);
    OutputTexture[id.xy] = rgba;
}
