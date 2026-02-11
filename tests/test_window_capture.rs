// 集成测试：按窗口标题截取指定窗口
//
// 修改 TARGET_WINDOW_TITLE 来指定要截取的窗口。
// 如果目标窗口不存在，测试会优雅地跳过（而非失败）。

use hdrcapture::capture::{enable_dpi_awareness, init_capture, CaptureTarget};
use hdrcapture::d3d11::create_d3d11_device;
use hdrcapture::d3d11::texture::TextureReader;

use image::{ImageBuffer, Rgba};
use std::{thread, time::Duration};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

// ---------------------------------------------------------------------------
// 配置：修改这里来指定目标窗口
// ---------------------------------------------------------------------------

/// 目标窗口标题（部分匹配不支持，必须完整标题）
/// 设为 None 表示不指定标题，仅按类名查找
const TARGET_WINDOW_TITLE: Option<&str> = Some("Endfield");

/// 目标窗口类名
/// 设为 None 表示不指定类名，仅按标题查找
/// 常见类名：
///   - "Notepad"          — 记事本
///   - "Chrome_WidgetWin_1" — Chrome 浏览器
///   - "CabinetWClass"    — 文件资源管理器
const TARGET_WINDOW_CLASS: Option<&str> = None;

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

fn find_target_window() -> Option<HWND> {
    unsafe {
        let class_wide: Vec<u16>;
        let class_ptr = match TARGET_WINDOW_CLASS {
            Some(s) => {
                class_wide = s.encode_utf16().chain(std::iter::once(0)).collect();
                windows::core::PCWSTR(class_wide.as_ptr())
            }
            None => windows::core::PCWSTR::null(),
        };

        let title_wide: Vec<u16>;
        let title_ptr = match TARGET_WINDOW_TITLE {
            Some(s) => {
                title_wide = s.encode_utf16().chain(std::iter::once(0)).collect();
                windows::core::PCWSTR(title_wide.as_ptr())
            }
            None => windows::core::PCWSTR::null(),
        };

        FindWindowW(class_ptr, title_ptr).ok()
    }
}

fn save_bgra8_png(data: &[u8], width: u32, height: u32, path: &str) {
    let mut img = ImageBuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let (b, g, r, a) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
            img.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    img.save(path)
        .unwrap_or_else(|e| panic!("Failed to save {}: {}", path, e));
    println!("Saved: {} ({}x{})", path, width, height);
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn test_capture_target_window() {
    enable_dpi_awareness();

    // 1. 查找目标窗口
    let hwnd = match find_target_window() {
        Some(h) => h,
        None => {
            println!(
                "SKIPPED: target window not found (class={:?}, title={:?})",
                TARGET_WINDOW_CLASS, TARGET_WINDOW_TITLE
            );
            return;
        }
    };
    println!("Found target window: HWND={:?}", hwnd);

    // 2. 截图
    let d3d_ctx = create_d3d11_device().unwrap();
    let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

    let target = CaptureTarget::Window(hwnd);
    let capture = init_capture(&d3d_ctx, target).unwrap();
    capture.start().unwrap();
    thread::sleep(Duration::from_millis(200));

    let texture = capture.capture_frame().unwrap();

    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        assert_eq!(desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
        assert!(desc.Width > 0 && desc.Height > 0);
        println!("Window texture: {}x{}", desc.Width, desc.Height);
    }

    let data = reader.read_texture(&texture).unwrap();
    assert!(data.iter().any(|&b| b != 0), "Window capture is all black");

    let (w, h) = unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        (desc.Width, desc.Height)
    };

    save_bgra8_png(data, w, h, "test_window_capture.png");
}
