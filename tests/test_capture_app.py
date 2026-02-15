import hdrcapture as hc
import time


def main():
    st = time.perf_counter()
    try:
        with hc.capture.window("endfield.exe", index=0) as cap:
            counter = 0
            while True:
                cap.grab().save(f"tests/test/test_endfield_{counter}.png")
                time.sleep(0.5)
                counter += 1
                ed = time.perf_counter()
                print(f"Using {(ed-st)*1000:.2f}ms")
                st = ed
            
    except RuntimeError as e:
        print(f"Skip Endfield window capture: {e}")



    """ try:
        hc.screenshot(window="Client-Win64-Shipping.exe").save("test_wuwa_0.png")
    except RuntimeError as e:
        print(f"Skip WUWA window screenshot: {e}")

    hc.screenshot().save("test_monitor_0.png") """


if __name__ == "__main__":
    main()
