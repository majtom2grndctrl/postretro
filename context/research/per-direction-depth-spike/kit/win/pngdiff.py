#!/usr/bin/env python3
"""Pixel diff between capture pairs: pngdiff.py <dir> <a> <b> [<a> <b> ...], luma values."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pngstat


def luma_rows(img):
    w, h, ch, rows = img
    return [[0.2126 * r[i] + 0.7152 * r[i + 1] + 0.0722 * r[i + 2] for i in range(0, w * ch, ch)] for r in rows]


def diff(a, b):
    la, lb = luma_rows(pngstat.read_png(a)), luma_rows(pngstat.read_png(b))
    n = darker = brighter = 0
    worst = 0.0
    worst_at = None
    total = 0.0
    big = 0
    for y, (ra, rb) in enumerate(zip(la, lb)):
        for x, (p, q) in enumerate(zip(ra, rb)):
            d = q - p
            n += 1
            total += abs(d)
            if abs(d) > 2:
                big += 1
                darker += d < 0
                brighter += d > 0
            if abs(d) > abs(worst):
                worst, worst_at = d, (x, y)
    return f"mean|d| {total / n:.3f}  max {worst:+.1f} at {worst_at}  |d|>2: {big} px ({darker} darker, {brighter} brighter) of {n}"


if __name__ == "__main__":
    d = sys.argv[1]
    pairs = sys.argv[2:]
    for a, b in zip(pairs[0::2], pairs[1::2]):
        print(f"{a} -> {b}: {diff(os.path.join(d, a + '.png'), os.path.join(d, b + '.png'))}", flush=True)
