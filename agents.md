# HDR_CAP - AI Agent Collaboration Guide

> 本文档为 AI 助手提供项目上下文和协作指南，帮助快速理解项目目标、技术栈和开发规范。

---

## 项目概述

**项目名称**：HDR_CAP  
**项目类型**：Rust 库 + Python 绑定（.pyd）  
**核心目标**：解决 Windows HDR 环境下屏幕截图泛白问题，实现"所见即所得"的 HDR→SDR 自动转换截图。

### 问题背景
- Windows 开启 HDR 后，现有 Python 截图库（dxcam、windows-capture 等）捕获的图像出现泛白/过亮现象
- 根本原因：这些库使用 8 位 SDR 格式（`B8G8R8A8_UNORM`），Windows 自动执行的 HDR→SDR 转换不完美
- 闭源工具 PixPin 能正确捕获，证明技术上可行

### 解决方案
1. 使用 **16-bit float 格式**（`R16G16B16A16_FLOAT`）捕获原始 HDR 数据
2. 通过 **GPU Compute Shader** 执行自定义色调映射（HDR→SDR）
3. 动态获取系统 **SDR White Level** 参数，自动校准亮度
4. 通过 **PyO3** 封装为 Python 可用的 NumPy 接口

---

## 技术栈

| 层级 | 技术 | 用途 |
|---|---|---|
| **底层捕获** | `windows-rs` + WGC (Windows Graphics Capture) | 屏幕/窗口捕获 |
| **图形 API** | Direct3D 11 | GPU 纹理管理、Compute Shader 调度 |
| **色调映射** | HLSL Compute Shader | GPU 加速的 HDR→SDR 转换 |
| **Python 绑定** | PyO3 + rust-numpy | 零拷贝 NumPy 数组传递 |
| **构建工具** | maturin | 生成 Python wheel 包 |

---

## 项目结构

```
HDR_CAP/
├── Cargo.toml                    # Rust 依赖配置
├── pyproject.toml                # Python 包配置（maturin）— 待创建
├── .gitignore
├── README.md
├── LICENSE
├── agents.md                     # 本文档
│
├── src/
│   ├── lib.rs                    # 库入口（模块声明）
│   ├── d3d11.rs                  # D3D11Context, create_d3d11_device
│   ├── d3d11/
│   │   └── texture.rs            # TextureReader（单缓冲、行剥离、buffer 复用）
│   ├── capture.rs                # 模块入口（re-export target + wgc）
│   ├── capture/
│   │   ├── target.rs             # find_monitor, find_window, enable_dpi_awareness
│   │   └── wgc.rs                # WGCCapture, init_capture, CaptureTarget（固定 BGRA8）
│   ├── pipeline.rs               # 完整处理管线（待实现）
│   └── python.rs                 # PyO3 Python 绑定（待实现）
│
├── tests/
│   ├── test_monitor_capture.rs   # 集成测试：按索引捕获显示器
│   ├── test_window_capture.rs    # 集成测试：按进程名捕获窗口
│   └── results/                  # 测试截图输出（gitignored）
│
└── docs/
    └── dev/                      # 开发文档（gitignored，不纳入版本控制）
        ├── plan_v2.md            # 高层设计（V2 简化路线）
        ├── STEP_BY_STEP_PLAN_V2.md # 分步骤执行计划（V2）
        └── ...                   # 其他调研/设计文档
```

---

## 开发阶段（P0-P1）

### P0：捕获引擎 ✅
- **目标**：统一使用 BGRA8 格式捕获，DWM 自动处理 HDR→SDR
- **已完成**：
  - D3D11 设备创建、WGC 捕获管线
  - 目标解析（显示器索引枚举、窗口 PID 预过滤查找）
  - 纹理 CPU 回读（行剥离、buffer 复用）
  - 简化管线（移除 HDR 检测，固定 BGRA8）
  - HDR 环境截图质量验证
- **验证标准**：HDR 环境下截图不泛白，色彩自然 ✅

### P1：性能优化与 Python 封装 ⬅️ 当前阶段
- **目标**：4K 延迟 < 10ms，生成可 pip install 的 .pyd 模块
- **关键步骤**：
  - Step 1.0 — Pipeline API（高层管线接口）⬅️ 下一步
  - Step 1.1 — GPU-CPU 回读优化 ✅
  - Step 1.2 — 性能基准测试
  - Step 1.3 — PyO3 基础封装
  - Step 1.4 — Python API 设计
  - Step 1.5 — 构建打包
- **验证标准**：Python 中 `import hdr_cap` 并成功截图，4K 延迟 < 10ms

---

## 关键技术要点

### 1. 为什么必须用 16-bit float？
- HDR 显示器使用 10-bit 或更高位深，动态范围超出 SDR 的 [0, 1]
- 8-bit SDR 格式无法承载超额亮度信息，Windows 自动转换会丢失细节
- `R16G16B16A16_FLOAT` 可以表示 scRGB 线性空间（值可 > 1.0）

### 2. 什么是 scRGB？
- 线性 RGB 色彩空间，1.0 = 80 nits（SDR 白点）
- 值可以超过 1.0（如 2.0 = 160 nits），表示 HDR 高光
- 避免了复杂的 ST.2084 (PQ) 曲线解码

