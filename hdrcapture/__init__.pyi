"""Windows screen capture that works correctly under HDR."""

import numpy as np
from typing import Literal
from numpy.typing import NDArray

class CapturedFrame:
    """A single captured frame holding BGRA8 or RGBA16F pixel data."""

    @property
    def width(self) -> int:
        """Frame width in pixels."""
        ...

    @property
    def height(self) -> int:
        """Frame height in pixels."""
        ...

    @property
    def timestamp(self) -> float:
        """Capture timestamp in seconds, relative to system boot."""
        ...

    @property
    def format(self) -> str:
        """Pixel format string: 'bgra8' or 'rgba16f'."""
        ...

    def save(self, path: str) -> None:
        """Save frame to file (format determined by extension).

        Supported formats:
          - .png — PNG (bgra8 / SDR only)
          - .bmp — BMP (bgra8 / SDR only)
          - .jpg / .jpeg — JPEG (bgra8 / SDR only)
          - .tiff / .tif — TIFF (bgra8 / SDR only)
          - .jxr — JPEG XR (both bgra8 and rgba16f / HDR)
        """
        ...

    def ndarray(self) -> NDArray[np.uint8]:
        """Convert to numpy array for bgra8 frames, shape (H, W, 4), dtype uint8."""
        ...

    def __array__(self, dtype: object = None, copy: object = None) -> NDArray[np.uint8]:
        """Support np.array(frame) protocol."""
        ...

    def __repr__(self) -> str: ...

class Capture:
    """Reusable screen/window capture pipeline.

    Not thread-safe. Each instance must only be used from the thread that created it.
    """

    @staticmethod
    def monitor(
        index: int = 0, mode: Literal["auto", "hdr", "sdr"] = "auto"
    ) -> "Capture":
        """Create a capture pipeline for a monitor by index."""
        ...

    @staticmethod
    def window(
        process_name: str,
        index: int | None = None,
        mode: Literal["auto", "hdr", "sdr"] = "auto",
    ) -> "Capture":
        """Create a capture pipeline for a window by process name."""
        ...

    def capture(self) -> CapturedFrame:
        """Screenshot mode: drain stale frames, wait for a fresh one."""
        ...

    def grab(self) -> CapturedFrame:
        """Streaming mode: return the latest available frame."""
        ...

    def close(self) -> None:
        """Release capture resources."""
        ...

    def __enter__(self) -> "Capture": ...
    def __exit__(self, exc_type: object, exc_val: object, exc_tb: object) -> bool: ...
    def __repr__(self) -> str: ...

def screenshot(
    monitor: int = 0,
    window: str | None = None,
    window_index: int | None = None,
    mode: Literal["auto", "hdr", "sdr"] = "auto",
) -> CapturedFrame:
    """One-shot capture of the specified monitor or window.

    Creates and destroys a pipeline internally (~70ms cold start).
    Use Capture class for repeated captures.

    If `window` is provided, window capture is used and `monitor` is ignored.
    """
    ...
