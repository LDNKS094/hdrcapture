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
        .context("D3D11CreateDevice 失败")?;

        (device.unwrap(), context.unwrap())
    };

    let dxgi_device: IDXGIDevice = device.cast().unwrap();

    let direct3d_device: IDirect3DDevice = unsafe {
        CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
            .unwrap()
            .cast()
            .unwrap()
    };

    let _ = print_device_info(&dxgi_device);

    Ok(D3D11Context {
        device,
        context,
        dxgi_device,
        direct3d_device,
    })
}

fn print_device_info(dxgi_device: &IDXGIDevice) -> anyhow::Result<()> {
    unsafe {
        let adapter = dxgi_device.GetAdapter()?;
        let desc = adapter.GetDesc()?;
        let name = String::from_utf16_lossy(&desc.Description);

        println!("✅ D3D11 设备创建成功");
        println!("   GPU: {}", name.trim_end_matches('\0'));
        println!("   显存: {} MB", desc.DedicatedVideoMemory / 1024 / 1024);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_creation() {
        // 如果创建失败会 panic，成功就说明设备有效
        let _ctx = create_d3d11_device().expect("设备创建失败");
    }

    #[test]
    fn test_device_info() {
        let ctx = create_d3d11_device().unwrap();

        // 验证可以获取设备信息
        let result = print_device_info(&ctx.dxgi_device);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dxgi_adapter() {
        let ctx = create_d3d11_device().unwrap();

        unsafe {
            // 验证可以获取适配器
            let adapter = ctx.dxgi_device.GetAdapter();
            assert!(adapter.is_ok());

            // 验证可以获取适配器描述
            let desc = adapter.unwrap().GetDesc();
            assert!(desc.is_ok());
        }
    }
}
