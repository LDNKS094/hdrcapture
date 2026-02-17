"""Windows screen capture with correct HDR handling.

Provides one-shot ``screenshot()`` and reusable ``capture`` pipeline.
Supports SDR (BGRA8) and HDR (RGBA16F) pixel formats, with automatic
tone-mapping from HDR to SDR when using ``mode='auto'``.
"""

import numpy as np
from typing import Literal
from numpy.typing import NDArray

class CapturedFrame:
    """A single captured frame holding pixel data.

    Pixel format is either ``bgra8`` (8-bit SDR) or ``rgba16f``
    (16-bit half-float HDR, scRGB linear).
    """

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
        """Capture timestamp in seconds, relative to system boot (QPC)."""
        ...

    @property
    def format(self) -> Literal["bgra8", "rgba16f"]:
        """Pixel format: ``'bgra8'`` for SDR, ``'rgba16f'`` for HDR."""
        ...

    def save(self, path: str) -> None:
        """Save frame to file. Format is determined by extension.

        SDR-only formats (bgra8):
          ``.png``, ``.bmp``, ``.jpg`` / ``.jpeg``, ``.tiff`` / ``.tif``

        HDR-capable formats (bgra8 and rgba16f):
          ``.jxr`` — JPEG XR (Windows native, viewable in Photos app)
          ``.exr`` — OpenEXR (industry standard for HDR/VFX)

        Raises:
            RuntimeError: If saving rgba16f data to an SDR-only format.
        """
        ...

    def ndarray(self) -> NDArray[np.uint8] | NDArray[np.float16]:
        """Convert to numpy array, shape ``(H, W, 4)``.

        - ``bgra8``: dtype ``uint8``, BGRA channel order
        - ``rgba16f``: dtype ``float16``, RGBA channel order
        """
        ...

    def __array__(
        self, dtype: object = None, copy: object = None
    ) -> NDArray[np.uint8] | NDArray[np.float16]:
        """NumPy ``__array__`` protocol — enables ``np.array(frame)``."""
        ...

    def __repr__(self) -> str: ...

class capture:
    """Reusable screen/window capture pipeline.

    Construct via static methods::

        cap = capture.monitor(0)
        cap = capture.window("notepad.exe")

    Supports context manager::

        with capture.monitor(0) as cap:
            frame = cap.capture()

    Not thread-safe — use from the creating thread only.

    If the display environment changes (HDR toggled, monitor
    plugged/unplugged), discard this instance and create a new one.
    """

    @staticmethod
    def monitor(
        index: int = 0,
        mode: Literal["auto", "hdr", "sdr"] = "auto",
    ) -> "capture":
        """Create a capture pipeline for a monitor.

        Args:
            index: Monitor index (system enumeration order).
            mode: ``'auto'`` adapts to HDR state (default),
                  ``'hdr'`` forces 16-bit float output,
                  ``'sdr'`` forces 8-bit output.
        """
        ...

    @staticmethod
    def window(
        process: str | None = None,
        *,
        pid: int | None = None,
        hwnd: int | None = None,
        index: int | None = None,
        mode: Literal["auto", "hdr", "sdr"] = "auto",
        headless: bool = True,
    ) -> "capture":
        """Create a capture pipeline for a window.

        Args:
            process: Target process name (e.g. ``"notepad.exe"``).
            pid: Target process id.
            hwnd: Target window handle.
            index: Ranked window index within candidate windows.
            mode: Capture mode (see ``monitor()``).
            headless: Crop title bar and borders in window mode.

        Notes:
            Selector priority is ``hwnd > pid > process``.
            At least one of ``hwnd``, ``pid``, or ``process`` must be provided.
        """
        ...

    @property
    def is_hdr(self) -> bool:
        """Whether the target monitor has HDR enabled."""
        ...

    def capture(self) -> CapturedFrame:
        """Screenshot mode: drain stale frames, wait for a fresh one.

        Guarantees the returned frame was generated after this call.
        Latency is roughly one VSync period. Releases the GIL.
        """
        ...

    def grab(self) -> CapturedFrame:
        """Streaming mode: return the latest available frame.

        May return a frame generated before this call. Lower latency
        than ``capture()``. Releases the GIL.
        """
        ...

    def close(self) -> None:
        """Release capture resources.

        After calling ``close()``, any further method call raises
        ``RuntimeError``.
        """
        ...

    def __enter__(self) -> "capture": ...
    def __exit__(self, exc_type: object, exc_val: object, exc_tb: object) -> bool: ...
    def __repr__(self) -> str: ...

def screenshot(
    monitor: int = 0,
    window: str | None = None,
    pid: int | None = None,
    hwnd: int | None = None,
    index: int | None = None,
    mode: Literal["auto", "hdr", "sdr"] = "auto",
    headless: bool = True,
) -> CapturedFrame:
    """One-shot capture of a monitor or window.

    Creates and destroys a pipeline internally (~70 ms cold start).
    For repeated captures, use the ``capture`` class instead.

    Args:
        monitor: Monitor index (ignored when *window* is set).
        window: Process name for window capture.
        pid: Process id for window capture.
        hwnd: Window handle for window capture.
        index: Ranked window index within candidate windows.
        mode: Capture mode — ``'auto'``, ``'hdr'``, or ``'sdr'``.
        headless: Crop title bar and borders for window capture.

    Returns:
        A ``CapturedFrame`` that can be saved or converted to numpy.
    """
    ...
