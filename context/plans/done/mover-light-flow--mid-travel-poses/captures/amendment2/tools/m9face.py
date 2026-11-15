# Face-anchored luma series: sample a moving mover face on a fixed (u, v) grid in its own map
# coordinates, so a cell follows the same patch of face at every t'.
#   cells(view, face, t) -> {(i, j): (px, py)}; series(view, face, col, ts, mask) -> array (nt, ni, nj)
import os, sys
import numpy as np
from PIL import Image
sys.path.insert(0, r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools')
import m9cap, acap  # noqa: E402

W, H = acap.W, acap.H
CELL = 16.0
# face: (plane axis, plane value at t, u axis range, v axis range, normal sign); map units.
FACES = {
    'sweep_east': dict(axis=0, at=lambda t: 1312.0 + 256.0 * t, u=(1, 64.0, 448.0), v=(2, 0.0, 96.0), n=+1),
    'sweep_top': dict(axis=2, at=lambda t: 96.0, u=(1, 64.0, 448.0), v=(0, 1280.0, 1312.0), n=+1, slide=0),
}


def project(view, t, P):
    spec = acap.VIEWS[view](t)
    C, yaw, pitch, fov = np.array(spec['pose'][0]), *spec['pose'][1:]
    y, p = np.radians(yaw), np.radians(pitch)
    f = np.array([np.cos(y) * np.cos(p), np.sin(y) * np.cos(p), np.sin(p)])
    r = np.array([np.sin(y), -np.cos(y), 0.0]); u = np.cross(r, f)
    th = np.tan(np.radians(fov) / 2.0); tv = th / (W / H)
    d = P - C
    z = d @ f
    xn = (d @ r) / z / th; yn = (d @ u) / z / tv
    return (xn + 1) / 2 * W - 0.5, (1 - yn) / 2 * H - 0.5, z


def grid(face):
    F = FACES[face]
    us = np.arange(F['u'][1] + CELL / 2, F['u'][2], CELL)
    vs = np.arange(F['v'][1] + CELL / 2, F['v'][2], CELL)
    return us, vs


def cells(view, face, t):
    F = FACES[face]
    us, vs = grid(face)
    out = np.zeros((len(us), len(vs), 2))
    for i, a in enumerate(us):
        for j, b in enumerate(vs):
            P = np.zeros(3)
            P[F['axis']] = F['at'](t) + 0.25 * F['n']
            P[F['u'][0]] = a; P[F['v'][0]] = b
            px, py, z = project(view, t, P)
            out[i, j] = (px, py) if z > 0 else (np.nan, np.nan)
    return out


def luma(path):
    a = np.asarray(Image.open(path).convert('RGB'), dtype=np.float64)
    return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]


def series(view, face, col, ts, mask=None, rad=1):
    us, vs = grid(face)
    out = np.full((len(ts), len(us), len(vs)), np.nan)
    for k, t in enumerate(ts):
        p = m9cap.path(view, t, col, mask)
        if not os.path.exists(p):
            continue
        L = luma(p)
        c = cells(view, face, t)
        for i in range(len(us)):
            for j in range(len(vs)):
                px, py = c[i, j]
                if np.isnan(px): continue
                x, y = int(round(px)), int(round(py))
                if rad <= x < W - rad and rad <= y < H - rad:
                    out[k, i, j] = L[y - rad:y + rad + 1, x - rad:x + rad + 1].mean()
    return out
