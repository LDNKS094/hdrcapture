"""Functional tests for hdrcapture Python API."""

from __future__ import annotations

import tracemalloc
from pathlib import Path
from typing import Any

import hdrcapture
import numpy as np
import pytest


pytestmark = [pytest.mark.requires_display]


def test_screenshot_save_multi_format(tmp_path: Path) -> None:
    frame = hdrcapture.screenshot()

    outputs = {
        "png": tmp_path / "test_screenshot.png",
        "bmp": tmp_path / "test_screenshot.bmp",
        "jpg": tmp_path / "test_screenshot.jpg",
        "tiff": tmp_path / "test_screenshot.tiff",
        "jxr": tmp_path / "test_screenshot.jxr",
        "exr": tmp_path / "test_screenshot.exr",
    }

    for path in outputs.values():
        frame.save(str(path))
        assert path.exists()
        assert path.stat().st_size > 0


def test_bgra8_ndarray_conversion() -> None:
    frame = hdrcapture.screenshot()

    arr = frame.ndarray()
    assert arr.shape == (frame.height, frame.width, 4)
    assert arr.dtype == np.uint8

    arr2 = np.array(frame)
    assert np.array_equal(arr, arr2)


def test_capture_class_capture_and_grab() -> None:
    cap = hdrcapture.capture.monitor(0)
    try:
        f1 = cap.capture()
        f2 = cap.grab()
        f3 = cap.grab()

        assert f1.width > 0 and f1.height > 0
        assert f2.width > 0 and f2.height > 0
        assert f3.width > 0 and f3.height > 0
    finally:
        cap.close()


def test_context_manager_closes_capture() -> None:
    with hdrcapture.capture.monitor(0) as cap:
        frame = cap.capture()
        assert frame.width > 0 and frame.height > 0

    with pytest.raises(RuntimeError):
        cap.capture()


def test_continuous_grab_latency_sanity() -> None:
    cap = hdrcapture.capture.monitor(0)
    try:
        _ = cap.grab()  # warm up
        for _ in range(20):
            frame = cap.grab()
            assert frame.width > 0 and frame.height > 0
    finally:
        cap.close()


def test_error_handling_invalid_targets() -> None:
    with pytest.raises(RuntimeError):
        hdrcapture.screenshot(monitor=999)

    with pytest.raises(RuntimeError):
        hdrcapture.screenshot(window="__nonexistent_process_12345__.exe")

    with pytest.raises(RuntimeError):
        hdrcapture.capture.window("__nonexistent_process_12345__.exe")

    with pytest.raises(RuntimeError):
        hdrcapture.capture.window()

    with pytest.raises(RuntimeError):
        hdrcapture.capture.window(pid=999_999_999)

    with pytest.raises(RuntimeError):
        hdrcapture.capture.window(hwnd=0)


def test_window_selector_priority_warnings() -> None:
    with pytest.warns(UserWarning):
        with pytest.raises(RuntimeError):
            hdrcapture.capture.window(
                "__nonexistent_process_12345__.exe", pid=999_999_999
            )

    with pytest.warns(UserWarning):
        with pytest.raises(RuntimeError):
            hdrcapture.capture.window(
                "__nonexistent_process_12345__.exe",
                pid=999_999_999,
                hwnd=0,
            )


def test_capture_modes_and_invalid_mode(tmp_path: Path) -> None:
    with pytest.warns(UserWarning):
        frame_sdr = hdrcapture.screenshot(mode="sdr")
    assert frame_sdr.format == "bgra8"

    frame_auto = hdrcapture.screenshot(mode="auto")
    assert frame_auto.format == "bgra8"

    # HDR mode may either return rgba16f (HDR monitor) or fall back depending on monitor state.
    frame_hdr = hdrcapture.screenshot(mode="hdr")
    assert frame_hdr.format in {"bgra8", "rgba16f"}
    if frame_hdr.format == "rgba16f":
        hdr_jxr = tmp_path / "test_hdr.jxr"
        hdr_exr = tmp_path / "test_hdr.exr"
        frame_hdr.save(str(hdr_jxr))
        frame_hdr.save(str(hdr_exr))
        assert hdr_jxr.exists() and hdr_jxr.stat().st_size > 0
        assert hdr_exr.exists() and hdr_exr.stat().st_size > 0

    with pytest.raises(RuntimeError):
        hdrcapture.screenshot(mode="invalid")  # type: ignore[arg-type]


def test_is_hdr_property_type() -> None:
    with hdrcapture.capture.monitor(0) as cap:
        assert isinstance(cap.is_hdr, bool)


def test_hdr_ndarray_conversion_when_available() -> None:
    hdr_frame: Any | None = None
    with hdrcapture.capture.monitor(0, mode="hdr") as cap:
        hdr_frame = cap.capture()

    assert hdr_frame is not None
    if hdr_frame.format != "rgba16f":
        pytest.skip(f"HDR frame format unavailable: {hdr_frame.format}")

    arr_hdr = hdr_frame.ndarray()
    assert arr_hdr.shape == (hdr_frame.height, hdr_frame.width, 4)
    assert arr_hdr.dtype == np.float16

    arr_hdr2 = np.array(hdr_frame)
    assert np.array_equal(arr_hdr, arr_hdr2)

    finite_ratio = np.isfinite(arr_hdr).mean()
    assert finite_ratio > 0.99


@pytest.mark.slow
def test_long_streaming_memory_stability() -> None:
    tracemalloc.start()
    cap = hdrcapture.capture.monitor(0)
    _ = cap.grab()  # warm up

    try:
        snapshot1 = tracemalloc.take_snapshot()
        for _ in range(100):
            frame = cap.grab()
            _ = frame.ndarray()
        snapshot2 = tracemalloc.take_snapshot()
    finally:
        cap.close()
        tracemalloc.stop()

    stats = snapshot2.compare_to(snapshot1, "lineno")
    total_diff = sum(s.size_diff for s in stats)
    assert total_diff <= 50 * 1024 * 1024
