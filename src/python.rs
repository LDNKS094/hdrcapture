// PyO3 Python 绑定层
//
// 两个 PyClass：
// - CapturedFrame：帧容器，持有像素数据，提供 save() 和 numpy 转换
// - Capture：可复用管线，包装 CapturePipeline
//
// Pipeline 保持纯 Rust，不依赖 pyo3/numpy。
// 本模块负责跨语言桥接和错误映射。

use numpy::ndarray::Array3;
use numpy::{IntoPyArray, PyArray3};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::pipeline;

// ---------------------------------------------------------------------------
// CapturedFrame — 帧容器
// ---------------------------------------------------------------------------

/// 一帧捕获结果
///
/// 持有 BGRA8 像素数据，提供保存和 numpy 转换功能。
/// `save()` 在 Rust 侧直接写盘，不经过 Python，性能最优。
#[pyclass]
struct CapturedFrame {
    inner: pipeline::CapturedFrame,
}

#[pymethods]
impl CapturedFrame {
    /// 帧宽度（像素）
    #[getter]
    fn width(&self) -> u32 {
        self.inner.width
    }

    /// 帧高度（像素）
    #[getter]
    fn height(&self) -> u32 {
        self.inner.height
    }

    /// 帧时间戳（秒），相对于系统启动时间
    #[getter]
    fn timestamp(&self) -> f64 {
        self.inner.timestamp
    }

    /// 保存为图片文件（格式由扩展名决定，如 .png、.bmp、.jpg）
    ///
    /// Rust 侧直接执行 BGRA→RGBA 转换并写盘，不经过 Python 内存。
    fn save(&self, path: &str) -> PyResult<()> {
        self.inner
            .save(path)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// 转换为 ndarray
    ///
    /// Returns:
    ///     numpy.ndarray: shape (H, W, 4), dtype uint8, BGRA 通道顺序
    fn ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        self.to_ndarray(py)
    }

    /// numpy __array__ 协议，使 np.array(frame) 自动工作
    #[pyo3(signature = (dtype=None, copy=None))]
    fn __array__<'py>(
        &self,
        py: Python<'py>,
        dtype: Option<Bound<'py, PyAny>>,
        copy: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let _ = (dtype, copy);
        self.to_ndarray(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "CapturedFrame({}x{}, timestamp={:.3}s)",
            self.inner.width, self.inner.height, self.inner.timestamp
        )
    }
}

impl CapturedFrame {
    /// 内部共用的 numpy 转换逻辑
    fn to_ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let array = Array3::from_shape_vec((h, w, 4), self.inner.data.clone())
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(array.into_pyarray(py))
    }
}

// ---------------------------------------------------------------------------
// Capture — 可复用管线
// ---------------------------------------------------------------------------

/// 屏幕/窗口捕获管线
///
/// 通过类方法构造：
///   cap = Capture.monitor(0)
///   cap = Capture.window("notepad.exe")
///
/// 支持 context manager：
///   with Capture.monitor(0) as cap:
///       frame = cap.capture()
#[pyclass(unsendable)]
struct Capture {
    pipeline: Option<pipeline::CapturePipeline>,
}

impl Capture {
    /// 获取 pipeline 引用，close() 后报错
    fn get_pipeline(&mut self) -> PyResult<&mut pipeline::CapturePipeline> {
        self.pipeline
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("Capture is closed"))
    }
}

#[pymethods]
impl Capture {
    /// 按显示器索引创建捕获管线
    ///
    /// Args:
    ///     index: 显示器索引，默认 0
    #[staticmethod]
    #[pyo3(signature = (index=0))]
    fn monitor(index: usize) -> PyResult<Self> {
        let pipeline = pipeline::CapturePipeline::monitor(index)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self {
            pipeline: Some(pipeline),
        })
    }

    /// 按进程名创建窗口捕获管线
    ///
    /// Args:
    ///     process_name: 进程名（如 "notepad.exe"）
    ///     index: 同名进程的窗口序号，默认 0
    #[staticmethod]
    #[pyo3(signature = (process_name, index=None))]
    fn window(process_name: &str, index: Option<usize>) -> PyResult<Self> {
        let pipeline = pipeline::CapturePipeline::window(process_name, index)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self {
            pipeline: Some(pipeline),
        })
    }

    /// 截图模式：捕获一帧全新的画面
    ///
    /// 排空积压帧后等待 DWM 推送新帧，保证返回的帧是调用之后产生的。
    fn capture(&mut self) -> PyResult<CapturedFrame> {
        let p = self.get_pipeline()?;
        let frame = p
            .capture()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(CapturedFrame { inner: frame })
    }

    /// 连续取帧模式：抓取最新可用帧
    ///
    /// 排空积压帧保留最后一帧，池空时等待新帧。延迟更低。
    fn grab(&mut self) -> PyResult<CapturedFrame> {
        let p = self.get_pipeline()?;
        let frame = p
            .grab()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(CapturedFrame { inner: frame })
    }

    /// 释放捕获资源
    fn close(&mut self) {
        self.pipeline = None;
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        self.close();
        false // 不吞异常
    }

    fn __repr__(&self) -> String {
        if self.pipeline.is_some() {
            "Capture(active)".to_string()
        } else {
            "Capture(closed)".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// 模块级函数
// ---------------------------------------------------------------------------

/// 一行截图：捕获指定显示器的当前画面
///
/// 内部创建并销毁 pipeline，冷启动 ~79ms。
/// 如需多次截图，请使用 Capture 类复用管线。
///
/// Args:
///     monitor: 显示器索引，默认 0
///
/// Returns:
///     CapturedFrame: 帧容器，可 save() 或转 numpy
#[pyfunction]
#[pyo3(signature = (monitor=0))]
fn screenshot(monitor: usize) -> PyResult<CapturedFrame> {
    let frame = pipeline::screenshot(monitor)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(CapturedFrame { inner: frame })
}

/// HDR-aware screen capture library for Windows
#[pymodule]
fn hdrcapture(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CapturedFrame>()?;
    m.add_class::<Capture>()?;
    m.add_function(wrap_pyfunction!(screenshot, m)?)?;
    Ok(())
}
