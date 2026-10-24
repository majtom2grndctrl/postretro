"""2D model of the texel footprint floor: cone kernel radius 1, probe early-out.

Compares probe layouts on hard half-plane edges (P16 adjacency + snap) and thin
strips (P3 centre-blocked + bilinear gaps along the line).
"""
import math
import sys
import numpy as np

GOLDEN = math.pi * (3.0 - math.sqrt(5.0))

def cone_cdf_inv(u):
    # F(r) = 3r^2 - 2r^3 on [0,1]; bisection
    lo, hi = 0.0, 1.0
    for _ in range(60):
        mid = 0.5 * (lo + hi)
        if 3 * mid * mid - 2 * mid ** 3 < u:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)

# Edge-spread function of the cone kernel: fraction of kernel weight with x < d.
_xs = np.linspace(-1, 1, 4001)
_ys = np.linspace(-1, 1, 4001)
_X, _Y = np.meshgrid(_xs, _ys)
_R = np.sqrt(_X ** 2 + _Y ** 2)
_W = np.clip(1 - _R, 0, None)
_proj = _W.sum(axis=0)
_cum = np.cumsum(_proj) / _proj.sum()
def esf(d):
    return np.interp(d, _xs, _cum, left=0.0, right=1.0)

def layout(n, design, rho):
    """Return (offsets[n,2] unrotated, probe_idx list)."""
    radii = [cone_cdf_inv((j + 0.5) / n) for j in range(n)]
    pts = []
    if design == "ring4":
        # research: 4-probe ring at top strata, clamped >= rho
        ring = list(range(n - 4, n))
        probes = ring
        for j in range(n):
            if j in ring:
                k = j - (n - 4)
                r = max(radii[j], rho)
                a = k * math.pi / 2
            else:
                r = radii[j]
                a = GOLDEN * j
            pts.append((r * math.cos(a), r * math.sin(a)))
    elif design == "centre_ring4":
        radii[0] = 0.0
        ring = list(range(n - 4, n))
        probes = [0] + ring
        for j in range(n):
            if j in ring:
                k = j - (n - 4)
                r = max(radii[j], rho)
                a = k * math.pi / 2
            else:
                r = radii[j]
                a = GOLDEN * j
            pts.append((r * math.cos(a), r * math.sin(a)))
    elif design == "centre_ring4_inner4":
        radii[0] = 0.0
        outer = list(range(n - 4, n))
        mid = n // 2
        inner = list(range(mid - 2, mid + 2))
        probes = [0] + inner + outer
        for j in range(n):
            if j in outer:
                k = j - (n - 4); r = max(radii[j], rho); a = k * math.pi / 2
            elif j in inner:
                k = j - (mid - 2); r = min(max(radii[j], 0.35), 0.45); a = k * math.pi / 2 + math.pi / 4
            else:
                r = radii[j]; a = GOLDEN * j
            pts.append((r * math.cos(a), r * math.sin(a)))
    elif design.startswith("centre_ring"):
        k_ring = int(design[len("centre_ring"):])
        radii[0] = 0.0
        ring = list(range(n - k_ring, n))
        probes = [0] + ring
        for j in range(n):
            if j in ring:
                k = j - (n - k_ring)
                r = max(radii[j], rho)
                a = k * 2 * math.pi / k_ring
            else:
                r = radii[j]
                a = GOLDEN * j
            pts.append((r * math.cos(a), r * math.sin(a)))
    else:
        raise ValueError(design)
    return np.array(pts), probes

def bake(grid_n, occluded, n, design, rho, rng, rotate=True):
    """occluded(px, py) -> bool array. Returns stored vis grid and centre-blocked grid."""
    base, probes = layout(n, design, rho)
    full = len(base)
    stored = np.zeros((grid_n, grid_n))
    centre_blocked = np.zeros((grid_n, grid_n), dtype=bool)
    for iy in range(grid_n):
        for ix in range(grid_n):
            th = rng.uniform(0, 2 * math.pi) if rotate else 0.0
            c, s = math.cos(th), math.sin(th)
            off = base @ np.array([[c, s], [-s, c]])
            px = ix + off[:, 0]
            py = iy + off[:, 1]
            occ = occluded(px, py)
            centre_blocked[iy, ix] = occluded(np.array([ix]), np.array([iy]))[0]
            pv = ~occ[probes]
            if pv.all() or (~pv).all():
                stored[iy, ix] = 1.0 if pv.all() else 0.0
            else:
                stored[iy, ix] = (~occ).sum() / full
    return stored, centre_blocked

def bilinear(grid, x, y):
    x0, y0 = int(math.floor(x)), int(math.floor(y))
    fx, fy = x - x0, y - y0
    g = grid
    return (g[y0, x0] * (1 - fx) * (1 - fy) + g[y0, x0 + 1] * fx * (1 - fy)
            + g[y0 + 1, x0] * (1 - fx) * fy + g[y0 + 1, x0 + 1] * fx * fy)

