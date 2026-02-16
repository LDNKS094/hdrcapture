"""hdrcapture cross-thread safety verification

Validates that the worker-thread architecture allows Capture objects to be
freely shared across Python threads without panic or crash.
"""

import os
import sys
import time
import threading
import concurrent.futures
import hdrcapture


def timed(label, fn):
    t0 = time.perf_counter()
    result = fn()
    dt = (time.perf_counter() - t0) * 1000
    print(f"  {label}: {dt:.2f}ms")
    return result


def main():
    os.makedirs("tests/results", exist_ok=True)
    errors = []
    print("=== Cross-Thread Safety Tests ===\n")

    # 1. Create on main thread, use on worker thread
    print("[1] Create on main, capture on worker thread")
    cap = hdrcapture.capture.monitor(0)
    result = [None]
    exc = [None]

    def worker_capture():
        try:
            result[0] = cap.capture()
        except Exception as e:
            exc[0] = e

    t = threading.Thread(target=worker_capture)
    t.start()
    t.join(timeout=10)
    if exc[0]:
        print(f"  FAIL: {exc[0]}")
        errors.append(("1", exc[0]))
    elif result[0] is None:
        print("  FAIL: no frame returned")
        errors.append(("1", "no frame"))
    else:
        print(f"  OK: {repr(result[0])}")
    cap.close()

    # 2. Create on worker thread, use on main thread
    print("\n[2] Create on worker, capture on main thread")
    cap2 = [None]
    exc2 = [None]

    def worker_create():
        try:
            cap2[0] = hdrcapture.capture.monitor(0)
        except Exception as e:
            exc2[0] = e

    t2 = threading.Thread(target=worker_create)
    t2.start()
    t2.join(timeout=10)
    if exc2[0]:
        print(f"  FAIL create: {exc2[0]}")
        errors.append(("2", exc2[0]))
    else:
        try:
            frame = cap2[0].capture()
            print(f"  OK: {repr(frame)}")
        except Exception as e:
            print(f"  FAIL capture: {e}")
            errors.append(("2", e))
        finally:
            cap2[0].close()

    # 3. Concurrent grab from multiple threads (shared Capture)
    print("\n[3] Concurrent grab from 4 threads (10 frames each)")
    cap3 = hdrcapture.capture.monitor(0)
    _ = cap3.grab()  # warm up

    thread_results = {}
    thread_errors = {}
    lock = threading.Lock()

    def worker_grab(tid):
        local_frames = []
        local_err = None
        for _ in range(10):
            try:
                f = cap3.grab()
                local_frames.append(f.timestamp)
            except Exception as e:
                local_err = e
                break
        with lock:
            thread_results[tid] = local_frames
            if local_err:
                thread_errors[tid] = local_err

    threads = [threading.Thread(target=worker_grab, args=(i,)) for i in range(4)]
    t0 = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=30)
    dt = (time.perf_counter() - t0) * 1000

    total_frames = sum(len(v) for v in thread_results.values())
    print(f"  {total_frames} frames in {dt:.0f}ms across 4 threads")
    if thread_errors:
        for tid, e in thread_errors.items():
            print(f"  FAIL thread {tid}: {e}")
            errors.append(("3", e))
    else:
        print("  OK: no errors")
    cap3.close()

    # 4. Close from different thread than creator
    print("\n[4] Close from different thread")
    cap4 = hdrcapture.capture.monitor(0)
    _ = cap4.capture()
    exc4 = [None]

    def worker_close():
        try:
            cap4.close()
        except Exception as e:
            exc4[0] = e

    t4 = threading.Thread(target=worker_close)
    t4.start()
    t4.join(timeout=10)
    if exc4[0]:
        print(f"  FAIL: {exc4[0]}")
        errors.append(("4", exc4[0]))
    else:
        # Verify it's actually closed
        try:
            cap4.capture()
            print("  FAIL: should have raised after close")
            errors.append(("4", "no error after close"))
        except RuntimeError:
            print("  OK: closed from worker thread, main thread gets RuntimeError")

    # 5. Drop via atexit / GC from different thread (simulated)
    print("\n[5] Drop without explicit close (GC simulation)")
    cap5 = hdrcapture.capture.monitor(0)
    _ = cap5.capture()
    # Just drop the reference — should not panic
    del cap5
    import gc

    gc.collect()
    print("  OK: no panic on implicit drop")

    # 6. ThreadPoolExecutor — real-world pattern
    print("\n[6] ThreadPoolExecutor pattern")
    cap6 = hdrcapture.capture.monitor(0)
    _ = cap6.grab()  # warm up

    def pool_grab(_):
        return cap6.grab().timestamp

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        t0 = time.perf_counter()
        futures = [pool.submit(pool_grab, i) for i in range(20)]
        timestamps = [f.result(timeout=10) for f in futures]
        dt = (time.perf_counter() - t0) * 1000

    print(f"  20 frames via ThreadPoolExecutor: {dt:.0f}ms")
    print(f"  timestamps range: {min(timestamps):.3f}s — {max(timestamps):.3f}s")
    cap6.close()

    # 7. screenshot() from worker thread
    print("\n[7] screenshot() from worker thread")
    exc7 = [None]
    frame7 = [None]

    def worker_screenshot():
        try:
            frame7[0] = hdrcapture.screenshot()
        except Exception as e:
            exc7[0] = e

    t7 = threading.Thread(target=worker_screenshot)
    t7.start()
    t7.join(timeout=15)
    if exc7[0]:
        print(f"  FAIL: {exc7[0]}")
        errors.append(("7", exc7[0]))
    elif frame7[0] is None:
        print("  FAIL: no frame")
        errors.append(("7", "no frame"))
    else:
        print(f"  OK: {repr(frame7[0])}")

    # Summary
    print(f"\n=== {'FAIL' if errors else 'ALL PASSED'} ===")
    if errors:
        for test_id, e in errors:
            print(f"  test {test_id}: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
