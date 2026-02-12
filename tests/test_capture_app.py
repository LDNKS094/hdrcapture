import hdrcapture as hc


def main():
    try:
        with hc.Capture.window("Endfield.exe") as cap:
            cap.grab().save("test_endfield.png")
    except RuntimeError as e:
        print(f"Skip Endfield window capture: {e}")

    try:
        hc.screenshot(window="Client-Win64-Shipping.exe").save("test_wuwa_0.png")
    except RuntimeError as e:
        print(f"Skip WUWA window screenshot: {e}")

    hc.screenshot().save("test_monitor_0.png")


if __name__ == "__main__":
    main()
