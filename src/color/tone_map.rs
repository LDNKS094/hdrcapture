// HDR/SDR tone-mapping stage.
//
// ToneMapPass holds compiled shader and GPU resources, created once per pipeline.
// process() dispatches the compute shader for Auto+Rgba16f frames,
// passes through all other combinations unchanged.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::capture::CapturePolicy;
use crate::d3d11::compute::{self, ComputeShader};

use super::{ColorFrame, ColorPixelFormat};

/// Constant buffer layout matching HLSL `ToneMapParams`.
#[repr(C)]
struct ToneMapParams {
    sdr_white_nits: f32,
    _pad: [f32; 3],
}

/// GPU tone-map pass: scRGB R16G16B16A16_FLOAT → BGRA8.
///
/// Created once per pipeline lifetime. Output texture is lazily created
/// and reused when dimensions match.
pub struct ToneMapPass {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shader: ComputeShader,
    cbuffer: ID3D11Buffer,
    /// Cached output texture + UAV, rebuilt on size change.
    output_cache: Option<OutputCache>,
}

struct OutputCache {
    texture: ID3D11Texture2D,
    uav: ID3D11UnorderedAccessView,
    width: u32,
    height: u32,
}

impl ToneMapPass {
    /// Create a new tone-map pass, compiling the shader.
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self> {
        let hlsl = crate::shader::HDR_TONEMAP_HLSL;
        let shader = ComputeShader::compile(device, hlsl, "main")?;

        // Create constant buffer (16 bytes, one float + padding)
        let cb_desc = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<ToneMapParams>() as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };

        // SAFETY: cb_desc is fully initialized; CreateBuffer allocates a GPU resource.
        let cbuffer = unsafe {
            let mut buf = None;
            device
                .CreateBuffer(&cb_desc, None, Some(&mut buf))
                .context("CreateBuffer for tone-map cbuffer failed")?;
            buf.unwrap()
        };

        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            shader,
            cbuffer,
            output_cache: None,
        })
    }

    /// Update the constant buffer with the current SDR white level.
    fn update_cbuffer(&self, sdr_white_nits: f32) -> Result<()> {
        // SAFETY: Map/Unmap pattern for DYNAMIC buffer with WRITE_DISCARD.
        // The buffer is 16 bytes, matching ToneMapParams layout.
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(
                    &self.cbuffer,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )
                .context("Map cbuffer failed")?;

            let params = mapped.pData as *mut ToneMapParams;
            (*params).sdr_white_nits = sdr_white_nits;

            self.context.Unmap(&self.cbuffer, 0);
        }
        Ok(())
    }

    /// Ensure output texture + UAV exist and match the given dimensions.
    fn ensure_output(&mut self, width: u32, height: u32) -> Result<()> {
        if let Some(ref cache) = self.output_cache {
            if cache.width == width && cache.height == height {
                return Ok(());
            }
        }

        let (texture, uav) =
            compute::create_output(&self.device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM)?;

        self.output_cache = Some(OutputCache {
            texture,
            uav,
            width,
            height,
        });
        Ok(())
    }

    /// Execute tone-map: scRGB float16 input → BGRA8 output texture.
    ///
    /// Returns the output texture. The input frame's texture must remain
    /// valid until this call returns (GPU work is synchronous on immediate context).
    pub fn execute(&mut self, input: &ColorFrame, sdr_white_nits: f32) -> Result<ID3D11Texture2D> {
        self.ensure_output(input.width, input.height)?;
        self.update_cbuffer(sdr_white_nits)?;

        let srv = compute::create_srv(&self.device, &input.texture)?;
        let cache = self.output_cache.as_ref().unwrap();

        // Bind constant buffer to b0
        // SAFETY: cbuffer is a valid D3D11 buffer, binding to CS stage slot 0.
        unsafe {
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.cbuffer.clone())]));
        }

        compute::dispatch(
            &self.context,
            &self.shader,
            &srv,
            &cache.uav,
            input.width,
            input.height,
        );

        // Unbind constant buffer
        // SAFETY: Unbinding prevents resource hazards.
        unsafe {
            let no_cb: [Option<ID3D11Buffer>; 1] = [None];
            self.context.CSSetConstantBuffers(0, Some(&no_cb));
        }

        Ok(cache.texture.clone())
    }
}

