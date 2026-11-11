# Motion diagnosis capture driver: task 5's views (cap.py) plus a ceiling
# view, over a fine t' grid, for columns head | nodepth | two | strip.
#
#   venv/bin/python -I mcap.py one <view> <t> <col>
#   venv/bin/python -I mcap.py batch <jobs> [view ...]
import json, math, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from PIL import Image

M = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/motion'
REPO = '/Users/dhiester/Projects/Personal/postretro'
MAPS = os.path.join(M, 'ws/content/dev/maps')
CAP = os.path.join(M, 'cap')
BIN = os.path.join(M, 'bin/postretro')
W, H = 960, 540
UNIT = 0.0254
BLOCK = 8
COLUMNS = os.environ.get('MCOLS', 'head,nodepth,two,strip').split(',')


def floor_view(t):
    floor = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((860.0, 320.0, floor + 64.0), 0.0, -28.0, 90.0),
                plane=('z', floor), rect=((856.0, 1128.0), (184.0, 456.0)), follow=True)


def ceiling_view(t):
    # Room interior: x 848..1136, y 176..464; ceiling underside z -16 at t' 0,
    # rising 176 with the room. Camera 24 u above the floor, pitched up.
    floor = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((870.0, 320.0, floor + 24.0), 0.0, 60.0, 100.0),
                plane=('z', -16.0 + 176.0 * t), rect=((856.0, 1128.0), (184.0, 456.0)),
                follow=True)


def interior_view(t):
    floor = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((860.0, 320.0, floor + 72.0), 0.0, -8.0, 100.0),
                plane=None, rect=None, follow=True)


def west_view(t):
    return dict(map='enemy-movers', mover='em_sweeper',
                pose=((1100.0, 256.0, 140.0), 0.0, -20.0, 90.0),
                plane=('x', 1280.0 + 256.0 * t), rect=((72.0, 440.0), (8.0, 88.0)), follow=False)


def east_view(t):
    return dict(map='enemy-movers', mover='em_sweeper',
                pose=((1700.0, 256.0, 140.0), 180.0, -20.0, 90.0),
                plane=('x', 1312.0 + 256.0 * t), rect=((72.0, 440.0), (8.0, 88.0)), follow=False)


VIEWS = {'floor': floor_view, 'ceiling': ceiling_view, 'interior': interior_view,
         'west': west_view, 'east': east_view}


def grid(step):
    n = int(round(1.0 / step))
    return [round(step * i, 6) for i in range(n + 1)]


# Room knots 0, 0.2 ... 1 sit on the 0.02 grid (midpoints every 5th sample).
# Sweeper knots k/7 do not, so the face views use a 1/70 grid: knots every
# 10th sample, interval midpoints every 5th.
def ts_for(view):
    return grid(1.0 / 70.0) if view in ('west', 'east') else grid(0.02)


def label(view, t, col):
    return f'{view}-t{t:.3f}-{col}'


def capture(view, t, col):
    spec = VIEWS[view](t)
    name = label(view, t, col)
    out = os.path.join(CAP, name + '.png')
    if os.path.exists(out):
        return out, 0.0
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {
        'map': os.path.join(MAPS, f"{col}-{spec['map']}.prl"),
        'camera': {'position': [-y * UNIT, z * UNIT, -x * UNIT], 'yaw_deg': yaw,
                   'pitch_deg': pitch, 'fov_deg': fov},
        'resolution': [W, H],
        'output': out,
        'force_full_resident_sh_compose': True,
        'reflection_quality': 'off',
        'force_movers': [{'name': spec['mover'], 't': t}],
    }
    scene_path = os.path.join(CAP, name + '.scene.json')
    with open(scene_path, 'w') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    env.pop('POSTRETRO_MOVER_DEPTH_BLEND', None)
    t0 = time.time()
    result = subprocess.run(['nice', BIN, '--capture', scene_path], cwd=REPO, env=env,
                            capture_output=True, text=True)
    dt = time.time() - t0
    if result.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {name} failed rc={result.returncode}\n{result.stdout[-2000:]}\n{result.stderr[-4000:]}')
    if result.stderr.strip():
        with open(os.path.join(CAP, name + '.stderr.txt'), 'w') as f:
            f.write(result.stderr)
    return out, dt


def read_luma(path):
    a = np.asarray(Image.open(path).convert('RGB'), dtype=np.float64)
    assert a.shape == (H, W, 3), path
    return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]


def rays(pose):
    (cx, cy, cz), yaw, pitch, fov = pose
    sy, cyw = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    sp, cp = math.sin(math.radians(pitch)), math.cos(math.radians(pitch))
    forward = np.array((cyw * cp, sy * cp, sp))
    right = np.array((sy, -cyw, 0.0))
    up = np.array((-cyw * sp, -sy * sp, cp))
    half_x = math.tan(math.radians(fov) / 2.0)
    half_y = half_x * H / W
    return forward, right, up, half_x, half_y


