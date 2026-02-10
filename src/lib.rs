// HDR Capture Library
// 解决 Windows HDR 环境下屏幕截图泛白问题

#![cfg(windows)] //如果目标操作系统不是 Windows，就完全忽略
#![allow(dead_code)] // 开发阶段允许未使用的代码

// 模块声明（crate 内部可见）
pub(crate) mod capture;
pub(crate) mod d3d11;
pub(crate) mod pipeline;
pub(crate) mod tonemap;

// Python 绑定（后续 P3 阶段启用）
// mod python;

// 公开 API（暂时为空，后续逐步添加）
// pub use capture::*;

#[cfg(test)]
mod tests {
    use crate::capture::{enable_dpi_awareness, init_capture, CaptureTarget};
    use crate::d3d11::create_d3d11_device;
    use crate::d3d11::texture::TextureReader;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    };
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    /// 内部集成测试：验证 WGC 捕获管线（Device -> Capture -> Texture Readback）
    #[test]
    fn test_wgc_capture_pipeline() {
        use std::thread;
        use std::time::Duration;

        // 1. 准备环境
        // 必须启用 DPI 感知，否则获取的分辨率是逻辑分辨率
        enable_dpi_awareness();

        let d3d_ctx = create_d3d11_device().unwrap();
        let monitor = get_primary_monitor();

        // 2. 初始化捕获会话（使用 CaptureTarget 枚举）
        let target = CaptureTarget::Monitor(monitor);
        let capture = init_capture(&d3d_ctx, target).unwrap();
        println!("✅ WGC 会话初始化成功");
        println!("   HDR 模式: {}", if capture.is_hdr { "是" } else { "否" });

        // 3. 启动捕获
        capture.start().unwrap();
        println!("✅ 捕获已启动，等待帧...");

        // 4. 等待一帧准备好 (100ms 足够大多数情况)
        thread::sleep(Duration::from_millis(100));

        // 5. 捕获一帧
        let texture = capture.capture_frame().unwrap();
        println!("✅ 成功获取帧");

        // 6. 验证纹理格式 (关键步骤)
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);

            println!("📊 纹理信息:");
            println!("   格式: {:?}", desc.Format);
            println!("   尺寸: {}x{}", desc.Width, desc.Height);
            println!("   MipLevels: {}", desc.MipLevels);

            // 根据 HDR 状态验证格式
            let expected_format = if capture.is_hdr {
                DXGI_FORMAT_R16G16B16A16_FLOAT
            } else {
                DXGI_FORMAT_B8G8R8A8_UNORM
            };

            assert_eq!(desc.Format, expected_format, "纹理格式必须与 HDR 状态匹配");
            assert!(desc.Width > 0);
            assert!(desc.Height > 0);
            assert_eq!(desc.MipLevels, 1, "截图纹理不应有 Mipmaps");
        }

        // 7. 回读数据测试
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());
        let data = reader.read_texture(&texture).unwrap();
        println!("✅ 成功回读数据: {} bytes", data.len());

        // 验证数据不是全黑
        let has_data = data.iter().any(|&b| b != 0);
        if has_data {
            println!("   数据验证: 包含非零像素值");
        } else {
            println!("⚠️ 警告: 捕获到的图像全黑 (如果是黑屏则正常)");
        }

        // 8. 保存首帧图像用于人工验证（Step 0.7）
        save_test_image(&texture, &data, &capture, "test_capture.png");

        println!("🎉 WGC 捕获管线测试通过！");
    }

    // --- 测试辅助函数 ---

    /// 保存测试图像（仅用于开发验证）
    /// 将纹理数据转换为 8-bit PNG
    /// 支持 HDR (R16G16B16A16_FLOAT) 和 SDR (B8G8R8A8_UNORM) 格式
    fn save_test_image(
        texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        data: &[u8],
        capture: &crate::capture::wgc::WGCCapture,
        filename: &str,
    ) {
        use half::f16;
        use image::{ImageBuffer, Rgba};

        unsafe {
            let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);

            let width = desc.Width as u32;
            let height = desc.Height as u32;

            // 创建 8-bit RGBA 图像缓冲区
            let mut img_buffer = ImageBuffer::new(width, height);

            if capture.is_hdr {
                // HDR 格式: R16G16B16A16_FLOAT = 8 bytes per pixel (4 channels * 2 bytes)
                let pixels_f16 = std::slice::from_raw_parts(
                    data.as_ptr() as *const f16,
                    (width * height * 4) as usize,
                );

                for y in 0..height {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;

                        // 读取 f16 RGBA 值
                        let r = pixels_f16[idx].to_f32();
                        let g = pixels_f16[idx + 1].to_f32();
                        let b = pixels_f16[idx + 2].to_f32();
                        let a = pixels_f16[idx + 3].to_f32();

                        // 简单 clamp 到 [0, 1] 并转换为 u8
                        // 注意：此时图像可能仍然泛白（因为还没有色调映射）
                        let r_u8 = (r.clamp(0.0, 1.0) * 255.0) as u8;
                        let g_u8 = (g.clamp(0.0, 1.0) * 255.0) as u8;
                        let b_u8 = (b.clamp(0.0, 1.0) * 255.0) as u8;
                        let a_u8 = (a.clamp(0.0, 1.0) * 255.0) as u8;

                        img_buffer.put_pixel(x, y, Rgba([r_u8, g_u8, b_u8, a_u8]));
                    }
                }
            } else {
                // SDR 格式: B8G8R8A8_UNORM = 4 bytes per pixel
                for y in 0..height {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;

                        // BGRA 格式，需要转换为 RGBA
                        let b = data[idx];
                        let g = data[idx + 1];
                        let r = data[idx + 2];
                        let a = data[idx + 3];

                        img_buffer.put_pixel(x, y, Rgba([r, g, b, a]));
                    }
                }
            }

            // 保存为 PNG
            img_buffer
                .save(filename)
                .expect("Failed to save test image");
            println!("📸 测试图像已保存: {}", filename);
            println!("   尺寸: {}x{}", width, height);
            println!(
                "   格式: {}",
                if capture.is_hdr {
                    "HDR (16-bit)"
                } else {
                    "SDR (8-bit)"
                }
            );
            if capture.is_hdr {
                println!("   ⚠️  注意：HDR 图像可能泛白（正常现象，P1 阶段会修复）");
            }
        }
    }

    /// 极简版获取主显示器句柄
    fn get_primary_monitor() -> HMONITOR {
        unsafe {
            let mut monitor = HMONITOR(std::ptr::null_mut());
            let _ = EnumDisplayMonitors(
                Some(HDC::default()),
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut monitor as *mut _ as isize),
            );
            // 如果枚举失败（monitor仍为null），这在测试环境下会 panic，也是预期的
            if monitor.0.is_null() {
                panic!("无法找到任何显示器");
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
        // 返回 FALSE (0) 停止枚举，因为我们只要第一个
        BOOL(0)
    }
}
