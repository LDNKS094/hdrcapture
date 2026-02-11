// 捕获管线：target 解析 → WGC 初始化 → 帧捕获 → 纹理回读
//
// 同步模式（方案 A）：每次 capture_frame() 排空积压帧后取帧 → CopyResource + Map。
// fresh=true 时排空后等全新帧（截图），fresh=false 时排空取最后一帧（连续取帧）。
// Frame 生命周期覆盖 CopyResource，确保 DWM 不会覆盖正在读取的 surface。

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use image::{ImageBuffer, Rgba};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;

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
/// let frame = pipeline.capture_frame().unwrap();
/// println!("{}x{}, {} bytes", frame.width, frame.height, frame.data.len());
/// ```
pub struct CapturePipeline {
    _d3d_ctx: D3D11Context,
    capture: WGCCapture,
    reader: TextureReader,
    /// 是否等待全新帧。
    /// - `true`（默认）：排空积压帧后等待 DWM 推送新帧，适合截图场景。
    /// - `false`：排空后直接取最后一帧，适合高频连续取帧场景。
    pub fresh: bool,
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
        let reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            capture,
            reader,
            fresh: true,
            first_call: true,
        })
    }

    /// 轮询等待下一帧（带超时）
    fn wait_frame(
        &self,
        deadline: Instant,
    ) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        loop {
            if let Ok(f) = self.capture.try_get_next_frame() {
                return Ok(f);
            }
            if Instant::now() >= deadline {
                bail!(
                    "Timeout waiting for capture frame ({}ms)",
                    FIRST_FRAME_TIMEOUT.as_millis()
                );
            }
            thread::sleep(Duration::from_millis(1));
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

    /// 捕获一帧，返回 BGRA8 像素数据及尺寸
    ///
    /// 行为取决于 `fresh` 字段：
    /// - `true`：排空积压帧 → 等待全新帧，保证帧是调用之后产生的（多等 ~1 VSync）。
    /// - `false`：排空积压帧 → 取最后一帧，延迟更低但可能拿到稍早的帧。
    ///
    /// 首次调用时跳过排空逻辑，因为 StartCapture() 后的首帧天然是 fresh 的。
    ///
    /// Frame 生命周期覆盖 CopyResource，确保数据安全。
    /// 返回的 `CapturedFrame` 拥有数据所有权，可自由传递到其他线程。
    pub fn capture_frame(&mut self) -> Result<CapturedFrame> {
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;

        // 首次调用：StartCapture() 后的首帧天然是 fresh 的，直接取即可
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(deadline)?;
            return self.read_frame(&frame);
        }

        // 后续调用：排空积压帧，fresh 模式丢弃所有，非 fresh 模式保留最后一帧
        let mut latest = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            if !self.fresh {
                latest = Some(f);
            }
        }

        // 取帧：fresh 模式或池空时等待新帧，否则用排空拿到的最后一帧
        let frame = match latest {
            Some(f) => f,
            None => self.wait_frame(deadline)?,
        };

        // frame 在 read_frame 返回后 drop，buffer 归还给 FramePool
        self.read_frame(&frame)
    }
}
