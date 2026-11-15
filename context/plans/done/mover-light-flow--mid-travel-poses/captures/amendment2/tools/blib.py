# Big-room re-capture (2026-11-13): views, pixel regions and loaders for setpiece_room_big.
# Scratch root: amend3 (A3_ROOT, else this file's scratch twin). Regions come from a per-pixel ray
# cast against the room's inner box in room-local (lowered-pose) coordinates, so a riding view's
# regions hold at every t'. Camera convention matches capture/driver.rs: horizontal FOV, yaw 0 = +x,
# pitch up positive, engine position = (-y, z, -x) * 0.0254.
import json, os, sys
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import acap  # noqa: E402  (door_ceiling, walledge, ... views; W, H, U)
import aseries  # noqa: E402  (door_ceiling A/B regions, blocks())

SCR = r'C:\Users\danhi\AppData\Local\Temp\claude\80d2a19114937a96\8904fe45-929f-4f76-b328-129186a2a8bd\scratchpad'
A3 = os.environ.get('A3_ROOT', os.path.join(SCR, 'amend3'))
CAP = os.path.join(A3, 'cap')
W, H, U, B = acap.W, acap.H, acap.U, 8

# setpiece_room_big, lowered pose: inner box, travel +z 272 at 1 m/s (39.37 u/s).
BOX = dict(x0=1536.0, x1=1920.0, y0=1360.0, y1=1792.0, z0=-272.0, z1=-112.0)
TRAVEL = 272.0
FRAME_T = 39.37 / 60.0 / TRAVEL          # one 60 fps frame in t' (0.002412)
# Arch opening in the south wall (y 1296-1360): voussoir bottoms, lowered pose.
ARCH = [(1678, -192), (1680, -178), (1688, -162), (1698, -152), (1714, -144), (1728, -142),
        (1742, -144), (1758, -152), (1768, -162), (1776, -178), (1778, -192)]
CELL = 39.37

# Riding views: (local position at the lowered pose, yaw, pitch, fov). z follows the room.
RIDE = {
    'bw': ((1640.0, 1576.0, -160.0), 180.0, 35.0, 100.0),      # west wall top + ceiling strip over it
    'bs': ((1728.0, 1464.0, -160.0), 270.0, 35.0, 100.0),      # south wall above the arch + ceiling strip
    'bfloor': ((1820.0, 1720.0, -180.0), 225.0, -30.0, 100.0),  # floor toward the south-west corner
    'broof': ((1760.0, 1640.0, 40.0), 225.0, -45.0, 100.0),    # above the roof top, down toward the SW corner
}
ROOF_TOP = -48.0
FOOT = dict(x0=1472.0, x1=1984.0, y0=1296.0, y1=1856.0)
# Fixed views (world positions).
FIXED = {
    'barch': ((1728.0, 1180.0, 80.0), 90.0, 0.0, 90.0),        # chamber, square on to the arch wall
    'bch': ((1400.0, 1200.0, 200.0), 45.0, -25.0, 100.0),       # chamber SW corner, down at the room
    'bhall': ((1728.0, 880.0, 48.0), 90.0, 0.0, 90.0),          # README station 4, hallway leg 2
}


def _view(pose, mover):
    return lambda t: dict(map='mover-setpiece', mover=mover, pose=pose(t))


for _n, ((x, y, z), yaw, pitch, fov) in RIDE.items():
    acap.VIEWS[_n] = _view(lambda t, x=x, y=y, z=z, yaw=yaw, pitch=pitch, fov=fov:
                           ((x, y, z + TRAVEL * t), yaw, pitch, fov), 'setpiece_room_big')
for _n, (p, yaw, pitch, fov) in FIXED.items():
    acap.VIEWS[_n] = _view(lambda t, p=p, yaw=yaw, pitch=pitch, fov=fov: (p, yaw, pitch, fov), 'setpiece_room_big')


