// 集成测试：按索引截取每个监视器

use hdrcapture::capture::{enable_dpi_awareness, find_monitor, init_capture, CaptureTarget};
use hdrcapture::d3d11::create_d3d11_device;
use hdrcapture::d3d11::texture::TextureReader;

use image::{ImageBuffer, Rgba};
use std::{thread, time::Duration};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

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

/// 截取指定索引的监视器并保存 PNG
fn capture_monitor(index: usize) {
    enable_dpi_awareness();

    let hmonitor = find_monitor(index).unwrap();
    let d3d_ctx = create_d3d11_device().unwrap();
    let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

    let capture = init_capture(&d3d_ctx, CaptureTarget::Monitor(hmonitor)).unwrap();
    capture.start().unwrap();
    thread::sleep(Duration::from_millis(200));

    let texture = capture.capture_frame().unwrap();

    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        assert_eq!(desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
        assert!(desc.Width > 0 && desc.Height > 0);
        println!("Monitor {}: {}x{}", index, desc.Width, desc.Height);
    }

    let data = reader.read_texture(&texture).unwrap();
    assert!(
        data.iter().any(|&b| b != 0),
        "Monitor {} captured all black",
        index
    );

    let (w, h) = unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        (desc.Width, desc.Height)
    };

    save_bgra8_png(data, w, h, &format!("test_monitor_{}.png", index));
}

#[test]
fn test_capture_monitor_0() {
    capture_monitor(0);
}

#[test]
fn test_capture_monitor_1() {
    // 第二个监视器可能不存在，跳过
    if find_monitor(1).is_err() {
        println!("SKIPPED: only one monitor detected");
        return;
    }
    capture_monitor(1);
}

#[test]
fn test_capture_monitor_2() {
    // 第三个监视器可能不存在，跳过
    if find_monitor(2).is_err() {
        println!("SKIPPED: only one monitor detected");
        return;
    }
    capture_monitor(2);
}
