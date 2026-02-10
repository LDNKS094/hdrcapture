// D3D11 设备创建与管理

pub mod texture;

use anyhow::Context;
use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

/// D3D11 设备上下文
pub struct D3D11Context {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub dxgi_device: IDXGIDevice,
    pub direct3d_device: IDirect3DDevice,
}

/// 创建 D3D11 设备
pub fn create_d3d11_device() -> anyhow::Result<D3D11Context> {
    let (device, context) = unsafe {
        let mut device = None;
        let mut context = None;

        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .context("Failed to create D3D11 device")?(device.unwrap(), context.unwrap())
    };

    let dxgi_device: IDXGIDevice = device.cast().unwrap();

    let direct3d_device: IDirect3DDevice = unsafe {
        CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
            .unwrap()
            .cast()
            .unwrap()
    };

    Ok(D3D11Context {
        device,
        context,
        dxgi_device,
        direct3d_device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_device_info(dxgi_device: &IDXGIDevice) -> anyhow::Result<()> {
        unsafe {
            let adapter = dxgi_device.GetAdapter()?;
            let desc = adapter.GetDesc()?;
            let name = String::from_utf16_lossy(&desc.Description);

            println!("✅ D3D11 Device Created");
            println!("   GPU: {}", name.trim_end_matches('\0'));
            println!("   VRAM: {} MB", desc.DedicatedVideoMemory / 1024 / 1024);
        }
        Ok(())
    }

    #[test]
    fn test_device_creation() {
        let ctx = create_d3d11_device().expect("Failed to create device");
        print_device_info(&ctx.dxgi_device).unwrap();
    }

    #[test]
    fn test_dxgi_adapter() {
        let ctx = create_d3d11_device().unwrap();

        unsafe {
            // Verify adapter access
            let adapter = ctx.dxgi_device.GetAdapter();
            assert!(adapter.is_ok());

            // Verify adapter description
            let desc = adapter.unwrap().GetDesc();
            assert!(desc.is_ok());
        }
    }
}