/// Color processing entry point.
///
/// - `Auto + Rgba16f`: run GPU tone-map, output BGRA8 texture.
/// - All other combinations: pass-through unchanged.
pub fn process(
    frame: ColorFrame,
    policy: CapturePolicy,
    tone_map_pass: Option<&mut ToneMapPass>,
    sdr_white_nits: f32,
) -> Result<ColorFrame> {
    match (policy, frame.format) {
        (CapturePolicy::Auto, ColorPixelFormat::Rgba16f) => {
            let pass =
                tone_map_pass.expect("ToneMapPass required for Auto+Rgba16f but not provided");
            let output_texture = pass.execute(&frame, sdr_white_nits)?;
            Ok(ColorFrame {
                texture: output_texture,
                width: frame.width,
                height: frame.height,
                timestamp: frame.timestamp,
                format: ColorPixelFormat::Bgra8,
            })
        }
        _ => Ok(frame),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d11::create_d3d11_device;
    use crate::d3d11::texture::TextureReader;
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
    };

    /// Test tone-map pass with a known scRGB input.
    #[test]
    fn test_tonemap_pass_produces_bgra8() {
        let ctx = create_d3d11_device().expect("D3D11 device");
        let mut pass = ToneMapPass::new(&ctx.device, &ctx.context).expect("ToneMapPass creation");

        let width = 4u32;
        let height = 4u32;

        // scRGB pixel: (1.0, 0.5, 0.0, 1.0) — orange, within SDR range
        let pixel: [u16; 4] = [0x3C00, 0x3800, 0x0000, 0x3C00];
        let mut init_data = Vec::new();
        for _ in 0..(width * height) {
            for &v in &pixel {
                init_data.extend_from_slice(&v.to_ne_bytes());
            }
        }

        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let subresource = D3D11_SUBRESOURCE_DATA {
            pSysMem: init_data.as_ptr() as *const _,
            SysMemPitch: width * 8,
            SysMemSlicePitch: 0,
        };

        let input_texture = unsafe {
            let mut tex = None;
            ctx.device
                .CreateTexture2D(&tex_desc, Some(&subresource), Some(&mut tex))
                .expect("Create input texture");
            tex.unwrap()
        };

        let frame = ColorFrame {
            texture: input_texture,
            width,
            height,
            timestamp: 0.0,
            format: ColorPixelFormat::Rgba16f,
        };

        // Use 80 nits (identity scaling: multiplier = 80/80 = 1.0)
        let result = pass.execute(&frame, 80.0).expect("Tone-map execute");

        // Readback and verify output is BGRA8
        let mut reader = TextureReader::new(ctx.device.clone(), ctx.context.clone());
        let data = reader.read_texture(&result).expect("Readback");

        // 4 × 4 × 4 bytes (BGRA8) = 64
        assert_eq!(data.len(), 64, "Output should be BGRA8");

        // First pixel: tone-mapped orange, BGRA order
        // After rec709->rec2020->reinhard->rec2020->rec709, values are non-trivial.
        // B channel is non-zero due to gamut matrix cross-talk.
        let b = data[0];
        let g = data[1];
        let r = data[2];
        let a = data[3];

        println!("First pixel BGRA: ({}, {}, {}, {})", b, g, r, a);
        assert!(r > 0, "R should be non-zero");
        assert!(g > 0, "G should be non-zero");
        assert!(r > b, "R should be greater than B for orange input");
        assert!(a > 200, "A should be near 255");
    }
}
