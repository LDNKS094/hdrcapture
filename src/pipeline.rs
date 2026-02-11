// 捕获管线：target 解析 → WGC 初始化 → 帧捕获 → 纹理回读
//
// 提供两种取帧模式：
// - capture()：排空积压帧后等全新帧，适合截图（保证帧是调用之后产生的）
// - grab()：排空积压帧取最后一帧，适合连续取帧（延迟更低）
// Frame 生命周期覆盖 CopyResource，确保 DWM 不会覆盖正在读取的 surface。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use image::{ImageBuffer, Rgba};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::capture::wgc::{CaptureTarget, WGCCapture};
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};

/// 首帧等待超时时间
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// 一帧捕获结果
pub struct CapturedFrame {
    /// BGRA8 像素数据，长度 = width * height * 4
    pub data: Vec<u8>,
    /// 帧宽度（像素）
    pub width: u32,
    /// 帧高度（像素）
    pub height: u32,
}

impl CapturedFrame {
    /// 保存为图片文件（格式由扩展名决定，如 .png、.bmp、.jpg）
    ///
    /// 内部执行 BGRA → RGBA 通道转换。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut img = ImageBuffer::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let i = ((y * self.width + x) * 4) as usize;
                let (b, g, r, a) = (
                    self.data[i],
                    self.data[i + 1],
                    self.data[i + 2],
                    self.data[i + 3],
                );
                img.put_pixel(x, y, Rgba([r, g, b, a]));
            }
        }
        img.save(path.as_ref())?;
        Ok(())
    }
}

/// 捕获管线
///
/// 封装 D3D11 设备、WGC 捕获会话、纹理回读器，提供一行代码截图的能力。
///
/// # Examples
/// ```no_run
/// # use hdrcapture::pipeline::CapturePipeline;
/// let mut pipeline = CapturePipeline::monitor(0).unwrap();
/// let frame = pipeline.capture().unwrap();
/// println!("{}x{}, {} bytes", frame.width, frame.height, frame.data.len());
/// ```
pub struct CapturePipeline {
    _d3d_ctx: D3D11Context,
    capture: WGCCapture,
    reader: TextureReader,
    /// 首次调用标记。StartCapture() 后的首帧天然是 fresh 的，
    /// 无需 drain-discard，直接取即可省 ~1 VSync。
    first_call: bool,
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
        let capture = init_capture(&d3d_ctx, target)?;
        capture.start()?;
        // start() 之后再创建 reader，让 DWM 尽早开始准备首帧
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // 预创建 Staging Texture，避免首次 read_frame 的 ~11ms 创建开销
        let (w, h) = capture.target_size();
        reader.ensure_staging_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?;

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            capture,
            reader,
            first_call: true,
        })
    }

    /// 等待下一帧（带超时），使用内核事件唤醒
    fn wait_frame(
        &self,
        deadline: Instant,
    ) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        loop {
            if let Ok(f) = self.capture.try_get_next_frame() {
                return Ok(f);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "Timeout waiting for capture frame ({}ms)",
                    FIRST_FRAME_TIMEOUT.as_millis()
                );
            }
            // 内核级等待，不消耗 CPU，唤醒延迟 ~0ms
            let timeout_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
            self.capture.wait_for_frame(timeout_ms)?;
        }
    }

    /// 从 WGC 帧中提取纹理并回读像素数据
    fn read_frame(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    ) -> Result<CapturedFrame> {
        let texture = WGCCapture::frame_to_texture(frame)?;

        let (width, height) = unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            (desc.Width, desc.Height)
        };

        let data = self.reader.read_texture(&texture)?;
        Ok(CapturedFrame {
            data,
            width,
            height,
        })
    }

    /// 截图模式：捕获一帧全新的画面
    ///
    /// 排空积压帧 → 等待 DWM 推送新帧，保证返回的帧是调用之后产生的。
    /// 首次调用时跳过排空（首帧天然是 fresh 的）。
    ///
    /// 适合截图场景，延迟 ~1 VSync。
    pub fn capture(&mut self) -> Result<CapturedFrame> {
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;

        // 首次调用：StartCapture() 后的首帧天然是 fresh 的，直接取即可
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(deadline)?;
            return self.read_frame(&frame);
        }

        // 排空积压帧（全部丢弃）
        while self.capture.try_get_next_frame().is_ok() {}

        // 等待全新帧
        let frame = self.wait_frame(deadline)?;
        self.read_frame(&frame)
    }

    /// 连续取帧模式：抓取最新可用帧
    ///
    /// 排空积压帧，保留最后一帧；池空时等待新帧。
    /// 返回的帧可能是调用之前产生的，但延迟更低。
    ///
    /// 适合高频连续取帧场景。
    pub fn grab(&mut self) -> Result<CapturedFrame> {
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;

        // 首次调用：直接取首帧
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(deadline)?;
            return self.read_frame(&frame);
        }

        // 排空积压帧，保留最后一帧
        let mut latest = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            latest = Some(f);
        }

        let frame = match latest {
            Some(f) => f,
            None => self.wait_frame(deadline)?,
        };

        self.read_frame(&frame)
    }
}

/// 一行截图：创建管线 → 捕获一帧 → 返回
///
/// 适合只需要截一张图的场景。内部创建并销毁 pipeline，
/// 冷启动 ~79ms（含 D3D11 设备创建 + WGC 会话创建 + 首帧等待）。
///
/// 如需多次截图，请使用 `CapturePipeline` 复用管线。
pub fn screenshot(monitor_index: usize) -> Result<CapturedFrame> {
    let mut pipeline = CapturePipeline::monitor(monitor_index)?;
    pipeline.capture()
}
