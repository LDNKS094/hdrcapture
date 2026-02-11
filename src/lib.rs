// HDR Capture Library
// 解决 Windows HDR 环境下屏幕截图泛白问题
//
// 核心原理：WGC 请求 BGRA8 格式时，DWM 自动完成 HDR→SDR 色调映射。

#![cfg(windows)]
#![allow(dead_code)] // 开发阶段允许未使用的代码
#![allow(unused_imports)] // 开发阶段允许未使用的导入

// 模块声明
pub(crate) mod capture;
pub(crate) mod d3d11;
pub(crate) mod pipeline;

#[cfg(test)]
mod tests {
    use crate::capture::{enable_dpi_awareness, init_capture, CaptureTarget};
    use crate::d3d11::create_d3d11_device;
    use crate::d3d11::texture::TextureReader;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    /// 集成测试：验证 WGC 捕获管线（Device -> Capture -> Texture Readback）
    #[test]
    fn test_wgc_capture_pipeline() {
        use std::thread;
        use std::time::Duration;

        // 1. 准备环境
        enable_dpi_awareness();

        let d3d_ctx = create_d3d11_device().unwrap();
        let monitor = get_primary_monitor();

        // 2. 初始化捕获会话
        let target = CaptureTarget::Monitor(monitor);
        let capture = init_capture(&d3d_ctx, target).unwrap();
        println!("WGC session initialized");

        // 3. 启动捕获
        capture.start().unwrap();
        println!("Capture started, waiting for frame...");

        // 4. 等待一帧准备好
        thread::sleep(Duration::from_millis(100));

        // 5. 捕获一帧
        let texture = capture.capture_frame().unwrap();
        println!("Frame captured");

        // 6. 验证纹理格式（必须是 BGRA8）
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);

            println!(
                "Texture: {}x{} format={:?}",
                desc.Width, desc.Height, desc.Format
            );

            assert_eq!(
                desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM,
                "Format must be B8G8R8A8_UNORM"
            );
            assert!(desc.Width > 0);
            assert!(desc.Height > 0);
            assert_eq!(desc.MipLevels, 1);
        }

        // 7. 回读数据
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());
        let data = reader.read_texture(&texture).unwrap();
        println!("Readback: {} bytes", data.len());

        let has_data = data.iter().any(|&b| b != 0);
        assert!(has_data, "Captured image should not be all black");

        // 8. 保存测试图像
        save_test_image(&texture, &data, "test_capture.png");

        println!("WGC capture pipeline test passed");
    }

    // --- 测试辅助函数 ---

    /// 保存测试图像（BGRA8 → RGBA PNG）
    fn save_test_image(
        texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        data: &[u8],
        filename: &str,
    ) {
        use image::{ImageBuffer, Rgba};

        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);

            let width = desc.Width;
            let height = desc.Height;

            // BGRA8 → RGBA
            let mut img_buffer = ImageBuffer::new(width, height);
            for y in 0..height {
                for x in 0..width {
                    let idx = ((y * width + x) * 4) as usize;
                    let b = data[idx];
                    let g = data[idx + 1];
                    let r = data[idx + 2];
                    let a = data[idx + 3];
                    img_buffer.put_pixel(x, y, Rgba([r, g, b, a]));
                }
            }

            img_buffer
                .save(filename)
                .expect("Failed to save test image");
            println!("Test image saved: {} ({}x{})", filename, width, height);
        }
    }

    /// 获取主显示器句柄
    fn get_primary_monitor() -> HMONITOR {
        unsafe {
            let mut monitor = HMONITOR(std::ptr::null_mut());
            let _ = EnumDisplayMonitors(
                Some(HDC::default()),
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut monitor as *mut _ as isize),
            );
            if monitor.0.is_null() {
                panic!("No monitor found");
            }
            monitor
        }
    }

    unsafe extern "system" fn monitor_enum_proc(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitor_ptr = lparam.0 as *mut HMONITOR;
        *monitor_ptr = hmonitor;
        BOOL(0)
    }
}
