// 捕获管线：target 解析 → WGC 初始化 → 帧捕获 → 纹理回读
//
// 提供两种取帧模式：
// - capture()：排空积压帧后等全新帧，适合截图（保证帧是调用之后产生的）
// - grab()：排空积压帧取最后一帧，适合连续取帧（延迟更低）
// Frame 生命周期覆盖 CopyResource，确保 DWM 不会覆盖正在读取的 surface。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::capture::wgc::{CaptureTarget, WGCCapture};
use crate::capture::{enable_dpi_awareness, find_monitor, find_window, init_capture};
use crate::d3d11::texture::TextureReader;
use crate::d3d11::{create_d3d11_device, D3D11Context};

/// 首帧等待超时时间
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// 等待新帧的短超时（~3 VSyncs at 60Hz）
/// 屏幕活跃时新帧在 1 VSync 内到达；超时说明屏幕静止，应使用 fallback。
const FRESH_FRAME_TIMEOUT: Duration = Duration::from_millis(50);

/// 一帧捕获结果
pub struct CapturedFrame {
    /// BGRA8 像素数据，长度 = width * height * 4
    pub data: Vec<u8>,
    /// 帧宽度（像素）
    pub width: u32,
    /// 帧高度（像素）
    pub height: u32,
    /// 帧时间戳（秒），相对于系统启动时间（QPC）
    pub timestamp: f64,
}

impl CapturedFrame {
    /// 保存为 PNG 文件（快速压缩）
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let file = std::fs::File::create(path.as_ref())?;
        let writer = std::io::BufWriter::new(file);
        let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);

        // BGRA → RGBA
        let mut rgba = self.data.clone();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        encoder.write_image(&rgba, self.width, self.height, ExtendedColorType::Rgba8)?;
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
    /// Timestamp from the last successful read_frame(), for static-screen fallback.
    /// Pixel data and dimensions live in TextureReader (persists across calls).
    cached_timestamp: Option<f64>,
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
            cached_timestamp: None,
        })
    }

    /// Wait for the next frame from the pool, with timeout.
    /// Returns None on timeout instead of error.
    fn try_wait_frame(
        &self,
        timeout: Duration,
    ) -> Result<Option<windows::Graphics::Capture::Direct3D11CaptureFrame>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(f) = self.capture.try_get_next_frame() {
                return Ok(Some(f));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let timeout_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
            if self.capture.wait_for_frame(timeout_ms).is_err() {
                return Ok(None);
            }
        }
    }

    /// Wait for the next frame, returning error on timeout.
    fn wait_frame(
        &self,
        timeout: Duration,
    ) -> Result<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        self.try_wait_frame(timeout)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Timeout waiting for capture frame ({}ms)",
                timeout.as_millis()
            )
        })
    }

    /// 从 WGC 帧中提取纹理并回读像素数据
    fn read_frame(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    ) -> Result<CapturedFrame> {
        // 提取时间戳（SystemRelativeTime，100ns 精度，转换为秒）
        let timestamp = frame.SystemRelativeTime()?.Duration as f64 / 10_000_000.0;

        let texture = WGCCapture::frame_to_texture(frame)?;

        let (width, height) = unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            (desc.Width, desc.Height)
        };

        let data = self.reader.read_texture(&texture)?;

        // Cache timestamp for static-screen fallback.
        // Pixel data and dimensions are already in TextureReader.
        self.cached_timestamp = Some(timestamp);

        Ok(CapturedFrame {
            data,
            width,
            height,
            timestamp,
        })
    }

    /// Build a CapturedFrame from the cached pixel data in TextureReader.
    /// Only called on the fallback path (static screen, no new frames available).
    fn build_cached_frame(&self) -> Result<CapturedFrame> {
        let timestamp = self
            .cached_timestamp
            .expect("build_cached_frame called without cached data");
        let (width, height) = self.reader.last_dimensions();
        let data = self.reader.clone_buffer();
        if data.is_empty() {
            bail!("No cached frame data available");
        }
        Ok(CapturedFrame {
            data,
            width,
            height,
            timestamp,
        })
    }

    /// 截图模式：捕获一帧全新的画面
    ///
    /// 排空积压帧 → 等待 DWM 推送新帧，保证返回的帧是调用之后产生的。
    /// 首次调用时跳过排空（首帧天然是 fresh 的）。
    /// 屏幕静止时使用 fallback 避免长时间阻塞。
    ///
    /// 适合截图场景，延迟 ~1 VSync。
    pub fn capture(&mut self) -> Result<CapturedFrame> {
        // 首次调用：StartCapture() 后的首帧天然是 fresh 的，直接取即可
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
            return self.read_frame(&frame);
        }

        // Drain pool, keep last frame as fallback
        let mut fallback = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            fallback = Some(f);
        }

        // Try to get a fresh frame with short timeout
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
            // New frame arrived — use it (discard fallback if any)
            return self.read_frame(&fresh);
        }

        // Timeout — screen is likely static
        if let Some(fb) = fallback {
            // Use the last drained frame (most recent available)
            return self.read_frame(&fb);
        }

        // Pool was empty AND no new frame — use cached data
        if self.cached_timestamp.is_some() {
            return self.build_cached_frame();
        }

        // No cache either — true cold start edge case, full timeout
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
        self.read_frame(&frame)
    }

    /// 连续取帧模式：抓取最新可用帧
    ///
    /// 排空积压帧，保留最后一帧；池空时等待新帧。
    /// 返回的帧可能是调用之前产生的，但延迟更低。
    /// 屏幕静止时使用 fallback 避免长时间阻塞。
    ///
    /// 适合高频连续取帧场景。
    pub fn grab(&mut self) -> Result<CapturedFrame> {
        // 首次调用：直接取首帧
        if self.first_call {
            self.first_call = false;
            let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
            return self.read_frame(&frame);
        }

        // Drain pool, keep last frame
        let mut latest = None;
        while let Ok(f) = self.capture.try_get_next_frame() {
            latest = Some(f);
        }

        // Got a buffered frame — use it
        if let Some(f) = latest {
            return self.read_frame(&f);
        }

        // Pool empty — try short wait for new frame
        if let Some(fresh) = self.try_wait_frame(FRESH_FRAME_TIMEOUT)? {
            return self.read_frame(&fresh);
        }

        // Timeout — screen is likely static, use cached data
        if self.cached_timestamp.is_some() {
            return self.build_cached_frame();
        }

        // No cache — full timeout (should not happen in normal usage)
        let frame = self.wait_frame(FIRST_FRAME_TIMEOUT)?;
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
