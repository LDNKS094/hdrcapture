use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::pipeline;

pub(super) fn parse_mode(mode: &str) -> PyResult<pipeline::CapturePolicy> {
    pipeline::CapturePolicy::from_mode(mode).ok_or_else(|| {
        PyRuntimeError::new_err(format!(
            "invalid mode '{}': expected 'auto', 'hdr', or 'sdr'",
            mode
        ))
    })
}

pub(super) fn warn_mode_mismatch(
    py: Python<'_>,
    policy: pipeline::CapturePolicy,
    is_hdr: bool,
) -> PyResult<()> {
    let msg = match (policy, is_hdr) {
        (pipeline::CapturePolicy::Hdr, false) => Some(
            "mode='hdr' requested but the target monitor is SDR; \
             capture will proceed but output will not contain real HDR data",
        ),
        (pipeline::CapturePolicy::Sdr, true) => Some(
            "mode='sdr' requested but the target monitor is HDR; \
             HDR content will be clipped to SDR range without tone-mapping",
        ),
        _ => None,
    };

    if let Some(msg) = msg {
        let warnings = py.import("warnings")?;
        warnings.call_method1("warn", (msg,))?;
    }
    Ok(())
}
