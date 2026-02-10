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
    use crate::capture::wgc::{
        create_capture_item_for_monitor, enable_dpi_awareness, init_capture,
    };
    use crate::d3d11::create_d3d11_device;
    use crate::d3d11::texture::TextureReader;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;
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
        let item = create_capture_item_for_monitor(monitor).unwrap();

        // 2. 初始化捕获会话
        let capture = init_capture(&d3d_ctx, item).unwrap();
        println!("✅ WGC 会话初始化成功");

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
            println!("   格式: {:?} (预期: R16G16B16A16_FLOAT)", desc.Format);
            println!("   尺寸: {}x{}", desc.Width, desc.Height);
            println!("   MipLevels: {}", desc.MipLevels);

            assert_eq!(
                desc.Format, DXGI_FORMAT_R16G16B16A16_FLOAT,
                "纹理格式必须是 FP16"
            );
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

        println!("🎉 WGC 捕获管线测试通过！");
    }

    // --- 测试辅助函数 ---

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
