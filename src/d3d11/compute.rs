// D3D11 Compute Shader runtime: compile HLSL, bind resources, dispatch.
//
// Designed for single-pass image processing (tone-map, format conversion).
// Reuses the existing D3D11Context device and immediate context.

use anyhow::{bail, Context, Result};
use windows::core::PCSTR;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Thread group size matching our HLSL shaders.
const THREAD_GROUP_SIZE: u32 = 8;

/// Compiled compute shader, ready to dispatch.
pub struct ComputeShader {
    shader: ID3D11ComputeShader,
}

impl ComputeShader {
    /// Compile HLSL source into a compute shader.
    ///
    /// `entry_point` is the shader entry function name (e.g. "main").
    pub fn compile(device: &ID3D11Device, hlsl: &str, entry_point: &str) -> Result<Self> {
        let mut blob: Option<ID3DBlob> = None;
        let mut error_blob: Option<ID3DBlob> = None;

        let entry = format!("{}\0", entry_point);
        let target = b"cs_5_0\0";

        // SAFETY: D3DCompile reads from hlsl slice and writes to COM blobs.
        // All pointers are valid for the duration of the call.
        let hr = unsafe {
            D3DCompile(
                hlsl.as_ptr() as *const _,
                hlsl.len(),
                None,
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(target.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut blob,
                Some(&mut error_blob),
            )
        };

        if hr.is_err() {
            let msg = error_blob
                .as_ref()
                .map(|b| unsafe {
                    let ptr = b.GetBufferPointer() as *const u8;
                    let len = b.GetBufferSize();
                    String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).to_string()
                })
                .unwrap_or_else(|| format!("D3DCompile failed: {:?}", hr));
            bail!("Shader compilation failed: {}", msg.trim_end_matches('\0'));
        }

        let blob = blob.context("D3DCompile succeeded but returned no bytecode")?;

        // SAFETY: blob contains valid compiled bytecode from D3DCompile.
        let shader = unsafe {
            let ptr = blob.GetBufferPointer();
            let len = blob.GetBufferSize();
            let bytecode = std::slice::from_raw_parts(ptr as *const u8, len);
            let mut cs = None;
            device
                .CreateComputeShader(bytecode, None, Some(&mut cs))
                .context("CreateComputeShader failed")?;
            cs.unwrap()
        };

        Ok(Self { shader })
    }
}

