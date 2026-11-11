# Task 5b: a CPU model of the rising room interior capture from the probe dump
# (`diag_room_interior_probe_dump`). Casts the interior camera's rays at the
# room's inner box, samples the 8 probes around each hit as `sh_sample.wgsl`
# does (0.1-cell normal offset, trilinear x validity x smooth mover backface,
# no Chebyshev depth), and reports mean luma per t for the composed build and
# the static-brush reference, direct and bounce apart, plus the probes that
# move the frame most.
#
#   python3 -I cpu_interior.py <room.jsonl> [--top T1,T2]
import json, math, sys

W, H, STEP = 960, 540, 8
ALBEDO, INDIRECT = 0.5, 0.33
NORMALS = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]


def load(path):
    header, probes = None, []
    for line in open(path):
        row = json.loads(line)
        if row['kind'] == 'header':
            header = row
        else:
            probes.append(row)
    return header, probes


def ray(pose, px, py):
    (cx, cy, cz), yaw, pitch, fov = pose
    sy, cyw = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    sp, cp = math.sin(math.radians(pitch)), math.cos(math.radians(pitch))
    f = (cyw * cp, sy * cp, sp)
    r = (sy, -cyw, 0.0)
    u = (-cyw * sp, -sy * sp, cp)
    hx = math.tan(math.radians(fov) / 2.0)
    hy = hx * H / W
    nx = 2.0 * px / W - 1.0
    ny = 1.0 - 2.0 * py / H
    return tuple(f[i] + r[i] * nx * hx + u[i] * ny * hy for i in range(3))