def edge_trial(n, design, rho, rng, grid_n=14):
    worst = 0.0; snaps = 0
    for adeg in np.linspace(0, 45, 10):
        a = math.radians(adeg)
        nx, ny = math.cos(a), math.sin(a)  # edge normal
        for phase in np.linspace(0, 1, 9, endpoint=False):
            off = grid_n / 2 + phase
            occluded = lambda px, py: (px * nx + py * ny - off) < 0
            stored, _ = bake(grid_n, occluded, n, design, rho, rng)
            ys, xs = np.mgrid[0:grid_n, 0:grid_n]
            d = xs * nx + ys * ny - off  # signed distance
            exact = esf(d)  # fraction of kernel on lit side (d>0 lit)
            inner = slice(2, grid_n - 2)
            st = stored[inner, inner]; ex = exact[inner, inner]
            dx = np.abs(np.diff(st, axis=1)).max(); dy = np.abs(np.diff(st, axis=0)).max()
            worst = max(worst, dx, dy)
            snaps += int((((ex > 0.1) & (ex < 0.9)) & ((st == 0) | (st == 1))).sum())
    return worst, snaps

def strip_trial(n, design, rho, width, rng, grid_n=16, point=False):
    h = width / 2
    centre_viol = 0; gap_pts = 0; total_pts = 0; worst = 0.0
    for adeg in np.linspace(0, 45, 10):
        a = math.radians(adeg)
        nx, ny = math.cos(a), math.sin(a)
        for phase in np.linspace(0, 1, 9, endpoint=False):
            off = grid_n / 2 + phase
            occluded = lambda px, py: np.abs(px * nx + py * ny - off) < h
            if point:
                ys, xs = np.mgrid[0:grid_n, 0:grid_n]
                stored = np.where(occluded(xs.astype(float), ys.astype(float)), 0.0, 1.0)
                cb = stored == 0
            else:
                stored, cb = bake(grid_n, occluded, n, design, rho, rng)
            centre_viol += int((cb & (stored >= 1.0)).sum())
            inner = slice(2, grid_n - 2)
            st = stored[inner, inner]
            worst = max(worst, np.abs(np.diff(st, axis=1)).max(), np.abs(np.diff(st, axis=0)).max())
            # walk along the strip centreline inside the inner region
            tx, ty = -ny, nx
            px0, py0 = off * nx, off * ny
            for t in np.linspace(-grid_n, grid_n, 800):
                x, y = px0 + tx * t, py0 + ty * t
                if 3 <= x <= grid_n - 4 and 3 <= y <= grid_n - 4:
                    total_pts += 1
                    if bilinear(stored, x, y) >= 1.0:
                        gap_pts += 1
    return centre_viol, gap_pts, total_pts, worst

def main2():
    rng = np.random.default_rng(11)
    for design, rho in [("centre_ring4", 0.75), ("centre_ring6", 0.65), ("centre_ring8", 0.6), ("centre_ring8", 0.7)]:
        for n in (12, 32):
            if n < 1 + int(design[11:]):
                continue
            w_e, snaps = edge_trial(n, design, rho, rng)
            out = [f"{design:14s} rho={rho} n={n:2d} edge_worst={w_e:.3f} snaps={snaps}"]
            for width in (0.45, 0.4, 0.3):
                cv, gp, tp, ws = strip_trial(n, design, rho, width, rng)
                out.append(f"w{width}: cv={cv} gaps={gp}/{tp}")
            print("  ".join(out), flush=True)

if __name__ == "__main__" and len(sys.argv) > 1 and sys.argv[1] == "v2":
    main2(); sys.exit(0)

if __name__ == "__main__":
    rng = np.random.default_rng(7)
    designs = sys.argv[1:] or ["ring4", "centre_ring4", "centre_ring4_inner4"]
    print("ESF 10% point:", float(np.interp(0.1, _cum, _xs)), "90%:", float(np.interp(0.9, _cum, _xs)))
    cv, gp, tp, w = strip_trial(32, "ring4", 0.75, 0.4, rng, point=True)
    print(f"point-sampled strip w=0.4: centre_viol={cv} gaps={gp}/{tp}")
    for design in designs:
        for rho in (0.75,):
            for n in (5, 6, 8, 16, 32, 64):
                if design == "centre_ring4_inner4" and n < 9:
                    continue
                if design == "ring4" and n < 4:
                    continue
                w_e, snaps = edge_trial(n, design, rho, rng)
                out = [f"{design:20s} rho={rho} n={n:2d} edge_worst_adj={w_e:.3f} snaps={snaps}"]
                for width in (0.4, 0.2):
                    cv, gp, tp, ws = strip_trial(n, design, rho, width, rng)
                    out.append(f"strip{width}: cviol={cv} gaps={gp}/{tp} worst={ws:.3f}")
                print("  ".join(out), flush=True)
