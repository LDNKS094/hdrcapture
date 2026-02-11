// 捕获管线：target 解析 → WGC 初始化 → 帧捕获 → 纹理回读
//
// 同步模式（方案 A）：每次 capture_frame() 执行排空 + CopyResource + Map。
// Frame 生命周期覆盖 CopyResource，确保 DWM 不会覆盖正在读取的 surface。

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;

use crate::capture::wgc::{CaptureTarget, WGCCapture};
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};

/// 首帧等待超时时间
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// 一帧捕获结果
pub struct CapturedFrame<'a> {
    /// BGRA8 像素数据，长度 = width * height * 4
    pub data: &'a [u8],
    /// 帧宽度（像素）
    pub width: u32,
    /// 帧高度（像素）
    pub height: u32,
}

/// 捕获管线
///
/// 封装 D3D11 设备、WGC 捕获会话、纹理回读器，提供一行代码截图的能力。
///
/// # Examples
/// ```no_run
/// # use hdrcapture::pipeline::CapturePipeline;
/// let mut pipeline = CapturePipeline::monitor(0).unwrap();
/// let frame = pipeline.capture_frame().unwrap();
/// println!("{}x{}, {} bytes", frame.width, frame.height, frame.data.len());
/// ```
pub struct CapturePipeline {
    _d3d_ctx: D3D11Context,
    capture: WGCCapture,
    reader: TextureReader,
    width: u32,
    height: u32,
}

impl CapturePipeline {
    /// 按显示器索引创建捕获管线
    ///
    /// 索引按系统枚举顺序排列，不保证 `0` 为主显示器。
    pub fn monitor(index: usize) -> Result<Self> {
        enable_dpi_awareness();
        let hmonitor = find_monitor(index)?;
        Self::new(CaptureTarget::Monitor(hmonitor))
    }

    /// 按进程名创建窗口捕获管线
    ///
    /// `index` 为同名进程的窗口序号，默认 0（第一个匹配窗口）。
    pub fn window(process_name: &str, index: Option<usize>) -> Result<Self> {
        enable_dpi_awareness();
        let hwnd = find_window(process_name, index)?;
        Self::new(CaptureTarget::Window(hwnd))
    }

    fn new(target: CaptureTarget) -> Result<Self> {
        let d3d_ctx = create_d3d11_device()?;
        let reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());
        let capture = init_capture(&d3d_ctx, target)?;
        capture.start()?;

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            capture,
            reader,
            width: 0,
            height: 0,
        })
    }

    /// 捕获一帧，返回 BGRA8 像素数据及尺寸
    ///
    /// 内部执行排空策略：循环取帧直到池空，只保留最新帧。
    /// Frame 生命周期覆盖 CopyResource，确保数据安全。
    ///
    /// 首次调用时会等待 DWM 推送第一帧（最多 1 秒超时）。
    /// 返回的数据有效期到下一次 `capture_frame()` 调用为止。
    pub fn capture_frame(&mut self) -> Result<CapturedFrame<'_>> {
        // 排空 FramePool，保留最新帧
        // 首次调用时池可能为空（StartCapture 后 DWM 尚未推帧），需要重试
        let mut latest_frame = None;
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;

        loop {
            // 尝试排空池中所有帧
            loop {
                match self.capture.try_get_next_frame() {
                    Ok(frame) => latest_frame = Some(frame),
                    Err(_) => break,
                }
            }

            if latest_frame.is_some() {
                break;
            }

            // 池空且无帧：首次调用等待，后续调用不应发生
            if Instant::now() >= deadline {
                bail!(
                    "Timeout waiting for capture frame ({}ms)",
                    FIRST_FRAME_TIMEOUT.as_millis()
                );
            }
            thread::sleep(Duration::from_millis(1));
        }

        let frame = latest_frame.unwrap();

        // 从 frame 提取 texture（frame 仍然存活）
        let texture = WGCCapture::frame_to_texture(&frame)?;

        // 更新尺寸
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            self.width = desc.Width;
            self.height = desc.Height;
        }

        // CopyResource + Map（frame 仍然存活，surface 安全）
        let data = self.reader.read_texture(&texture)?;

        // frame 在此处 drop，buffer 归还给 FramePool
        Ok(CapturedFrame {
            data,
            width: self.width,
            height: self.height,
        })
    }
}
