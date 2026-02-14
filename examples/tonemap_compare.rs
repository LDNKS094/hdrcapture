// A/B test: compare three tone mapping strategies on the same HDR frame.
//
// Captures one HDR frame (RGBA16F), then runs it through:
//   1. DWM-equivalent (hard clip + sRGB)
//   2. Shoulder (linear + rolloff)
//   3. BT.2390 EETF (PQ-space Hermite spline)
//
// Outputs:
//   tests/results/tonemap_dwm.png
//   tests/results/tonemap_shoulder.png
//   tests/results/tonemap_eetf.png
//   tests/results/tonemap_sdr_ref.png  (direct 8-bit reference)
//
// Usage: cargo run --release --example tonemap_compare

use std::fs;
use std::time::Instant;

use hdrcapture::capture::{find_monitor, init_capture, CapturePolicy, CaptureTarget};
use hdrcapture::color::tone_map::ToneMapPass;
use hdrcapture::color::white_level::query_sdr_white_level;
use hdrcapture::color::{ColorFrame, ColorPixelFormat};
use hdrcapture::d3d11::create_d3d11_device;
use hdrcapture::d3d11::texture::TextureReader;
use hdrcapture::pipeline::CapturePipeline;

fn main() {
    fs::create_dir_all("tests/results").ok();

    let d3d = create_d3d11_device().expect("D3D11");
    let hmonitor = find_monitor(0).expect("monitor");
    let sdr_white = query_sdr_white_level(hmonitor);
    println!("SDR white level: {:.1} nits", sdr_white);

    // --- 1. Capture HDR frame (RGBA16F) ---
    let capture = init_capture(
        &d3d,
        CaptureTarget::Monitor(hmonitor),
        CapturePolicy::Hdr,
    )
    .expect("init capture");
    capture.start().expect("start");

    // Wait for first frame
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let wgc_frame = loop {
        if let Ok(f) = capture.try_get_next_frame() {
            break f;
        }
        if std::time::Instant::now() >= deadline {
            panic!("Timeout waiting for frame");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };

    let hdr_texture = hdrcapture::capture::wgc::WGCCapture::frame_to_texture(&wgc_frame)
        .expect("frame_to_texture");

    // Get texture dimensions
    let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
    unsafe { hdr_texture.GetDesc(&mut desc) };
    let width = desc.Width;
    let height = desc.Height;
    println!("Captured: {}x{} RGBA16F", width, height);

    // --- 2. Run three tone mappers ---
    let strategies: &[(&str, &str)] = &[
        ("dwm", hdrcapture::shader::HDR_TONEMAP_HLSL),
        ("shoulder", hdrcapture::shader::HDR_TONEMAP_SHOULDER_HLSL),
        ("eetf", hdrcapture::shader::HDR_TONEMAP_EETF_HLSL),
    ];

    let mut reader = TextureReader::new(d3d.device.clone(), d3d.context.clone());

    for (name, hlsl) in strategies {
        let mut pass = ToneMapPass::with_shader(&d3d.device, &d3d.context, hlsl)
            .expect(&format!("compile {}", name));

        let frame = ColorFrame {
            texture: hdr_texture.clone(),
            width,
            height,
            timestamp: 0.0,
            format: ColorPixelFormat::Rgba16f,
        };

        let t_shader = Instant::now();
        let output = pass.execute(&frame, sdr_white).expect(&format!("execute {}", name));
        let shader_ms = t_shader.elapsed().as_secs_f64() * 1000.0;

        let t_read = Instant::now();
        let data = reader.read_texture(&output).expect("readback");
        let readback_ms = t_read.elapsed().as_secs_f64() * 1000.0;

        let t_save = Instant::now();
        let path = format!("tests/results/tonemap_{}.png", name);
        hdrcapture::image::save(
            std::path::Path::new(&path),
            &data,
            width,
            height,
            ColorPixelFormat::Bgra8,
        )
        .expect("save");
        let save_ms = t_save.elapsed().as_secs_f64() * 1000.0;

        let size = fs::metadata(&path).unwrap().len();
        println!(
            "{:<12} shader={:.2}ms  readback={:.2}ms  save={:.2}ms  size={} bytes",
            name, shader_ms, readback_ms, save_ms, size
        );
    }

    // --- 3. SDR reference (direct 8-bit capture) ---
    let t = Instant::now();
    let mut sdr_pipeline = CapturePipeline::monitor(0, CapturePolicy::Sdr).expect("SDR pipeline");
    let sdr_frame = sdr_pipeline.capture().expect("SDR capture");
    let ms = t.elapsed().as_secs_f64() * 1000.0;

    let path = "tests/results/tonemap_sdr_ref.png";
    sdr_frame.save(path).expect("save SDR ref");
    let size = fs::metadata(path).unwrap().len();
    println!("{:<12} {:.2}ms  {} bytes", "sdr_ref", ms, size);

    println!("\nDone. Compare images in tests/results/tonemap_*.png");
}
