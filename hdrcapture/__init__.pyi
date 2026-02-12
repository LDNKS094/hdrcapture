"""Windows screen capture that works correctly under HDR."""

import numpy as np
from numpy.typing import NDArray

class CapturedFrame:
    """A single captured frame holding BGRA8 pixel data."""

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

    def save(self, path: str) -> None:
        """Save as PNG file."""
        ...

    def ndarray(self) -> NDArray[np.uint8]:
        """Convert to numpy array, shape (H, W, 4), dtype uint8, BGRA channel order."""
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
    def monitor(index: int = 0, force_sdr: bool = False) -> "Capture":
        """Create a capture pipeline for a monitor by index."""
        ...

    @staticmethod
    def window(
        process_name: str,
        index: int | None = None,
        force_sdr: bool = False,
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
    force_sdr: bool = False,
) -> CapturedFrame:
    """One-shot capture of the specified monitor or window.

    Creates and destroys a pipeline internally (~70ms cold start).
    Use Capture class for repeated captures.

    If `window` is provided, window capture is used and `monitor` is ignored.
    """
    ...
