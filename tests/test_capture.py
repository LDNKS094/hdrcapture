"""hdrcapture Python API functional verification + timing stats"""

import os
import time
import tracemalloc
import hdrcapture
import numpy as np


def timed(label, fn):
    """Execute fn and print elapsed time"""
    t0 = time.perf_counter()
    result = fn()
    dt = (time.perf_counter() - t0) * 1000
    print(f"  {label}: {dt:.2f}ms")
    return result


def main():
    os.makedirs("tests/results", exist_ok=True)
    print("=== Functional Verification ===\n")

    # 1. screenshot (cold start) + multi-format save
    print("[1] screenshot() + multi-format save")
    frame = timed("cold start", lambda: hdrcapture.screenshot())
    print(f"  {repr(frame)}")
    formats = {
        "png": "tests/results/test_screenshot.png",
        "bmp": "tests/results/test_screenshot.bmp",
        "jpg": "tests/results/test_screenshot.jpg",
        "tiff": "tests/results/test_screenshot.tiff",
        "jxr": "tests/results/test_screenshot.jxr",
        "exr": "tests/results/test_screenshot.exr",
    }
    for fmt, path in formats.items():
        timed(f"save {fmt}", lambda p=path: frame.save(p))
        size = os.path.getsize(path)
        print(f"    {fmt}: {size} bytes")

    # 2. numpy conversion (bgra8)
    print("\n[2] numpy conversion (bgra8)")
    arr = timed("ndarray()", lambda: frame.ndarray())
    print(f"  shape={arr.shape}, dtype={arr.dtype}")
    assert arr.shape == (frame.height, frame.width, 4)
    assert arr.dtype == np.uint8
    arr2 = timed("np.array()", lambda: np.array(frame))
    print(f"  __array__ match: {np.array_equal(arr, arr2)}")

    # 3. Capture class — capture + grab
    print("\n[3] Capture class")
    cap = timed("capture.monitor(0)", lambda: hdrcapture.capture.monitor(0))
    f1 = timed("capture()", lambda: cap.capture())
    f2 = timed("grab()", lambda: cap.grab())
    f3 = timed("grab()", lambda: cap.grab())
    print(f"  capture: {repr(f1)}")
    print(f"  grab:    {repr(f2)}")
    print(f"  grab:    {repr(f3)}")
    cap.close()

    # 4. context manager
    print("\n[4] context manager")
    with hdrcapture.capture.monitor(0) as cap2:
        f = timed("capture()", lambda: cap2.capture())
        print(f"  {repr(f)}")
    try:
        cap2.capture()
        print("  ERROR: should have raised")
    except RuntimeError as e:
        print(f"  close works: {e}")

    # 5. Continuous capture performance
    print("\n[5] Continuous capture performance (20 rounds)")
    cap3 = hdrcapture.capture.monitor(0)
    _ = cap3.grab()  # warm up

    times = []
    for _ in range(20):
        t0 = time.perf_counter()
        cap3.grab()
        times.append((time.perf_counter() - t0) * 1000)
    cap3.close()

    times.sort()
    print(f"  min:  {times[0]:.2f}ms")
    print(f"  p50:  {times[len(times)//2]:.2f}ms")
    print(f"  p95:  {times[int(len(times)*0.95)]:.2f}ms")
    print(f"  max:  {times[-1]:.2f}ms")
    print(f"  avg:  {sum(times)/len(times):.2f}ms")

    # 6. Error handling
    print("\n[6] Error handling")
    try:
        hdrcapture.screenshot(monitor=999)
        print("  ERROR: should have raised")
    except RuntimeError as e:
        print(f"  invalid monitor: {e}")

    try:
        hdrcapture.screenshot(window="__nonexistent_process_12345__.exe")
        print("  ERROR: should have raised")
    except RuntimeError as e:
        print(f"  invalid window: {e}")

    try:
        hdrcapture.capture.window("__nonexistent_process_12345__.exe")
        print("  ERROR: should have raised")
    except RuntimeError as e:
        print(f"  invalid window (pipeline): {e}")

    # 7. Capture modes (auto / sdr / hdr)
    print("\n[7] Capture modes")

    # SDR mode
    frame_sdr = timed("screenshot(mode='sdr')", lambda: hdrcapture.screenshot(mode="sdr"))
    print(f"  sdr: {repr(frame_sdr)}")
    assert frame_sdr.format == "bgra8"

    # Auto mode
    frame_auto = timed("screenshot(mode='auto')", lambda: hdrcapture.screenshot(mode="auto"))
    print(f"  auto: {repr(frame_auto)}")
    assert frame_auto.format == "bgra8"  # auto tone-maps to bgra8

    # HDR mode
    try:
        frame_hdr = timed("screenshot(mode='hdr')", lambda: hdrcapture.screenshot(mode="hdr"))
        print(f"  hdr: {repr(frame_hdr)}")
        # On HDR monitor: rgba16f; on SDR monitor: still works but may be bgra8
        if frame_hdr.format == "rgba16f":
            print("  HDR monitor detected — testing rgba16f path")
            frame_hdr.save("tests/results/test_hdr.jxr")
            frame_hdr.save("tests/results/test_hdr.exr")
            print(f"    jxr: {os.path.getsize('tests/results/test_hdr.jxr')} bytes")
            print(f"    exr: {os.path.getsize('tests/results/test_hdr.exr')} bytes")
        else:
            print(f"  SDR monitor — hdr mode returned {frame_hdr.format}")
    except RuntimeError as e:
        print(f"  hdr mode error (may be expected on SDR): {e}")

    # Invalid mode
    try:
        hdrcapture.screenshot(mode="invalid")
        print("  ERROR: should have raised")
    except RuntimeError as e:
        print(f"  invalid mode: {e}")

    # 8. is_hdr property
    print("\n[8] is_hdr property")
    with hdrcapture.capture.monitor(0) as cap4:
        print(f"  monitor 0 is_hdr: {cap4.is_hdr}")
        assert isinstance(cap4.is_hdr, bool)

    # 9. numpy conversion (rgba16f)
    print("\n[9] numpy conversion (rgba16f)")
    try:
        with hdrcapture.capture.monitor(0, mode="hdr") as cap5:
            hdr_frame = cap5.capture()
            if hdr_frame.format == "rgba16f":
                arr_hdr = timed("ndarray()", lambda: hdr_frame.ndarray())
                print(f"  shape={arr_hdr.shape}, dtype={arr_hdr.dtype}")
                assert arr_hdr.shape == (hdr_frame.height, hdr_frame.width, 4)
                assert arr_hdr.dtype == np.float16
                arr_hdr2 = timed("np.array()", lambda: np.array(hdr_frame))
                print(f"  __array__ match: {np.array_equal(arr_hdr, arr_hdr2)}")
                # Sanity: values should be finite and non-negative for typical desktop
                finite_ratio = np.isfinite(arr_hdr).mean()
                print(f"  finite ratio: {finite_ratio:.4f}")
                assert finite_ratio > 0.99, f"Too many non-finite values: {finite_ratio}"
            else:
                print(f"  SKIP: monitor returned {hdr_frame.format} (not HDR)")
    except RuntimeError as e:
        print(f"  SKIP: HDR not available: {e}")

    # 10. Long streaming — memory stability
    print("\n[10] Long streaming memory stability (100 rounds)")
    tracemalloc.start()
    cap6 = hdrcapture.capture.monitor(0)
    _ = cap6.grab()  # warm up

    snapshot1 = tracemalloc.take_snapshot()
    for i in range(100):
        f = cap6.grab()
        # Force ndarray conversion to exercise full path
        _ = f.ndarray()
    snapshot2 = tracemalloc.take_snapshot()
    cap6.close()

    stats = snapshot2.compare_to(snapshot1, "lineno")
    total_diff = sum(s.size_diff for s in stats)
    print(f"  Python memory delta: {total_diff / 1024:.1f} KB over 100 frames")
    # Allow some growth for Python internals, but flag large leaks
    if total_diff > 50 * 1024 * 1024:  # 50 MB threshold
        print(f"  WARNING: possible memory leak ({total_diff / 1024 / 1024:.1f} MB)")
    else:
        print("  OK: no significant memory growth")
    tracemalloc.stop()

    print("\n=== DONE ===")


if __name__ == "__main__":
    main()
