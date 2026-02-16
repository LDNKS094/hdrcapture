use half::f16;
use numpy::ndarray::Array3;
use numpy::{IntoPyArray, PyArrayMethods};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::color::ColorPixelFormat;
use crate::pipeline;

/// Single frame capture result
///
/// Holds BGRA8 pixel data, provides save and numpy conversion functionality.
/// `save()` writes directly to disk on the Rust side, bypassing Python, for optimal performance.
#[pyclass]
pub(crate) struct CapturedFrame {
    pub(super) inner: pipeline::CapturedFrame,
}

#[pymethods]
impl CapturedFrame {
    /// Frame width (pixels)
    #[getter]
    fn width(&self) -> u32 {
        self.inner.width
    }

    /// Frame height (pixels)
    #[getter]
    fn height(&self) -> u32 {
        self.inner.height
    }

    /// Frame timestamp (seconds), relative to system boot time
    #[getter]
    fn timestamp(&self) -> f64 {
        self.inner.timestamp
    }

    /// Pixel format string ("bgra8" or "rgba16f")
    #[getter]
    fn format(&self) -> &'static str {
        match self.inner.format {
            ColorPixelFormat::Bgra8 => "bgra8",
            ColorPixelFormat::Rgba16f => "rgba16f",
        }
    }

    /// Save frame to file (format determined by extension).
    ///
    /// Supported formats:
    ///   - .png .bmp .jpg .tiff — standard formats (BGRA8 / SDR only)
    ///   - .jxr — JPEG XR (both BGRA8 and RGBA16F / HDR)
    ///   - .exr — OpenEXR (both BGRA8 and RGBA16F / HDR)
    ///
    /// Releases GIL during encoding, doesn't block other Python threads.
    fn save(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        let inner = &self.inner;
        let path = path.to_string();
        py.detach(|| inner.save(&path))
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// Convert to numpy array.
    ///
    /// Returns:
    ///     numpy.ndarray: shape (H, W, 4).
    ///       - bgra8: dtype uint8, BGRA channel order
    ///       - rgba16f: dtype float16, RGBA channel order
    fn ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.to_ndarray(py)
    }

    /// numpy __array__ protocol, enables np.array(frame) to work automatically
    #[pyo3(signature = (dtype=None, copy=None))]
    fn __array__<'py>(
        &self,
        py: Python<'py>,
        dtype: Option<Bound<'py, PyAny>>,
        copy: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _ = (dtype, copy);
        self.to_ndarray(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "CapturedFrame({}x{}, format={}, timestamp={:.3}s)",
            self.inner.width,
            self.inner.height,
            self.format(),
            self.inner.timestamp
        )
    }
}

impl CapturedFrame {
    /// Internal shared numpy conversion logic.
    ///
    /// - bgra8 → (H, W, 4) uint8
    /// - rgba16f → (H, W, 4) float16
    fn to_ndarray<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let h = self.inner.height as usize;
        let w = self.inner.width as usize;
        let data = self.inner.data.as_slice();

        match self.inner.format {
            ColorPixelFormat::Bgra8 => {
                let array = Array3::from_shape_vec((h, w, 4), data.to_vec())
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
                let pyarray = array.into_pyarray(py);
                pyarray
                    .try_readwrite()
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
                    .make_nonwriteable();
                Ok(pyarray.into_any())
            }
            ColorPixelFormat::Rgba16f => {
                // SAFETY: f16 is #[repr(transparent)] over u16 (2 bytes).
                // data length is guaranteed to be h * w * 8 by the capture pipeline.
                let f16_slice: &[f16] = unsafe {
                    std::slice::from_raw_parts(data.as_ptr() as *const f16, data.len() / 2)
                };
                let array = Array3::from_shape_vec((h, w, 4), f16_slice.to_vec())
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
                let pyarray = array.into_pyarray(py);
                pyarray
                    .try_readwrite()
                    .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
                    .make_nonwriteable();
                Ok(pyarray.into_any())
            }
        }
    }
}
