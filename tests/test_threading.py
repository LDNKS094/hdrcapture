"""Cross-thread safety tests for the Python binding."""

import concurrent.futures
import gc
import threading
from typing import Any

import hdrcapture
import pytest


pytestmark = [pytest.mark.threading, pytest.mark.requires_display]


def _join_or_fail(thread: threading.Thread, timeout: float) -> None:
    thread.join(timeout=timeout)
    assert not thread.is_alive(), "worker thread timed out"


def test_create_on_main_capture_on_worker() -> None:
    cap = hdrcapture.capture.monitor(0)
    result: list[Any | None] = [None]
    exc: list[Exception | None] = [None]

    def worker_capture() -> None:
        try:
            result[0] = cap.capture()
        except Exception as err:  # pragma: no cover - diagnostic path
            exc[0] = err

    try:
        t = threading.Thread(target=worker_capture)
        t.start()
        _join_or_fail(t, timeout=10)

        assert exc[0] is None
        assert result[0] is not None
    finally:
        cap.close()


def test_create_on_worker_capture_on_main() -> None:
    cap_ref: list[Any | None] = [None]
    exc: list[Exception | None] = [None]

    def worker_create() -> None:
        try:
            cap_ref[0] = hdrcapture.capture.monitor(0)
        except Exception as err:  # pragma: no cover - diagnostic path
            exc[0] = err

    t = threading.Thread(target=worker_create)
    t.start()
    _join_or_fail(t, timeout=10)

    assert exc[0] is None
    assert cap_ref[0] is not None

    cap = cap_ref[0]
    try:
        frame = cap.capture()
        assert frame is not None
    finally:
        cap.close()


def test_concurrent_grab_shared_capture() -> None:
    cap = hdrcapture.capture.monitor(0)
    _ = cap.grab()  # warm up

    thread_results: dict[int, list[float]] = {}
    thread_errors: dict[int, Exception] = {}
    lock = threading.Lock()

    def worker_grab(tid: int) -> None:
        local_frames: list[float] = []
        local_err = None
        for _ in range(10):
            try:
                frame = cap.grab()
                local_frames.append(frame.timestamp)
            except Exception as err:  # pragma: no cover - diagnostic path
                local_err = err
                break
        with lock:
            thread_results[tid] = local_frames
            if local_err is not None:
                thread_errors[tid] = local_err

    try:
        threads = [threading.Thread(target=worker_grab, args=(i,)) for i in range(4)]
        for t in threads:
            t.start()
        for t in threads:
            _join_or_fail(t, timeout=30)

        assert not thread_errors
        total_frames = sum(len(v) for v in thread_results.values())
        assert total_frames == 40
    finally:
        cap.close()


def test_close_from_different_thread() -> None:
    cap = hdrcapture.capture.monitor(0)
    _ = cap.capture()
    exc: list[Exception | None] = [None]

    def worker_close() -> None:
        try:
            cap.close()
        except Exception as err:  # pragma: no cover - diagnostic path
            exc[0] = err

    t = threading.Thread(target=worker_close)
    t.start()
    _join_or_fail(t, timeout=10)

    assert exc[0] is None
    with pytest.raises(RuntimeError):
        cap.capture()


def test_drop_without_explicit_close() -> None:
    cap = hdrcapture.capture.monitor(0)
    _ = cap.capture()
    del cap

    # Should not panic or crash.
    gc.collect()


@pytest.mark.slow
def test_thread_pool_executor_pattern() -> None:
    cap = hdrcapture.capture.monitor(0)
    _ = cap.grab()  # warm up

    def pool_grab(_: int) -> float:
        return cap.grab().timestamp

    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            futures = [pool.submit(pool_grab, i) for i in range(20)]
            timestamps = [f.result(timeout=10) for f in futures]

        assert len(timestamps) == 20
        assert min(timestamps) <= max(timestamps)
    finally:
        cap.close()


def test_screenshot_from_worker_thread() -> None:
    exc: list[Exception | None] = [None]
    frame_ref: list[Any | None] = [None]

    def worker_screenshot() -> None:
        try:
            frame_ref[0] = hdrcapture.screenshot()
        except Exception as err:  # pragma: no cover - diagnostic path
            exc[0] = err

    t = threading.Thread(target=worker_screenshot)
    t.start()
    _join_or_fail(t, timeout=15)

    assert exc[0] is None
    assert frame_ref[0] is not None