def hits(t):
    """Interior box hit points and inward normals over the subsampled frame."""
    floor = -176.0 + 176.0 * t
    lo = (848.0, 176.0, floor)
    hi = (1136.0, 464.0, floor + 160.0)
    pose = ((860.0, 320.0, floor + 72.0), 0.0, -8.0, 100.0)
    o = pose[0]
    out = []
    for py in range(STEP // 2, H, STEP):
        for px in range(STEP // 2, W, STEP):
            d = ray(pose, px, py)
            best = None
            for a in range(3):
                if abs(d[a]) < 1e-12:
                    continue
                plane = hi[a] if d[a] > 0 else lo[a]
                s = (plane - o[a]) / d[a]
                if s > 0 and (best is None or s < best[0]):
                    n = [0.0, 0.0, 0.0]
                    n[a] = -1.0 if d[a] > 0 else 1.0
                    best = (s, tuple(n))
            s, n = best
            out.append((tuple(o[i] + s * d[i] for i in range(3)), n))
    return out


def srgb(x):
    x = min(max(x, 0.0), 1.0)
    return 12.92 * x if x <= 0.0031308 else 1.055 * x ** (1 / 2.4) - 0.055


def tonemap(c):
    peak = max(c)
    if peak <= 0.98:
        return c
    w = 0.02
    e = peak - 0.98
    k = 0.98 + w * e / (e + w)
    return [v * k / peak for v in c]


def luma(rgb):
    c = tonemap([ALBEDO * v for v in rgb])
    s = [255 * srgb(v) for v in c]
    return 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2]


class Grid:
    def __init__(self, probes):
        self.axes = [sorted({round(p['q'][a], 2) for p in probes}) for a in range(3)]
        self.by = {tuple(round(v, 2) for v in p['q']): p for p in probes}
        self.cell = [self.axes[a][1] - self.axes[a][0] for a in range(3)]

    def corners(self, s):
        idx = []
        for a in range(3):
            ax = self.axes[a]
            i = max(0, min(len(ax) - 2, int((s[a] - ax[0]) // self.cell[a])))
            idx.append(i)
        out = []
        for dx in (0, 1):
            for dy in (0, 1):
                for dz in (0, 1):
                    q = (self.axes[0][idx[0] + dx], self.axes[1][idx[1] + dy], self.axes[2][idx[2] + dz])
                    out.append(self.by.get(q))
        frac = [(s[a] - self.axes[a][idx[a]]) / self.cell[a] for a in range(3)]
        return out, frac


def weights(corners, frac, s, n, valid_of):
    ws = []
    k = 0
    for dx in (0, 1):
        for dy in (0, 1):
            for dz in (0, 1):
                p = corners[k]
                k += 1
                if p is None or not valid_of(p):
                    ws.append(0.0)
                    continue
                tri = (frac[0] if dx else 1 - frac[0]) * (frac[1] if dy else 1 - frac[1]) * (frac[2] if dz else 1 - frac[2])
                v = [p['q'][a] - s[a] for a in range(3)]
                l = math.sqrt(sum(x * x for x in v))
                c = 1.0 if l < 1e-6 else max(-1.0, min(1.0, sum(v[a] * n[a] for a in range(3)) / l))
                wrap = (c + 1) * 0.5
                ws.append(tri * (wrap ** 8 + 0.03))
    return ws


def valid_fn(which, ti):
    if which == 'ref':
        return lambda p: p['valid'] and p['at'][ti]['rvalid']
    return lambda p: p['valid']


def value(p, ti, which, ni):
    """(direct, bounce) RGB along normal ni: the composed build ('new'), the
    static reference ('ref'), termini only, today's compose ('today'), or the
    mover absent, as with id 57 stripped ('strip')."""
    if which == 'new':
        return p['at'][ti]['d'][ni], p['at'][ti]['b'][ni]
    if which == 'ref':
        return p['at'][ti]['rd'][ni], p['at'][ti]['rb'][ni]
    if which == 'today':
        return p['today'][ti]['d'][ni], p['today'][ti]['b'][ni]
    return p['free_d'][ni], p['base_b'][ni]


def frame(grid, probes, header, ti, which):
    """Mean luma (total, direct, bounce) and per-probe luma attribution."""
    t = header['ts'][ti]
    tot = [0.0, 0.0, 0.0]
    attrib = {}
    pts = hits(t)
    for (P, n) in pts:
        s = tuple(P[a] + n[a] * 0.1 * grid.cell[a] for a in range(3))
        corners, frac = grid.corners(s)
        valid_of = valid_fn(which, ti)
        ws = weights(corners, frac, s, n, valid_of)
        total = sum(ws)
        if total <= 0:
            continue
        ni = NORMALS.index(tuple(int(v) for v in n))
        d = [0.0] * 3
        b = [0.0] * 3
        for p, w in zip(corners, ws):
            if w == 0:
                continue
            dv, bv = value(p, ti, which, ni)
            for c in range(3):
                d[c] += w / total * dv[c]
                b[c] += w / total * bv[c]
            # Attribution: this probe's weighted share of the pixel's bounce
            # and direct, in linear Rec.709 luminance.
            a = attrib.setdefault(p['p'], [0.0, 0.0, 0.0])
            a[0] += w / total
            a[1] += w / total * (0.2126 * dv[0] + 0.7152 * dv[1] + 0.0722 * dv[2])
            a[2] += w / total * INDIRECT * (0.2126 * bv[0] + 0.7152 * bv[1] + 0.0722 * bv[2])
        tot[0] += luma([d[c] + INDIRECT * b[c] for c in range(3)])
        tot[1] += luma(d)
        tot[2] += luma([INDIRECT * v for v in b])
    n = len(pts)
    return [v / n for v in tot], {k: [x / n for x in v] for k, v in attrib.items()}


def main():
    header, probes = load(sys.argv[1])
    grid = Grid(probes)
    by_p = {p['p']: p for p in probes}
    rows = []
    print('t     | new total direct bounce | ref total direct bounce | today total | strip total')
    attribs = {}
    for ti, t in enumerate(header['ts']):
        new, an = frame(grid, probes, header, ti, 'new')
        ref, ar = frame(grid, probes, header, ti, 'ref')
        today = frame(grid, probes, header, ti, 'today')[0] if any('today' in p for p in probes) else [float('nan')] * 3
        strip = frame(grid, probes, header, ti, 'strip')[0] if any('free_d' in p for p in probes) else [float('nan')] * 3
        attribs[round(t, 3)] = (an, ar)
        rows.append({'t': t, 'new': new, 'ref': ref, 'today': today, 'strip': strip})
        print(f'{t:.2f}  | {new[0]:6.2f} {new[1]:6.2f} {new[2]:6.2f}   | {ref[0]:6.2f} {ref[1]:6.2f} {ref[2]:6.2f}   | {today[0]:6.2f}      | {strip[0]:6.2f}')
    out = sys.argv[1].replace('.jsonl', '-cpu.json')
    json.dump(rows, open(out, 'w'), indent=1)
    # The probes whose attributed light differs most between two t.
    if '--top' in sys.argv:
        ta, tb = [round(float(v), 3) for v in sys.argv[sys.argv.index('--top') + 1].split(',')]
        a, b = attribs[ta][0], attribs[tb][0]
        keys = set(a) | set(b)
        diff = sorted(keys, key=lambda k: -abs((b.get(k, [0, 0, 0])[1] + b.get(k, [0, 0, 0])[2]) - (a.get(k, [0, 0, 0])[1] + a.get(k, [0, 0, 0])[2])))
        print(f'\nprobes moving the frame most, t {ta} -> {tb} (weight share, direct lum, bounce lum x0.33):')
        for k in diff[:16]:
            p = by_p[k]
            print(f"  probe {k} q {p['q']} enc {''.join('E' if e else '.' for e in p['enc'])} marks {p.get('marks')} dom {p['domain']}"
                  f" | {ta}: {[round(x, 4) for x in a.get(k, [0, 0, 0])]} | {tb}: {[round(x, 4) for x in b.get(k, [0, 0, 0])]} q8 {p.get('q8')}")


if __name__ == '__main__' and '--vs-ref' not in sys.argv:
    main()


def vs_ref(path, t_req):
    """Per-probe attributed luminance, composed minus reference, at one t."""
    header, probes = load(path)
    grid = Grid(probes)
    by_p = {p['p']: p for p in probes}
    ti = min(range(len(header['ts'])), key=lambda i: abs(header['ts'][i] - t_req))
    _, an = frame(grid, probes, header, ti, 'new')
    _, ar = frame(grid, probes, header, ti, 'ref')
    keys = set(an) | set(ar)
    z = [0.0, 0.0, 0.0]
    rows = sorted(keys, key=lambda k: -((an.get(k, z)[1] + an.get(k, z)[2]) - (ar.get(k, z)[1] + ar.get(k, z)[2])))
    # Group by z layer and enclosure pattern.
    groups = {}
    for k in keys:
        p = by_p[k]
        key = (round(p['q'][2], 1), ''.join('E' if e else '.' for e in p['enc']), p.get('marks'))
        g = groups.setdefault(key, [0.0, 0.0, 0.0, 0.0, 0])
        a, r = an.get(k, z), ar.get(k, z)
        g[0] += a[1] - r[1]
        g[1] += a[2] - r[2]
        g[2] += a[0]
        g[3] += r[0]
        g[4] += 1
    print(f'\nt {header["ts"][ti]:.2f}: composed minus reference, attributed linear luminance, by (z, enclosure, marks):')
    print('  z       enc     marks   | d_direct  d_bounce | weight new  ref | probes')
    for key, g in sorted(groups.items(), key=lambda kv: -(abs(kv[1][0]) + abs(kv[1][1])))[:14]:
        print(f'  {key[0]:7.1f} {key[1]} {key[2]} | {g[0]:8.4f} {g[1]:8.4f} | {g[2]:.3f} {g[3]:.3f} | {g[4]}')


if __name__ == '__main__' and '--vs-ref' in sys.argv:
    for t in sys.argv[sys.argv.index('--vs-ref') + 1].split(','):
        vs_ref(sys.argv[1], float(t))
