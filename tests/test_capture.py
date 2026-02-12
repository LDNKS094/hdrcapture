import hdrcapture
import numpy as np
# 1. screenshot
frame = hdrcapture.screenshot()
print(repr(frame))                    # CapturedFrame(5120x1440, timestamp=xxx)
frame.save("test_screenshot.png")
# 2. numpy 转换
arr = frame.ndarray()
print(f"shape={arr.shape}, dtype={arr.dtype}")
arr2 = np.array(frame)
print(f"__array__ works: {np.array_equal(arr, arr2)}")
# 3. Capture 类
with hdrcapture.Capture.monitor(0) as cap:
    f1 = cap.capture()
    f2 = cap.grab()
    print(f"capture: {repr(f1)}")
    print(f"grab: {repr(f2)}")
# 4. close 后报错
cap = hdrcapture.Capture.monitor(0)
cap.close()
try:
    cap.capture()
except RuntimeError as e:
    print(f"close works: {e}")