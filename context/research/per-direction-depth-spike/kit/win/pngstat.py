#!/usr/bin/env python3
"""Dependency-free PNG stats for the closet captures: frame mean and floor profile."""
import math, struct, sys, zlib

S = 0.0254

def read_png(path):
    b = open(path, "rb").read()
    assert b[:8] == b"\x89PNG\r\n\x1a\n", path
    p, idat = 8, bytearray()
    while p < len(b):
        n, t = struct.unpack(">I4s", b[p:p + 8])
        d = b[p + 8:p + 8 + n]
        if t == b"IHDR":
            w, h, depth, ct, _, _, il = struct.unpack(">IIBBBBB", d)
            assert depth == 8 and il == 0 and ct in (2, 6), (depth, ct, il)
            ch = 3 if ct == 2 else 4
        elif t == b"IDAT":
            idat += d
        p += n + 12
    raw = zlib.decompress(bytes(idat))
    st = w * ch
    prev = bytearray(st)
    rows = []
    for y in range(h):
        o = y * (st + 1)
        f = raw[o]
        cur = bytearray(raw[o + 1:o + 1 + st])
        if f == 1:
            for i in range(ch, st):
                cur[i] = (cur[i] + cur[i - ch]) & 255
        elif f == 2:
            for i in range(st):
                cur[i] = (cur[i] + prev[i]) & 255
        elif f == 3:
            for i in range(st):
                a = cur[i - ch] if i >= ch else 0
                cur[i] = (cur[i] + ((a + prev[i]) >> 1)) & 255
        elif f == 4:
            for i in range(st):
                a = cur[i - ch] if i >= ch else 0
                c = prev[i - ch] if i >= ch else 0
                q = prev[i]
                pa, pb, pc = abs(q - c), abs(a - c), abs(a + q - 2 * c)
                pr = a if pa <= pb and pa <= pc else (q if pb <= pc else c)
                cur[i] = (cur[i] + pr) & 255
        rows.append(cur)
        prev = cur
    return w, h, ch, rows

def frame_mean(img):
    w, h, ch, rows = img
    # Rec. 709 luma: reproduces the Mac's table exactly (6.60 / 0.50 / 6.83 / 0.15).
    t = 0.0
    for r in rows:
        t += 0.2126 * sum(r[0::ch]) + 0.7152 * sum(r[1::ch]) + 0.0722 * sum(r[2::ch])
    return t / (w * h)

def project(pt, cam, yaw, pitch, fov, w, h, fov_is_vertical):
    """Map-unit point to pixel. Yaw 90 faces +y in map axes, z up."""
    d = [pt[i] - cam[i] for i in range(3)]
    cy, sy = math.cos(math.radians(yaw)), math.sin(math.radians(yaw))
    cp, sp = math.cos(math.radians(pitch)), math.sin(math.radians(pitch))
    fwd = (cy * cp, sy * cp, sp)
    right = (sy, -cy, 0.0)
    up = (-cy * sp, -sy * sp, cp)
    dot = lambda a, b: sum(x * y for x, y in zip(a, b))
    zc = dot(d, fwd)
    if zc <= 0:
        return None
    half = math.tan(math.radians(fov) / 2)
    hx, hy = (half * w / h, half) if fov_is_vertical else (half, half * h / w)
    return (w / 2 * (1 + dot(d, right) / zc / hx), h / 2 * (1 - dot(d, up) / zc / hy))

def floor_row(img, past_m, cam=(320, 610, 96), yaw=270, pitch=-38, fov=100, vertical=False,
              x0=272, x1=368, wall_y=512):
    """Mean sRGB across the doorway's width, past_m metres north of the closet wall."""
    w, h, ch, rows = img
    y = wall_y + past_m / S
    vals = []
    for i in range(49):
        x = x0 + (x1 - x0) * (i + 0.5) / 49
        p = project((x, y, 0.0), cam, yaw, pitch, fov, w, h, vertical)
        if p is None:
            continue
        px, py = int(p[0]), int(p[1])
        if 0 <= px < w and 0 <= py < h:
            r = rows[py]
            vals.append(0.2126 * r[px * ch] + 0.7152 * r[px * ch + 1] + 0.0722 * r[px * ch + 2])
    return (sum(vals) / len(vals)) if vals else float("nan")

if __name__ == "__main__":
    for f in sys.argv[1:]:
        im = read_png(f)
        print(f"{f}: {im[0]}x{im[1]} mean {frame_mean(im):.2f} floor",
              " ".join(f"{m}={floor_row(im, m):.1f}" for m in (0.04, 0.26, 0.48)))
