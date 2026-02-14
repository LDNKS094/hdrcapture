"""hdrcapture Python API functional verification + timing stats"""

import os
import time
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

    # 2. numpy conversion
    print("\n[2] numpy conversion")
    arr = timed("ndarray()", lambda: frame.ndarray())
    print(f"  shape={arr.shape}, dtype={arr.dtype}")
    arr2 = timed("np.array()", lambda: np.array(frame))
    print(f"  __array__ match: {np.array_equal(arr, arr2)}")

    # 3. Capture class — capture + grab
    print("\n[3] Capture class")
    cap = timed("Capture.monitor(0)", lambda: hdrcapture.capture.monitor(0))
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
    # cap2 should be closed now
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

    print("\n=== DONE ===")


if __name__ == "__main__":
    main()
