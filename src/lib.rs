//! # hdrcapture
//!
//! Windows screen capture that works correctly under HDR.
//!
//! When Windows HDR is enabled, existing screenshot tools produce washed-out images.
//! This library uses Windows Graphics Capture (WGC) with DWM's built-in HDR→SDR
//! tone mapping to deliver accurate colors with zero configuration.
//!
//! ## Rust usage
//!
//! ```no_run
//! use hdrcapture::pipeline;
//! use hdrcapture::pipeline::{CapturePolicy, CaptureSource};
//!
//! // One-shot screenshot
//! let frame = pipeline::screenshot(CaptureSource::Monitor(0), CapturePolicy::Auto).unwrap();
//! frame.save("screenshot.png").unwrap();
//!
//! // Reusable pipeline
//! let mut cap = pipeline::CapturePipeline::monitor(0, CapturePolicy::Auto).unwrap();
//! let frame = cap.capture().unwrap();
//! println!("{}x{}", frame.width, frame.height);
//! ```

#![cfg(windows)]

pub mod capture;
pub mod color;
pub mod d3d11;
pub mod memory;
pub mod pipeline;
mod python;
pub mod shader;
