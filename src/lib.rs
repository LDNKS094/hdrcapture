// HDR Capture Library
// 解决 Windows HDR 环境下屏幕截图泛白问题

#![cfg(windows)] //如果目标操作系统不是 Windows，就完全忽略
#![allow(dead_code)] // 开发阶段允许未使用的代码

// 模块声明（crate 内部可见）
pub(crate) mod capture;
pub(crate) mod d3d11;
pub(crate) mod pipeline;
pub(crate) mod tonemap;

// Python 绑定（后续 P3 阶段启用）
// mod python;

// 公开 API（暂时为空，后续逐步添加）
// pub use capture::*;

#[cfg(test)]
mod tests {
    // use super::*;

    #[test]
    fn test_basic() {
        assert_eq!(2 + 2, 4);
    }

    // 集成测试：等多个模块完成后添加
    // 例如：测试 d3d11 + capture 协作
    // #[test]
    // fn test_d3d11_and_capture() {
    //     let device = d3d11::create_d3d11_device().unwrap();
    //     let capture = capture::init(&device).unwrap();
    // }
}
