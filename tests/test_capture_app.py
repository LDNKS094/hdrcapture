import hdrcapture as hc
import time


def main():
    st = time.perf_counter()
    try:
        with hc.capture.window("Endfield.exe") as cap:
            cap.grab().save("test_endfield.png")
            
    except RuntimeError as e:
        print(f"Skip Endfield window capture: {e}")

    ed = time.perf_counter()
    print(f"Using {(ed-st)*1000:.2f}ms")

    try:
        hc.screenshot(window="Client-Win64-Shipping.exe").save("test_wuwa_0.png")
    except RuntimeError as e:
        print(f"Skip WUWA window screenshot: {e}")

    hc.screenshot().save("test_monitor_0.png")


if __name__ == "__main__":
    main()
