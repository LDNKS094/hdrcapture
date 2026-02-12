# hdrcapture

Windows screen capture that works correctly under HDR.

Existing Python screenshot libraries (dxcam, windows-capture, etc.) produce washed-out images when Windows HDR is enabled. `hdrcapture` solves this by leveraging Windows Graphics Capture (WGC) with DWM's built-in HDR→SDR tone mapping, delivering accurate colors with zero configuration.

## Features

- Correct HDR→SDR tone mapping — no washed-out screenshots
- Monitor and window capture
- NumPy array output (H, W, 4) BGRA uint8
- PNG save with fast compression
- GIL released during capture — won't block your Python threads
- Single-shot and streaming modes

## Requirements

- Windows 10 version 1903 or later
- Python >= 3.9
- NumPy

## Installation

```bash
pip install hdrcapture
```

Or build from source:

```bash
pip install maturin
git clone https://github.com/LDNKS094/hdrcapture.git
cd hdrcapture
maturin develop --release
```

## Quick Start

One-liner screenshot:

```python
import hdrcapture

frame = hdrcapture.screenshot()
frame.save("screenshot.png")
```

As a NumPy array:

```python
import hdrcapture
import numpy as np

frame = hdrcapture.screenshot()
arr = frame.ndarray()  # shape (H, W, 4), dtype uint8, BGRA
# or: arr = np.array(frame)
```

Reusable capture pipeline (lower latency for multiple captures):

```python
import hdrcapture

with hdrcapture.Capture.monitor(0) as cap:
    # Screenshot mode — waits for a fresh frame
    frame = cap.capture()
    frame.save("capture.png")

    # Streaming mode — grabs the latest available frame
    frame = cap.grab()
    arr = frame.ndarray()
```

Window capture:

```python
import hdrcapture

with hdrcapture.Capture.window("notepad.exe") as cap:
    frame = cap.capture()
    frame.save("notepad.png")
```

## API Reference

### Module-level

#### `screenshot(monitor=0) -> CapturedFrame`

One-shot capture. Creates and destroys a pipeline internally (~70ms cold start). Use `Capture` class for repeated captures.

### `CapturedFrame`

| Property / Method | Description |
|---|---|
| `width` | Frame width in pixels |
| `height` | Frame height in pixels |
| `timestamp` | Capture timestamp in seconds (relative to system boot) |
| `save(path)` | Save as PNG file |
| `ndarray()` | Convert to NumPy array, shape `(H, W, 4)`, dtype `uint8`, BGRA channel order |

Supports `np.array(frame)` via the `__array__` protocol.

### `Capture`

Reusable capture pipeline. Construct via static methods:

| Method | Description |
|---|---|
| `Capture.monitor(index=0)` | Capture a monitor by index |
| `Capture.window(process_name, index=None)` | Capture a window by process name |
| `capture()` | Screenshot mode — drains stale frames, waits for a fresh one |
| `grab()` | Streaming mode — returns the latest available frame |
| `close()` | Release capture resources |

Supports context manager (`with` statement).

#### `capture()` vs `grab()`

- `capture()`: Discards all buffered frames and waits for DWM to push a new one. Guarantees the frame was produced after the call. ~1 VSync latency.
- `grab()`: Drains the buffer but keeps the last frame. If the buffer is empty, waits for a new frame. Lower latency, but the frame may have been produced before the call.

## Performance

Measured on 5120×1440 (ultrawide):

| Metric | Value |
|---|---|
| Single-shot (cold start) | ~70ms |
| Streaming `grab()` p50 | ~15.7ms |
| Streaming theoretical limit | 16.7ms (60Hz VSync) |

## How It Works

When Windows HDR is enabled, the Desktop Window Manager (DWM) composites everything in a high dynamic range color space. By requesting frames in `B8G8R8A8_UNORM` format through WGC, DWM automatically performs HDR→SDR tone mapping — producing correctly exposed images without any custom shader or manual color conversion.

## License

[MIT](LICENSE)
