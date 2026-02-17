use super::*;
use crate::capture::WindowSelector;
use windows::Win32::Foundation::HWND;

impl CapturePipeline {
    pub(super) fn frame_bytes(width: u32, height: u32, format: ColorPixelFormat) -> usize {
        let bpp = match format {
            ColorPixelFormat::Bgra8 => 4,
            ColorPixelFormat::Rgba16f => 8,
        };
        width as usize * height as usize * bpp
    }

    pub(super) fn color_format(format: DXGI_FORMAT) -> Result<ColorPixelFormat> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Ok(ColorPixelFormat::Bgra8),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(ColorPixelFormat::Rgba16f),
            _ => bail!("Unsupported DXGI_FORMAT for color pipeline: {:?}", format),
        }
    }

    /// Create capture pipeline by monitor index
    ///
    /// Indices are ordered by system enumeration, not guaranteed that `0` is the primary monitor.
    pub fn monitor(index: usize, policy: CapturePolicy) -> Result<Self> {
        enable_dpi_awareness();
        let hmonitor = find_monitor(index)?;
        let sdr_white_nits = white_level::query_sdr_white_level(hmonitor);
        Self::new(
            CaptureTarget::Monitor(hmonitor),
            policy,
            sdr_white_nits,
            false,
        )
    }

    /// Create window capture pipeline by selector inputs.
    ///
    /// Priority: `hwnd` > `pid` > `process`.
    ///
    /// `index` is the ranked window index within the selected candidate set.
    /// When `index` is `None`, the highest-ranked window is selected.
    /// `headless` controls whether to crop the title bar and borders (default: true).
    pub fn window(
        process: Option<&str>,
        pid: Option<u32>,
        hwnd: Option<isize>,
        index: Option<usize>,
        policy: CapturePolicy,
        headless: bool,
    ) -> Result<Self> {
        enable_dpi_awareness();
        let selector = if let Some(raw_hwnd) = hwnd {
            WindowSelector::Hwnd(HWND(raw_hwnd as *mut core::ffi::c_void))
        } else if let Some(pid) = pid {
            WindowSelector::Pid(pid)
        } else if let Some(process) = process {
            WindowSelector::Process(process.to_string())
        } else {
            bail!("window target requires one of: hwnd, pid, process");
        };

        let hwnd = find_window(selector, index)?;
        let hmonitor = unsafe {
            windows::Win32::Graphics::Gdi::MonitorFromWindow(
                hwnd,
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            )
        };
        let sdr_white_nits = white_level::query_sdr_white_level(hmonitor);
        Self::new(
            CaptureTarget::Window(hwnd),
            policy,
            sdr_white_nits,
            headless,
        )
    }

    fn new(
        target: CaptureTarget,
        policy: CapturePolicy,
        sdr_white_nits: f32,
        headless: bool,
    ) -> Result<Self> {
        let d3d_ctx = create_d3d11_device()?;
        let capture = init_capture(&d3d_ctx, target, policy)?;
        let target_hdr = capture.is_hdr();
        capture.start()?;
        // Create reader after start() to let DWM start preparing first frame as early as possible
        let mut reader = TextureReader::new(d3d_ctx.device.clone(), d3d_ctx.context.clone());

        // Pre-create Staging Texture to avoid ~11ms creation overhead on first frame readback.
        // Hdr mode outputs R16G16B16A16_FLOAT (8 bpp); Auto/Sdr output BGRA8 (4 bpp).
        let (w, h) = capture.pool_size();
        let (staging_format, bpp) = if policy == CapturePolicy::Hdr {
            (DXGI_FORMAT_R16G16B16A16_FLOAT, 8)
        } else {
            (DXGI_FORMAT_B8G8R8A8_UNORM, 4)
        };
        reader.ensure_staging_texture(w, h, staging_format)?;
        let output_frame_bytes = w as usize * h as usize * bpp;
        let output_pool = ElasticBufferPool::new(output_frame_bytes);

        // Create tone-map pass only for Auto (may need HDR->SDR conversion)
        let tone_map_pass = if policy == CapturePolicy::Auto {
            Some(ToneMapPass::new(&d3d_ctx.device, &d3d_ctx.context)?)
        } else {
            None
        };

        Ok(Self {
            _d3d_ctx: d3d_ctx,
            policy,
            capture,
            reader,
            output_pool,
            output_frame_bytes,
            first_call: true,
            cached_frame: None,
            tone_map_pass,
            sdr_white_nits,
            target_hdr,
            headless,
            crop_texture: None,
            force_fresh: false,
            _not_send_sync: PhantomData,
        })
    }
}