def hit(spec, px, py):
    """World hit on the crop plane of rays through pixel coords (arrays)."""
    forward, right, up, hx, hy = rays(spec['pose'])
    nx = 2.0 * px / W - 1.0
    ny = 1.0 - 2.0 * py / H
    d = (forward[None, :] + right[None, :] * (nx * hx)[:, None] + up[None, :] * (ny * hy)[:, None])
    axis, value = spec['plane']
    a = {'x': 0, 'y': 1, 'z': 2}[axis]
    o = np.array(spec['pose'][0])
    with np.errstate(divide='ignore', invalid='ignore'):
        s = (value - o[a]) / d[:, a]
    p = o[None, :] + s[:, None] * d
    others = [i for i in range(3) if i != a]
    (u0, u1), (v0, v1) = spec['rect']
    u, v = p[:, others[0]], p[:, others[1]]
    ok = (s > 0) & (u >= u0) & (u <= u1) & (v >= v0) & (v <= v1)
    return ok, u, v


def crop_blocks(spec):
    """8x8 pixel blocks whose four corners hit the plane inside the rect (cap.py's rule)."""
    bx, by = np.meshgrid(np.arange(W // BLOCK), np.arange(H // BLOCK))
    bx, by = bx.ravel(), by.ravel()
    ok = np.ones(bx.shape, bool)
    for dx in (0, BLOCK):
        for dy in (0, BLOCK):
            o, _, _ = hit(spec, (bx * BLOCK + dx).astype(float), (by * BLOCK + dy).astype(float))
            ok &= o
    mask = np.zeros((H // BLOCK, W // BLOCK), bool)
    mask[by[ok], bx[ok]] = True
    return mask


def block_means(luma):
    hb = H // BLOCK
    return luma[:hb * BLOCK].reshape(hb, BLOCK, W // BLOCK, BLOCK).mean(axis=(1, 3))


CELL = 8.0  # world units, face-space cells for the fixed-camera face views


def face_cells(spec, luma):
    """Mean luma per CELL x CELL face-space cell (u, v on the plane); NaN where
    fewer than 12 pixel centres land in the cell."""
    py, px = np.mgrid[0:H, 0:W]
    ok, u, v = hit(spec, px.ravel() + 0.5, py.ravel() + 0.5)
    (u0, u1), (v0, v1) = spec['rect']
    nu, nv = int((u1 - u0) // CELL), int((v1 - v0) // CELL)
    iu = np.floor((u - u0) / CELL).astype(int)
    iv = np.floor((v - v0) / CELL).astype(int)
    ok &= (iu >= 0) & (iu < nu) & (iv >= 0) & (iv < nv)
    idx = iv[ok] * nu + iu[ok]
    sums = np.bincount(idx, weights=luma.ravel()[ok], minlength=nu * nv)
    cnt = np.bincount(idx, minlength=nu * nv)
    out = np.where(cnt >= 12, sums / np.maximum(cnt, 1), np.nan)
    return out.reshape(nv, nu)


def excess_metric(values, mask):
    """cap.py's 12c/12d excess: value minus median of crop blocks within 6
    (Chebyshev); peaks/pits = local max/min past +/-3."""
    hh, ww = values.shape
    ex = np.full(values.shape, np.nan)
    for y, x in zip(*np.nonzero(mask)):
        y0, y1, x0, x1 = max(0, y - 6), min(hh, y + 7), max(0, x - 6), min(ww, x + 7)
        nb = values[y0:y1, x0:x1][mask[y0:y1, x0:x1]]
        ex[y, x] = values[y, x] - np.median(nb)
    peaks = pits = 0
    for y, x in zip(*np.nonzero(mask)):
        nb = [values[y + dy, x + dx] for dy in (-1, 0, 1) for dx in (-1, 0, 1)
              if (dx or dy) and 0 <= y + dy < hh and 0 <= x + dx < ww and mask[y + dy, x + dx]]
        if ex[y, x] > 3 and all(values[y, x] >= n for n in nb):
            peaks += 1
        if ex[y, x] < -3 and all(values[y, x] <= n for n in nb):
            pits += 1
    e = ex[mask]
    return dict(bright=float(np.max(e)), peaks=peaks, dark=float(-np.min(e)), pits=pits)


def run_batch(jobs, views):
    tasks = [(v, t, c) for v in views for t in ts_for(v) for c in COLUMNS
             if not os.path.exists(os.path.join(CAP, label(v, t, c) + '.png'))]
    print(f'{len(tasks)} captures to run, {jobs} at a time', flush=True)
    start = time.time()
    done = 0
    with ThreadPoolExecutor(jobs) as ex:
        futs = {ex.submit(capture, *task): task for task in tasks}
        try:
            for fut in futs:
                fut.result()  # raises on the first failure in submission order
                done += 1
                if done % 20 == 0:
                    el = time.time() - start
                    print(f'{done}/{len(tasks)} {el:.0f}s, eta {el / done * (len(tasks) - done):.0f}s',
                          flush=True)
        except Exception as e:
            for f in futs:
                f.cancel()
            print('ABORT', e, flush=True)
            sys.exit(1)
    print(f'batch done in {time.time() - start:.0f}s', flush=True)


if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'one':
        print(capture(sys.argv[2], float(sys.argv[3]), sys.argv[4]))
    elif sys.argv[1] == 'batch':
        run_batch(int(sys.argv[2]), sys.argv[3:] or list(VIEWS))