### 3. 色调映射核心算法
```hlsl
// 1. 亮度标定
float scale = 80.0 / sdr_white_level;  // sdr_white_level 通常 80~400 nits
float3 color = hdr_input.rgb * scale;

// 2. 色调压缩（Reinhard 或 ACES）
color = color / (1.0 + color);  // 简化版 Reinhard

// 3. Gamma 校正（Linear → sRGB）
color = pow(color, 1.0 / 2.2);

// 4. 输出
output = saturate(color);  // Clamp 到 [0, 1]
```

### 4. SDR White Level 是什么？
- Windows HDR 设置中的"SDR 内容亮度"滑块
- 定义 SDR 内容在 HDR 显示器上的显示亮度（nits）
- 通过 `DisplayConfigGetDeviceInfo` API 查询
- 公式：`实际 nits = (SDRWhiteLevel / 1000) * 80`

---

## 编码规范

### Rust 代码风格
- 使用 `rustfmt` 格式化（`cargo fmt`）
- 使用 `clippy` 检查（`cargo clippy`）
- 错误处理：优先使用 `Result<T, E>`，避免 `unwrap()`
- 不安全代码：必须添加 `// SAFETY:` 注释说明

### 命名约定
- 模块/文件：`snake_case`（如 `white_level.rs`）
- 结构体/枚举：`PascalCase`（如 `HDRCapture`）
- 函数/变量：`snake_case`（如 `create_device`）
- 常量：`SCREAMING_SNAKE_CASE`（如 `DEFAULT_WHITE_LEVEL`）

### Git 提交规范
```
<type>(<scope>): <subject>

<body>

<footer>
```
**Type**：
- `feat`: 新功能
- `fix`: Bug 修复
- `docs`: 文档更新
- `refactor`: 代码重构
- `perf`: 性能优化
- `test`: 测试相关
- `chore`: 构建/工具配置

**示例**：
```
feat(capture): implement WGC frame capture with R16G16B16A16Float

- Add D3D11 device creation
- Implement GraphicsCaptureSession initialization
- Support monitor and window capture modes

Closes #1
```

---

## AI 协作指南

### 当你被要求实现某个功能时：

1. **先查阅文档**：
   - `docs/dev/STEP_BY_STEP_PLAN.md` — 详细步骤指南
   - `docs/dev/plan.md` — 高层设计思路
   - `docs/dev/HDR_INVESTIGATION.md` — 技术调研背景

2. **确认当前阶段**：
   - 检查 `Cargo.toml` 是否存在（判断是否已初始化）
   - 检查 `src/` 目录结构（判断进度）
   - 询问用户当前在哪个 Step

3. **编写代码前**：
   - 确认依赖是否已添加到 `Cargo.toml`
   - 确认 `windows-rs` features 是否已启用
   - 确认前置步骤是否完成（参考依赖关系图）

4. **编写代码时**：
   - 添加详细注释，特别是 `unsafe` 代码块
   - 提供错误处理（不要 `unwrap()`）
   - 添加 `#[cfg(windows)]` 条件编译（仅 Windows 平台）

5. **验证代码后**：
   - 提供测试命令（如 `cargo run --example capture_test`）
   - 说明预期输出
   - 提供调试建议（如果失败）

### 常见问题处理

| 问题 | 解决方案 |
|---|---|
| `windows-rs` 编译错误 | 检查 features 是否完整，参考 Step 0.2 |
| WGC 初始化失败 | 确认 Windows 版本 ≥ 10 1903，检查 D3D11 设备是否创建成功 |
| 捕获的图像全黑 | 检查纹理格式是否为 `B8G8R8A8_UNORM`，检查 CopyResource 是否执行 |
| 色调映射后仍泛白 | 已简化：DWM 自动处理 HDR→SDR，无需自定义色调映射 |
| PyO3 编译错误 | 确认 Python 版本 ≥ 3.9，检查 `pyproject.toml` 配置 |

---

## 参考资源

### 官方文档
- [windows-rs GitHub](https://github.com/microsoft/windows-rs)
- [PyO3 User Guide](https://pyo3.rs/)
- [Microsoft Docs: Windows Graphics Capture](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture)
- [DXGI_FORMAT Enum](https://learn.microsoft.com/en-us/windows/win32/api/dxgiformat/ne-dxgiformat-dxgi_format)

### 参考项目
- [ScreenCapy](https://github.com/dumbie/ScreenCapy) — C++ HDR 截图实现
- [OBS Studio](https://github.com/obsproject/obs-studio) — 色调映射着色器参考

### 技术文章
- [Understanding scRGB](https://learn.microsoft.com/en-us/windows/win32/wcs/scrgb)
- [HDR in Windows](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range)

---

## 当前状态

**阶段**：P1 - 性能优化与 Python 封装  
**下一步**：Step 1.0 - Pipeline API  
**已完成**：
- ✅ P0 捕获引擎（Step 0.1 ~ 0.9）
  - D3D11 设备创建、WGC 捕获（固定 BGRA8）、DPI 感知
  - 目标解析（显示器索引枚举、窗口 PID 预过滤查找）
  - 纹理回读（行剥离、buffer 复用）
  - HDR 环境截图质量验证
- ✅ Step 1.1 GPU-CPU 回读优化（Staging 缓存、RowPitch 剥离、buffer 复用）

**待办事项**：
- [ ] Step 1.0 — Pipeline API（高层管线接口）
- [ ] Step 1.2 — 性能基准测试
- [ ] Step 1.3 — PyO3 基础封装
- [ ] Step 1.4 — Python API 设计
- [ ] Step 1.5 — 构建打包

---

> **最后更新**：2026-02-11  
> **文档版本**：v2.0
