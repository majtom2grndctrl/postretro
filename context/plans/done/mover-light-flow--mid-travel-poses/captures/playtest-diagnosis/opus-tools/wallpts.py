# Luma at world points on the moving wall's north face (y = 1072), sampled
# by projecting through the wall_back camera. Regions are in wall-local x
# (distance east of the trailing edge x_e = 96 + 544 t) and z.
#   python -I wallpts.py cols lo:hi:step
import math, os, sys
import numpy as np
from PIL import Image
O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import label, parse_ts, VIEWS, W, H

VIEW = os.environ.get('VIEW', 'wall_back')
REGIONS = {
    # name: (local x range, z range, y)
    'edge_lo': ((2, 30), (8, 104), 1072.5),     # north face, lower row, next to the edge
    'edge_hi': ((2, 30), (120, 216), 1072.5),   # north face, upper row, next to the edge
    'sq_hi0': ((10, 126), (122, 214), 1072.5),  # upper square nearest the edge (panel c0 r1)
    'sq_hi1': ((146, 262), (122, 214), 1072.5), # next upper square (c1 r1)
    'sq_lo0': ((10, 126), (10, 102), 1072.5),
    'sq_lo1': ((146, 262), (10, 102), 1072.5),
}


def project(pose, pts):
    (cx, cy, cz), yaw, pitch, fov = pose
    sy, cyw = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    sp, cp = math.sin(math.radians(pitch)), math.cos(math.radians(pitch))
    f = np.array((cyw * cp, sy * cp, sp)); r = np.array((sy, -cyw, 0.0)); u = np.array((-cyw * sp, -sy * sp, cp))
    hx = math.tan(math.radians(fov) / 2); hy = hx * H / W
    d = pts - np.array((cx, cy, cz))
    zf = d @ f
    nx = (d @ r) / zf / hx; ny = (d @ u) / zf / hy
    px = (nx + 1) / 2 * W; py = (1 - ny) / 2 * H
    ok = (zf > 1) & (px >= 0) & (px < W) & (py >= 0) & (py < H)
    return px, py, ok


def region_luma(t, col, reg):
    (lx0, lx1), (z0, z1), y = REGIONS[reg]
    xe = 96 + 544 * t
    xs, zs = np.meshgrid(np.linspace(lx0, lx1, 24), np.linspace(z0, z1, 24))
    pts = np.stack([xe + xs.ravel(), np.full(xs.size, y), zs.ravel()], 1)
    px, py, ok = project(VIEWS[VIEW](t)['pose'], pts)
    if ok.sum() < 50:
        return float('nan')
    a = np.asarray(Image.open(os.path.join(O, 'cap', label(VIEW, t, col) + '.png')).convert('RGB'), dtype=np.float64)
    L = 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
    return float(L[py[ok].astype(int), px[ok].astype(int)].mean())


if __name__ == '__main__':
    cols = sys.argv[1].split(','); ts = parse_ts(sys.argv[2])
    regs = sys.argv[3].split(',') if len(sys.argv) > 3 else list(REGIONS)
    print('t      ' + ' '.join(f'{c[:5]}:{r:>7s}' for r in regs for c in cols))
    for t in ts:
        print(f'{t:.4f} ' + ' '.join(f'{region_luma(t, c, r):13.2f}' for r in regs for c in cols))