/// Create a SRV for an existing texture (read-only input).
pub fn create_srv(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView> {
    // SAFETY: texture is a valid D3D11 resource; CreateShaderResourceView
    // reads the texture desc and creates a COM view object.
    unsafe {
        let mut srv = None;
        device
            .CreateShaderResourceView(texture, None, Some(&mut srv))
            .context("CreateShaderResourceView failed")?;
        Ok(srv.unwrap())
    }
}

/// Create a UAV-bindable output texture and its UAV.
///
/// The output texture has the same dimensions as `width` × `height`
/// with the specified format, bound for unordered access.
pub fn create_output(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<(ID3D11Texture2D, ID3D11UnorderedAccessView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };

    // SAFETY: desc is fully initialized; CreateTexture2D allocates a GPU resource.
    let texture = unsafe {
        let mut tex = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .context("CreateTexture2D for compute output failed")?;
        tex.unwrap()
    };

    // SAFETY: texture is a valid D3D11 resource with UAV bind flag.
    let uav = unsafe {
        let mut uav = None;
        device
            .CreateUnorderedAccessView(&texture, None, Some(&mut uav))
            .context("CreateUnorderedAccessView failed")?;
        uav.unwrap()
    };

    Ok((texture, uav))
}

/// Dispatch a compute shader over a width × height image.
///
/// Binds input SRV to t0, output UAV to u0, dispatches with ceil-division
/// thread groups, then unbinds resources.
pub fn dispatch(
    context: &ID3D11DeviceContext,
    shader: &ComputeShader,
    srv: &ID3D11ShaderResourceView,
    uav: &ID3D11UnorderedAccessView,
    width: u32,
    height: u32,
) {
    let groups_x = width.div_ceil(THREAD_GROUP_SIZE);
    let groups_y = height.div_ceil(THREAD_GROUP_SIZE);

    // SAFETY: All COM objects are valid. Bind → Dispatch → Unbind is the
    // standard D3D11 compute pattern. Unbinding prevents resource hazards.
    unsafe {
        context.CSSetShader(&shader.shader, None);
        context.CSSetShaderResources(0, Some(&[Some(srv.clone())]));

        let uav_list: [Option<ID3D11UnorderedAccessView>; 1] = [Some(uav.clone())];
        context.CSSetUnorderedAccessViews(0, 1, Some(uav_list.as_ptr()), None);

        context.Dispatch(groups_x, groups_y, 1);

        // Unbind to avoid resource hazards on subsequent operations
        let no_srv: [Option<ID3D11ShaderResourceView>; 1] = [None];
        let no_uav: [Option<ID3D11UnorderedAccessView>; 1] = [None];
        context.CSSetShaderResources(0, Some(&no_srv));
        context.CSSetUnorderedAccessViews(0, 1, Some(no_uav.as_ptr()), None);
        context.CSSetShader(None, None);
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

    /// Identity shader: output = input (no transformation).
    const IDENTITY_HLSL: &str = r#"
Texture2D<float4> InputTexture : register(t0);
RWTexture2D<float4> OutputTexture : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    OutputTexture[id.xy] = InputTexture[id.xy];
}
"#;

    #[test]
    fn test_identity_shader_roundtrip() {
        let ctx = create_d3d11_device().expect("D3D11 device creation failed");

        // Compile identity shader
        let shader =
            ComputeShader::compile(&ctx.device, IDENTITY_HLSL, "main").expect("Shader compile");

        // Create 4x4 R16G16B16A16_FLOAT input texture with known data
        let width = 4u32;
        let height = 4u32;
        let format = DXGI_FORMAT_R16G16B16A16_FLOAT;

        // f16: 1.0 = 0x3C00, 0.5 = 0x3800, 0.0 = 0x0000
        let pixel_data: [u16; 4] = [0x3C00, 0x3800, 0x0000, 0x3C00]; // (1.0, 0.5, 0.0, 1.0)
        let mut init_data = Vec::new();
        for _ in 0..(width * height) {
            for &v in &pixel_data {
                init_data.extend_from_slice(&v.to_ne_bytes());
            }
        }

        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
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
            SysMemPitch: width * 8, // 4 channels × 2 bytes
            SysMemSlicePitch: 0,
        };

        let input_texture = unsafe {
            let mut tex = None;
            ctx.device
                .CreateTexture2D(&tex_desc, Some(&subresource), Some(&mut tex))
                .expect("Create input texture");
            tex.unwrap()
        };

        // Create SRV, output texture + UAV
        let srv = create_srv(&ctx.device, &input_texture).expect("Create SRV");
        let (output_texture, uav) =
            create_output(&ctx.device, width, height, format).expect("Create output");

        // Dispatch
        dispatch(&ctx.context, &shader, &srv, &uav, width, height);

        // Readback output and verify
        let mut reader = TextureReader::new(ctx.device.clone(), ctx.context.clone());
        let result = reader.read_texture(&output_texture).expect("Readback");

        // Verify size: 4 × 4 × 8 bytes = 128
        assert_eq!(result.len(), 128, "Output size mismatch");

        // Verify first pixel matches input
        let u16_data =
            unsafe { std::slice::from_raw_parts(result.as_ptr() as *const u16, result.len() / 2) };
        assert_eq!(u16_data[0], 0x3C00, "R channel"); // 1.0
        assert_eq!(u16_data[1], 0x3800, "G channel"); // 0.5
        assert_eq!(u16_data[2], 0x0000, "B channel"); // 0.0
        assert_eq!(u16_data[3], 0x3C00, "A channel"); // 1.0

        // Verify last pixel (offset = 15 pixels × 4 channels = 60)
        assert_eq!(u16_data[60], 0x3C00, "Last pixel R");
        assert_eq!(u16_data[61], 0x3800, "Last pixel G");
    }
}
