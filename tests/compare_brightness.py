"""Compare brightness/color of monitor_0.png vs sdr_test.png (ground truth)."""

import numpy as np
from PIL import Image

def analyze(path):
    img = np.array(Image.open(path).convert("RGB")).astype(np.float32)
    r, g, b = img[:,:,0], img[:,:,1], img[:,:,2]
    luma = 0.2126 * r + 0.7152 * g + 0.0722 * b
    return {
        "path": path,
        "mean_r": r.mean(),
        "mean_g": g.mean(),
        "mean_b": b.mean(),
        "mean_luma": luma.mean(),
        "median_luma": np.median(luma),
        "p5_luma": np.percentile(luma, 5),
        "p95_luma": np.percentile(luma, 95),
        "min_luma": luma.min(),
        "max_luma": luma.max(),
    }

def main():
    ref = analyze("tests/results/sdr_test.png")
    test = analyze("tests/results/monitor_0.png")

    print(f"{'metric':<16} {'sdr_test(ref)':>14} {'monitor_0':>14} {'diff':>10}")
    print("-" * 58)
    for key in ["mean_r", "mean_g", "mean_b", "mean_luma", "median_luma", "p5_luma", "p95_luma", "min_luma", "max_luma"]:
        rv = ref[key]
        tv = test[key]
        diff = tv - rv
        print(f"{key:<16} {rv:>14.2f} {tv:>14.2f} {diff:>+10.2f}")

    # Sample a few pixels from center
    ref_img = np.array(Image.open(ref["path"]).convert("RGB"))
    test_img = np.array(Image.open(test["path"]).convert("RGB"))
    h, w = ref_img.shape[:2]
    print(f"\nCenter pixel samples (y, x) -> ref RGB vs test RGB:")
    for dy, dx in [(0,0), (0,100), (100,0), (-100,-100)]:
        y, x = h//2 + dy, w//2 + dx
        rp = ref_img[y, x]
        tp = test_img[y, x]
        print(f"  ({y:4d},{x:4d}): ref=({rp[0]:3d},{rp[1]:3d},{rp[2]:3d})  test=({tp[0]:3d},{tp[1]:3d},{tp[2]:3d})  diff=({tp[0]-rp[0]:+4d},{tp[1]-rp[1]:+4d},{tp[2]-rp[2]:+4d})")

if __name__ == "__main__":
    main()