def rays(yaw, pitch, fov):
    """Per-pixel unit directions (H, W, 3) in Quake axes."""
    y, p = np.radians(yaw), np.radians(pitch)
    f = np.array([np.cos(y) * np.cos(p), np.sin(y) * np.cos(p), np.sin(p)])
    r = np.array([np.sin(y), -np.cos(y), 0.0])
    u = np.cross(r, f)
    th = np.tan(np.radians(fov) / 2.0)
    tv = th / (W / H)
    xs = ((np.arange(W) + 0.5) / W * 2 - 1) * th
    ys = (1 - (np.arange(H) + 0.5) / H * 2) * tv
    d = f[None, None] + xs[None, :, None] * r[None, None] + ys[:, None, None] * u[None, None]
    return d / np.linalg.norm(d, axis=2, keepdims=True)


def arch_bottom(x):
    xs, zs = zip(*ARCH)
    return np.interp(x, xs, zs)


def cast_roof(view):
    """broof: 'T' where the pixel's ray meets the roof top inside the room's footprint, else 'X'."""
    (cx, cy, cz), yaw, pitch, fov = RIDE[view]
    d = rays(yaw, pitch, fov)
    with np.errstate(divide='ignore', invalid='ignore'):
        tt = (ROOF_TOP - cz) / d[..., 2]
    p = np.array([cx, cy, cz])[None, None] + d * tt[..., None]
    ok = (tt > 0) & (p[..., 0] > FOOT['x0']) & (p[..., 0] < FOOT['x1']) & (p[..., 1] > FOOT['y0']) & (p[..., 1] < FOOT['y1'])
    return np.where(ok, 'T', 'X'), p


def cast(view):
    """Riding view: face id per pixel ('W','E','S','N','F','C', 'O' = arch opening) and local hit point."""
    (cx, cy, cz), yaw, pitch, fov = RIDE[view]
    d = rays(yaw, pitch, fov)
    c = np.array([cx, cy, cz])
    lo = np.array([BOX['x0'], BOX['y0'], BOX['z0']]); hi = np.array([BOX['x1'], BOX['y1'], BOX['z1']])
    with np.errstate(divide='ignore', invalid='ignore'):
        tl = np.where(d > 0, (hi - c) / d, np.where(d < 0, (lo - c) / d, np.inf))
    ax = np.argmin(tl, axis=2)
    tt = np.min(tl, axis=2)
    p = c[None, None] + d * tt[..., None]
    sign = np.take_along_axis(d, ax[..., None], 2)[..., 0] > 0
    names = np.array([['W', 'E'], ['S', 'N'], ['F', 'C']])
    face = names[ax, sign.astype(int)]
    opening = (face == 'S') & (p[..., 0] > 1678) & (p[..., 0] < 1778) & (p[..., 2] < arch_bottom(p[..., 0]))
    face = np.where(opening, 'O', face)
    return face, p


_regions = None


def regions():
    """(view, region) -> bool mask. Bands follow the in-wall layers: west x 1521.5 (14.5 u inside the
    west wall), south y 1354.0 (6 u inside the south wall), and the lookup reach of one cell beside them."""
    global _regions
    if _regions is not None:
        return _regions
    R = {}
    for v in RIDE:
        if v == 'broof':
            f, p = cast_roof(v)
            x, y = p[..., 0], p[..., 1]
            R[(v, 'frame')] = np.ones((H, W), bool)
            R[(v, 'top')] = f == 'T'
            R[(v, 'topW')] = (f == 'T') & (x < 1560.9) & (y > 1400) & (y < 1760)
            R[(v, 'topS')] = (f == 'T') & (y < 1393.4) & (x > 1560) & (x < 1900)
            continue
        f, p = cast(v)
        x, y, z = p[..., 0], p[..., 1], p[..., 2]
        R[(v, 'frame')] = np.ones((H, W), bool)
        R[(v, 'ceil')] = f == 'C'
        # ceiling strip over the west wall (lookups between the in-wall layer and the next one in)
        R[(v, 'W')] = (f == 'C') & (x < 1560.9) & (y > 1400) & (y < 1760)
        # ceiling strip over the solid south wall, both sides of the arch
        R[(v, 'S')] = (f == 'C') & (y < 1393.4) & (((x > 1548) & (x < 1668)) | ((x > 1788) & (x < 1908)))
        # ceiling strip over the arch, and the wall face above the arch
        R[(v, 'A')] = (f == 'C') & (y < 1393.4) & (x > 1682) & (x < 1774)
        R[(v, 'AF')] = (f == 'S') & (x > 1690) & (x < 1766) & (z > -138)
        R[(v, 'WF')] = (f == 'W') & (y > 1400) & (y < 1760)
        R[(v, 'SF')] = (f == 'S') & ((x < 1668) | (x > 1788))
        R[(v, 'FW')] = (f == 'F') & (x < 1560.9) & (y > 1400) & (y < 1760)
        R[(v, 'FS')] = (f == 'F') & (y < 1393.4) & (x > 1560) & (x < 1900)
        R[(v, 'floor')] = f == 'F'
    for v in FIXED:
        R[(v, 'frame')] = np.ones((H, W), bool)
    for (v, r), m in list(aseries.REGIONS.items()):
        R[(v, r)] = m
    for k, val in json.load(open(os.path.join(HERE, 'dregions.json'), encoding='utf-8')).items():
        v, r = k.split('/')
        R[(v, r)] = aseries.rect(*val['rect'])
    _regions = R
    return R


