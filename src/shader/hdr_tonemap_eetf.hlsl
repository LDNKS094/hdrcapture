// HDR (scRGB R16G16B16A16_FLOAT) -> SDR (B8G8R8A8_UNORM) tone-mapping.
//
// BT.2390 EETF approach: perceptually uniform HDR->SDR mapping.
//   1. Normalize scRGB by SDR white level
//   2. BT.2390 EETF in PQ space (maxRGB method, Hermite spline shoulder)
//   3. sRGB piecewise encode
//
// Based on ITU-R BT.2390 and OBS Studio's color.effect implementation.
// The EETF operates in PQ (ST 2084) perceptual space, giving natural
// rolloff that matches human vision sensitivity.

Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

cbuffer ToneMapParams : register(b0)
{
    float sdr_white_nits;
    float3 _pad;
};

// --- ST 2084 (PQ) transfer functions ---

float linear_to_pq(float x)
{
    float c = pow(abs(x), 0.1593017578);
    return pow((0.8359375 + 18.8515625 * c) / (1.0 + 18.6875 * c), 78.84375);
}

float pq_to_linear(float u)
{
    float c = pow(abs(u), 1.0 / 78.84375);
    return pow(abs(max(c - 0.8359375, 0.0) / (18.8515625 - 18.6875 * c)), 1.0 / 0.1593017578);
}

// --- BT.2390 EETF (Hermite spline in PQ space) ---

// Core EETF: maps normalized PQ value E1 to compressed E2.
// Lw = source peak (nits), Lmax = target peak (nits).
float eetf_channel(float maxRGB_pq, float Lw, float Lmax)
{
    float Lw_pq = linear_to_pq(Lw / 10000.0);
    float E1 = saturate(maxRGB_pq / Lw_pq);

    float maxLum = linear_to_pq(Lmax / 10000.0) / Lw_pq;
    float KS = 1.5 * maxLum - 0.5;

    float E2 = E1;
    if (E1 > KS)
    {
        float T = (E1 - KS) / (1.0 - KS);
        float T2 = T * T;
        float T3 = T2 * T;
        // Hermite spline: C1 continuous at KS, approaches maxLum
        E2 = (2.0 * T3 - 3.0 * T2 + 1.0) * KS
           + (T3 - 2.0 * T2 + T) * (1.0 - KS)
           + (-2.0 * T3 + 3.0 * T2) * maxLum;
    }

    return E2 * Lw_pq;
}

// Apply EETF to linear RGB (in nits) using maxRGB method (preserves hue).
float3 eetf_linear(float3 rgb_nits, float Lw, float Lmax)
{
    float maxRGB_nits = max(max(rgb_nits.r, rgb_nits.g), rgb_nits.b);
    float maxRGB_pq = linear_to_pq(maxRGB_nits / 10000.0);

    float mapped_pq = eetf_channel(maxRGB_pq, Lw, Lmax);
    float mapped_nits = pq_to_linear(mapped_pq) * 10000.0;

    // Uniform scale all channels by the same ratio (preserves hue)
    float scale = mapped_nits / max(maxRGB_nits, 6.10352e-5);
    return rgb_nits * scale;
}

// --- sRGB OETF ---

float srgb_encode(float u)
{
    return (u <= 0.0031308) ? (u * 12.92) : (1.055 * pow(u, 1.0 / 2.4) - 0.055);
}

// --- Main ---

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    float4 rgba = InputTexture[id.xy];

    // 1. Normalize scRGB to absolute nits
    //    scRGB 1.0 = 80 nits, so pixel_nits = pixel * 80.
    //    But we want values relative to SDR white = 1.0,
    //    so normalize: pixel * 80 / sdr_white_nits.
    //    Then EETF maps from source peak (Lw) to target peak (Lmax).
    //
    //    Alternative: work in nits directly.
    //    pixel_nits = rgba.rgb * 80.0
    //    EETF(pixel_nits, Lw=source_peak_nits, Lmax=sdr_white_nits)
    //    Then normalize result to [0,1] by dividing by sdr_white_nits.

    // Convert to nits
    float3 nits = rgba.rgb * 80.0;

    // Clamp negatives (out-of-gamut scRGB values)
    nits = max(nits, 0.0);

    // Source peak: assume typical HDR content peak.
    // For scRGB desktop, max is typically sdr_white_nits for SDR content,
    // but HDR content can go much higher. Use 1000 nits as assumed source peak.
    float Lw = 1000.0;
    float Lmax = sdr_white_nits;

    // Only apply EETF if source peak exceeds target
    if (Lw > Lmax)
    {
        nits = eetf_linear(nits, Lw, Lmax);
    }

    // Normalize to [0, 1] relative to SDR white
    rgba.rgb = nits / sdr_white_nits;
    rgba.rgb = saturate(rgba.rgb);

    // sRGB encode
    rgba.rgb = float3(
        srgb_encode(rgba.r),
        srgb_encode(rgba.g),
        srgb_encode(rgba.b));

    OutputTexture[id.xy] = rgba;
}
