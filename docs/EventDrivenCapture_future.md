# 进阶方案：事件驱动后台捕获

> **定位**：当同步模式（方案 A）的 2-5ms 阻塞无法满足需求时的升级路径。
> **前置条件**：方案 A（同步 Pipeline）已实现并验证。

---

## 1. 动机

同步模式下，每次 `capture_frame()` 调用在调用方线程执行完整流程：

```
排空 FramePool → CopyResource → Map → 逐行拷贝 → 返回
                |←────────── 2~5ms（4K）──────────→|
```

对于 Python 调用方，GIL 在此期间被持有，阻塞 Python 线程。
60fps 连续读屏时，每秒累计阻塞 120-300ms。

事件驱动模式将捕获工作移至后台线程，`capture_frame()` 仅读取已准备好的 buffer，耗时 <0.1ms。

---

## 2. 架构

采用生产者-消费者模型：

```
后台线程（生产者）                          调用方线程（消费者）
┌─────────────────────────┐               ┌──────────────────┐
│ FrameArrived 事件触发    │               │ capture_frame()  │
│   ↓                     │               │   ↓              │
│ 排空 FramePool          │               │ RwLock::read()   │
│   ↓                     │    共享        │   ↓              │
│ CopyResource            │◄──buffer──►   │ 返回 &[u8]       │
│   ↓                     │   (RwLock)     │                  │
│ drop Frame              │               └──────────────────┘
│   ↓                     │
│ Map + 逐行拷贝          │
│   ↓                     │
│ RwLock::write() 更新    │
└─────────────────────────┘
```

---

## 3. 需要解决的问题

### 3.1 D3D11 线程安全

D3D11 immediate context 不是线程安全的。后台线程执行 `CopyResource` / `Map` 时，
如果调用方线程同时操作同一 device context，会产生竞态。

解决方式：
- 创建设备时启用 `ID3D11Multithread::SetMultithreadProtected(true)`
- 或确保所有 D3D11 操作仅在后台线程执行

微软文档原文：
> "it is recommended that applications take the ID3D11Multithread lock on the same device
> that is associated with the Direct3D11CaptureFramePool object."

### 3.2 FrameArrived 回调线程

`CreateFreeThreaded` 创建的 FramePool，`FrameArrived` 在线程池线程上触发。
多个回调可能并发执行，需要串行化（Mutex 或单线程调度）。

### 3.3 共享 buffer 同步

后台线程写入、调用方线程读取，需要 `RwLock<Vec<u8>>` 或类似机制。
写入频率 = 显示器刷新率（60-240Hz），读取频率 = 调用方决定。

### 3.4 线程生命周期

`CapturePipeline` drop 时需要：
1. 停止 `GraphicsCaptureSession`
2. 通知后台线程退出
3. 等待线程结束（join）
4. 释放 D3D11 资源

### 3.5 首帧同步

后台线程启动后，调用方需要等待第一帧就绪。
可用 `Condvar` 或 `Event` 信号通知。

---

## 4. 性能对比预期

| 指标 | 同步模式（方案 A） | 事件驱动（方案 B） |
|------|-------------------|-------------------|
| capture_frame() 耗时 | 2-5ms | <0.1ms |
| 帧新鲜度 | 调用时刻池中最新 | 后台持续更新，始终最新 |
| CPU 占用 | 按需 | 持续（后台线程） |
| 实现复杂度 | 低 | 高 |
| 适用场景 | 截图、中频读屏 | 高频连续读屏 |

---

## 5. 迁移路径

公共 API 保持不变：

```rust
let mut pipeline = CapturePipeline::monitor(0)?;
let pixels: &[u8] = pipeline.capture_frame()?;
```

内部实现从同步切换为事件驱动，对调用方透明。

---

> **文档版本**：v2.0
> **最后更新**：2026-02-11
> **状态**：待评估（方案 A 性能不足时启用）