def path(view, t, col, mask=None):
    return os.path.join(CAP, f'{view}-t{t:.5f}-{col}' + (f'-m{mask}' if mask else '') + '.png')


_cache = {}


def luma(view, t, col, mask=None):
    p = path(view, t, col, mask)
    if p in _cache:
        return _cache[p]
    if not os.path.exists(p):
        return None
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)
    L = (0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]).astype(np.float32)
    if len(_cache) > 1500:
        _cache.clear()
    _cache[p] = L
    return L


def rgb(view, t, col, mask=None):
    p = path(view, t, col, mask)
    return None if not os.path.exists(p) else np.asarray(Image.open(p).convert('RGB'))


def blocks(L, m):
    return aseries.blocks(L, m)


def series(view, region, col, ts, mask=None):
    m = regions()[(view, region)]
    means, bl = [], []
    for t in ts:
        L = luma(view, t, col, mask)
        if L is None:
            means.append(None); bl.append(None); continue
        means.append(float(L[m].mean())); bl.append(blocks(L, m))
    return means, bl


def trip_ts(step=1):
    """Whole trip at one sample per `step` 60 fps frames, ending exactly on t' 1."""
    n = int(np.ceil(1.0 / (FRAME_T * step)))
    return sorted({round(min(1.0, i * FRAME_T * step), 5) for i in range(n + 1)} | {1.0})


if __name__ == '__main__':
    # Region overlay sheet: python blib.py overlay <view> <png> <out>
    if sys.argv[1] == 'overlay':
        v, src, out = sys.argv[2], sys.argv[3], sys.argv[4]
        a = np.asarray(Image.open(src).convert('RGB'), dtype=np.float32)
        R = regions()
        cols = {'W': (255, 0, 0), 'S': (0, 160, 255), 'A': (255, 200, 0), 'AF': (0, 255, 0), 'WF': (255, 0, 255),
                'SF': (0, 255, 255), 'FW': (255, 120, 0), 'FS': (120, 0, 255), 'topW': (255, 0, 0), 'topS': (0, 160, 255)}
        for r, c in cols.items():
            if (v, r) in R:
                m = R[(v, r)]
                a[m] = a[m] * 0.5 + np.array(c) * 0.5
        f, _ = (cast_roof(v) if v == 'broof' else cast(v)) if v in RIDE else (None, None)
        if f is not None:
            edge = np.zeros((H, W), bool)
            edge[:, 1:] |= f[:, 1:] != f[:, :-1]; edge[1:] |= f[1:] != f[:-1]
            a[edge] = (255, 255, 255)
        Image.fromarray(np.clip(a, 0, 255).astype(np.uint8)).save(out)
        for r in cols:
            if (v, r) in R:
                m = R[(v, r)]
                print(v, r, int(m.sum()), 'px', len(blocks(np.zeros((H, W), np.float32), m)), 'blocks')
