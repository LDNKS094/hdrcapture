// HDR Capture Library
// 解决 Windows HDR 环境下屏幕截图泛白问题
//
// 核心原理：WGC 请求 BGRA8 格式时，DWM 自动完成 HDR→SDR 色调映射。

#![cfg(windows)]
#![allow(dead_code)] // 开发阶段允许未使用的代码
#![allow(unused_imports)] // 开发阶段允许未使用的导入

pub mod capture;
pub mod d3d11;
pub(crate) mod pipeline;
