# hdrcapture

Windows screen capture with correct HDR handling.

Existing Python screenshot libraries produce washed-out images when Windows HDR is enabled. `hdrcapture` solves this with Windows Graphics Capture (WGC) and a GPU-accelerated BT.2390 tone-mapping pipeline, delivering accurate colors in both SDR and HDR workflows.

## Features

- Three capture modes: `auto`, `hdr`, `sdr`
- Correct colors on HDR monitors — no washed-out screenshots
- Monitor and window capture
- Single-shot and streaming modes
- NumPy array output
- GIL released during capture

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

with hdrcapture.capture.monitor(0) as cap:
    frame = cap.capture()
    frame.save("capture.png")

    # Streaming mode — grabs the latest available frame
    frame = cap.grab()
    arr = frame.ndarray()
```

Window capture:

```python
import hdrcapture

frame = hdrcapture.screenshot(window="notepad.exe")
frame.save("notepad.png")

with hdrcapture.capture.window("notepad.exe") as cap:
    frame = cap.capture()
```

## Capture Modes

The `mode` parameter controls how HDR content is handled:

| Mode | Pixel Format | Behavior |
|------|-------------|----------|
| `"auto"` (default) | bgra8 | HDR monitor → GPU tone-map to SDR; SDR monitor → direct capture |
| `"hdr"` | rgba16f | Raw 16-bit float scRGB output, no tone mapping |
| `"sdr"` | bgra8 | Force 8-bit capture, DWM hard-clips HDR content |

```python
# Auto mode (recommended) — handles HDR transparently
frame = hdrcapture.screenshot()

# HDR mode — raw float16 for professional workflows
frame = hdrcapture.screenshot(mode="hdr")
frame.save("capture.jxr")   # JPEG XR preserves HDR data
frame.save("capture.exr")   # OpenEXR for VFX pipelines

# SDR mode — legacy behavior, equivalent to DWM's built-in conversion
frame = hdrcapture.screenshot(mode="sdr")
```

## Save Formats

| Extension | HDR Support | Notes |
|-----------|------------|-------|
| `.png` | SDR only | Fast, lossless |
| `.bmp` | SDR only | Uncompressed |
| `.jpg` / `.jpeg` | SDR only | Lossy |
| `.tiff` / `.tif` | SDR only | Lossless |
| `.jxr` | SDR + HDR | Windows native, viewable in Photos app |
| `.exr` | SDR + HDR | Industry standard for HDR/VFX |

## API Reference

### `screenshot(monitor=0, window=None, window_index=None, mode="auto") -> CapturedFrame`

One-shot capture. Creates and destroys a pipeline internally (~70ms cold start). Use `capture` class for repeated captures.

### `CapturedFrame`

| Property / Method | Description |
|---|---|
| `width` | Frame width in pixels |
| `height` | Frame height in pixels |
| `timestamp` | Capture timestamp in seconds (relative to system boot) |
| `format` | Pixel format: `"bgra8"` or `"rgba16f"` |
| `save(path)` | Save to file (format by extension) |
| `ndarray()` | NumPy array `(H, W, 4)`, dtype `uint8`, BGRA (bgra8 only) |

Supports `np.array(frame)` via the `__array__` protocol.

### `capture`

Reusable capture pipeline. Not thread-safe — use from the creating thread only.

If the display environment changes (HDR toggled, monitor plugged/unplugged), discard the instance and create a new one.

| Method | Description |
|---|---|
| `capture.monitor(index=0, mode="auto")` | Create pipeline for a monitor |
| `capture.window(process_name, index=None, mode="auto")` | Create pipeline for a window |
| `.is_hdr` | Whether the target monitor has HDR enabled |
| `.capture()` | Screenshot mode — waits for a fresh frame (~1 VSync) |
| `.grab()` | Streaming mode — returns the latest available frame |
| `.close()` | Release capture resources |

Supports context manager (`with` statement).

## Performance

Measured on 5120×1440 (ultrawide):

| Metric | Value |
|---|---|
| Single-shot (cold start) | ~70ms |
| Streaming `grab()` p50 | ~15.7ms |
| Tone-map overhead | <0.5ms |

## FAQ

**Why do screenshots look washed out with other libraries?**

When Windows HDR is enabled, the desktop is composited in scRGB (linear, wide-range). Libraries that read pixels as plain 8-bit BGRA get DWM's hard-clipped conversion, which compresses the SDR range and makes everything look flat. `hdrcapture` in `auto` mode captures the full HDR signal and applies BT.2390 tone mapping on the GPU, preserving the SDR range while smoothly rolling off highlights.

**When should I use `mode="sdr"`?**

When you want the exact same output as a non-HDR-aware screenshot tool — DWM's built-in hard clip. This is useful for pixel-exact comparisons or when you know the content is pure SDR.

**When should I use `mode="hdr"`?**

When you need the raw HDR pixel data for professional workflows (color grading, VFX compositing). Save as `.exr` or `.jxr` to preserve the full dynamic range.

**Can I convert `rgba16f` frames to NumPy?**

Not yet — `ndarray()` currently only supports `bgra8`. For HDR frames, use `save()` to write `.exr` or `.jxr` files.

## Acknowledgements

The HDR capture and tone-mapping pipeline references [OBS Studio](https://obsproject.com/)'s HDR handling logic.

## License

[MIT](LICENSE)
