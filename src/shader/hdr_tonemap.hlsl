// HDR tone-mapping shader skeleton.
// Step 1.2 keeps this file as a placeholder for upcoming GPU path.

Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    OutputTexture[id.xy] = InputTexture[id.xy];
}
