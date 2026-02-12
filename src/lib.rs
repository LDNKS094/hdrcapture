// HDR Capture Library
// 解决 Windows HDR 环境下屏幕截图泛白问题
//
// 核心原理：WGC 请求 BGRA8 格式时，DWM 自动完成 HDR→SDR 色调映射。

#![cfg(windows)]

pub mod capture;
pub mod d3d11;
pub mod pipeline;
mod python;
