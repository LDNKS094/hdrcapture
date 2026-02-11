// 集成测试：按进程名截取指定窗口
//
// 修改 TARGET_PROCESS 来指定要截取的进程。
// 如果目标窗口不存在，测试会优雅地跳过。

use hdrcapture::capture::{enable_dpi_awareness, find_window, init_capture, CaptureTarget};
use hdrcapture::d3d11::create_d3d11_device;
use hdrcapture::d3d11::texture::TextureReader;

use image::{ImageBuffer, Rgba};
use std::{thread, time::Duration};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

// ---------------------------------------------------------------------------
// 配置：修改这里来指定目标窗口
// ---------------------------------------------------------------------------

/// 目标进程名（如 "notepad.exe"、"chrome.exe"）
const TARGET_PROCESS: &str = "Endfield.exe";

/// 窗口索引（同一进程有多个窗口时，0 = 第一个）
const TARGET_INDEX: usize = 0;

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

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

    let hwnd = match find_window(TARGET_PROCESS, Some(TARGET_INDEX)) {
        Ok(h) => h,
        Err(_) => {
            println!(
                "SKIPPED: no window found for \"{}\" index {}",
                TARGET_PROCESS, TARGET_INDEX
            );
            return;
        }
    };
    println!(
        "Found window: HWND={:?} (process={}, index={})",
        hwnd, TARGET_PROCESS, TARGET_INDEX
    );

    let d3d_ctx = create_d3d11_device().unwrap();
    let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

    let capture = init_capture(&d3d_ctx, CaptureTarget::Window(hwnd)).unwrap();
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

    save_bgra8_png(data, w, h, "tests/results/window_capture.png");
}
