# Task 12c's eighth-power metric (rising-room floor lattice dots) per form.
#   python afloor.py <bakes> <forms> <ts>
# Per capture: crop = 8x8 blocks wholly on the interior floor (inset 8 u);
# excess = block minus the median of crop blocks within 6; reported as max
# excess / count of local-max blocks more than 3 levels over that median.
import math, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, aseries
W, H, BLOCK = acap.W, acap.H, 8

def hit(pose, plane_z, px, py, rect):
    (cx, cy, cz), yaw, pitch, fov = pose
    sy, cyw = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    sp, cp = math.sin(math.radians(pitch)), math.cos(math.radians(pitch))
    f = np.array((cyw * cp, sy * cp, sp)); r = np.array((sy, -cyw, 0.0)); u = np.array((-cyw * sp, -sy * sp, cp))
    hx = math.tan(math.radians(fov) / 2.0); hy = hx * H / W
    nx = 2.0 * px / W - 1.0; ny = 1.0 - 2.0 * py / H
    d = f[None] + r[None] * (nx * hx)[:, None] + u[None] * (ny * hy)[:, None]
    s = (plane_z - cz) / d[:, 2]
    p = np.array((cx, cy, cz))[None] + s[:, None] * d
    (u0, u1), (v0, v1) = rect
    return (s > 0) & (p[:, 0] >= u0) & (p[:, 0] <= u1) & (p[:, 1] >= v0) & (p[:, 1] <= v1)

def crop(t):
    pose = acap.VIEWS['floor'](t)['pose']; z = -176.0 + 176.0 * t
    bx, by = np.meshgrid(np.arange(W // BLOCK), np.arange(H // BLOCK)); bx, by = bx.ravel(), by.ravel()
    ok = np.ones(bx.shape, bool)
    for dx in (0, BLOCK):
        for dy in (0, BLOCK):
            ok &= hit(pose, z, (bx * BLOCK + dx).astype(float), (by * BLOCK + dy).astype(float), ((856.0, 1128.0), (184.0, 456.0)))
    m = np.zeros((H // BLOCK, W // BLOCK), bool); m[by[ok], bx[ok]] = True
    return m

def excess(values, mask):
    hh, ww = values.shape
    ex = np.full(values.shape, np.nan)
    for y, x in zip(*np.nonzero(mask)):
        y0, y1, x0, x1 = max(0, y - 6), min(hh, y + 7), max(0, x - 6), min(ww, x + 7)
        ex[y, x] = values[y, x] - np.median(values[y0:y1, x0:x1][mask[y0:y1, x0:x1]])
    peaks = 0
    for y, x in zip(*np.nonzero(mask)):
        nb = [values[y + dy, x + dx] for dy in (-1, 0, 1) for dx in (-1, 0, 1)
              if (dx or dy) and 0 <= y + dy < hh and 0 <= x + dx < ww and mask[y + dy, x + dx]]
        if ex[y, x] > 3 and all(values[y, x] >= n for n in nb):
            peaks += 1
    return float(np.nanmax(ex[mask])), peaks, ex

if __name__ == '__main__':
    bakes, forms, ts = sys.argv[1].split(','), sys.argv[2].split(','), acap.parse_ts(sys.argv[3])
    print('t      ' + ' '.join(f'{b}/{f:>4s}' .rjust(14) for b in bakes for f in forms))
    for t in ts:
        m = crop(t); row = f'{t:.3f} '
        for b in bakes:
            for f in forms:
                L = aseries.load('floor', t, b, f)
                bm = L[:H // BLOCK * BLOCK].reshape(H // BLOCK, BLOCK, W // BLOCK, BLOCK).mean(axis=(1, 3))
                mx, pk, _ = excess(bm, m)
                row += f'{mx:9.1f} / {pk:3d}'
        print(row, f'(blocks {int(m.sum())})')
