# Enclosure runs per (mover, probe) from geometry, by the bake's enclosing rule (inside one posed
# brush by enclosure_inset on every plane), with edge copy's value sources (edge_copy::value_sources),
# checked against id 57's stored bounce and depth marks.
#   python m9runs.py <sweeper|bigroom> <prl> [n-probes-to-print]
import itertools, re, sys
import numpy as np
import m9id57

U = 0.0254
REPO_MAPS = r'C:\Users\danhi\Projects\Personal\postretro\content\dev\maps'
MOVERS = {
    'sweeper': dict(map='enemy-movers.map', name='em_sweeper', id=1, travel=(256.0, 0.0, 0.0)),
    'bigroom': dict(map='mover-setpiece.map', name='setpiece_room_big', id=None, travel=(0.0, 0.0, 272.0)),
}


def brushes(mapfile, name):
    """Convex brushes of the named entity: list of (label, [(n, d)]) with outward unit normals, map units."""
    src = open(mapfile, encoding='utf-8').read().splitlines()
    num = r'(-?[\d.]+)'
    pl = re.compile(r'\(\s*' + r'\s+'.join([num] * 3) + r'\s*\)\s*\(\s*' + r'\s+'.join([num] * 3) +
                    r'\s*\)\s*\(\s*' + r'\s+'.join([num] * 3) + r'\s*\)')
    depth = 0; ents = []; cur = None; label = ''; last = ''
    for line in src:
        s = line.strip()
        if s.startswith('//'):
            last = s; continue
        if s == '{':
            depth += 1
            if depth == 1: ents.append(dict(keys={}, brushes=[]))
            if depth == 2: cur = []; label = last
            continue
        if s == '}':
            if depth == 2: ents[-1]['brushes'].append((label, cur))
            depth -= 1; continue
        if depth == 1:
            m = re.match(r'"([^"]+)"\s+"([^"]*)"', s)
            if m: ents[-1]['keys'][m.group(1)] = m.group(2)
        elif depth == 2:
            m = pl.search(s)
            if m: cur.append(np.array([float(v) for v in m.groups()]).reshape(3, 3))
    ent = next(e for e in ents if e['keys'].get('name') == name)
    out = []
    for label, pts in ent['brushes']:
        planes = []
        for p in pts:
            n = np.cross(p[1] - p[0], p[2] - p[0]); n /= np.linalg.norm(n)
            planes.append([n, float(n @ p[0])])
        verts = []
        for a, b, c in itertools.combinations(range(len(planes)), 3):
            A = np.array([planes[a][0], planes[b][0], planes[c][0]])
            if abs(np.linalg.det(A)) < 1e-9: continue
            v = np.linalg.solve(A, [planes[a][1], planes[b][1], planes[c][1]])
            verts.append(v)
        # keep vertices on the brush (inside every plane, either orientation resolved by majority)
        cen = np.mean(pts, axis=(0, 1))
        for q in planes:
            if q[0] @ cen - q[1] > 0: q[0], q[1] = -q[0], -q[1]
        vs = [v for v in verts if all(q[0] @ v - q[1] <= 1e-6 for q in planes)]
        cen = np.mean(vs, axis=0)
        for q in planes:
            if q[0] @ cen - q[1] > 0: q[0], q[1] = -q[0], -q[1]
        out.append((label, [(q[0], q[1]) for q in planes]))
    return out


def inset_u(p_map):
    e = np.array([-p_map[1], p_map[2], -p_map[0]]) * U
    m = 1e-3 + np.sqrt(3) * (np.abs(e).max() + 1e-3) * 2.0 ** -23
    return m / U


def enclosed(p, brs, off):
    ins = inset_u(p)
    for _, planes in brs:
        if all(n @ (p - off) - d <= -ins for n, d in planes):
            return True
    return False


def run_list(enc):
    runs = []; k = 0; n = len(enc)
    while k < n:
        if not enc[k]:
            k += 1; continue
        a = k
        while k < n and enc[k]: k += 1
        runs.append((a, k - 1))
    return runs


def sources(enc, ts):
    n = len(enc); src = list(range(n))
    for a, b in run_list(enc):
        if a == b:
            if a not in (0, n - 1):
                src[a] = a - 1 if (ts[a] - ts[a - 1]) < (ts[a + 1] - ts[a]) - 1e-5 else a + 1
            continue
        if a != 0: src[a] = a - 1
        if b != n - 1: src[b] = b + 1
    return src


def mover_id(ch, which):
    if MOVERS[which]['id'] is not None:
        return MOVERS[which]['id']
    # the mover with the most knots among those whose travel matches is resolved by the caller
    raise KeyError


def report(prl, which, mid=None):
    M = MOVERS[which]
    brs = brushes(REPO_MAPS + '/' + M['map'], M['name'])
    ch = m9id57.load(prl)
    mid = M['id'] if mid is None else mid
    ts = ch['movers'][mid]['knots']
    travel = np.array(M['travel'])
    keys = sorted({pi for (m, pi) in list(ch['bounce']) + list(ch['depth']) if m == mid})
    rows = []
    for pi in keys:
        p = ch['pos'](pi)
        enc = [enclosed(p, brs, travel * t) for t in ts]
        if not any(enc):
            continue
        rows.append(dict(pi=pi, p=p, enc=enc, runs=run_list(enc), src=sources(enc, ts),
                         b=ch['bounce'].get((mid, pi)), d=ch['depth'].get((mid, pi))))
    return ts, rows, ch, brs


def check(ts, rows):
    """Geometry vs id 57: every enclosed knot whose depth is stored must be the dark tile (mean 0),
    every clear stored depth must be non-zero."""
    bad = 0; n = 0
    for r in rows:
        if not r['d']: continue
        for k, (mk, v) in enumerate(r['d']):
            if mk != 'S': continue
            n += 1
            if r['enc'][k] != (v == 0.0): bad += 1
    return n, bad


if __name__ == '__main__':
    which, prl = sys.argv[1], sys.argv[2]
    mid = int(sys.argv[4]) if len(sys.argv) > 4 else None
    ts, rows, ch, brs = report(prl, which, mid)
    hist = {}
    for r in rows:
        for a, b in r['runs']:
            hist[b - a + 1] = hist.get(b - a + 1, 0) + 1
    n, bad = check(ts, rows)
    print('knots', len(ts), 'enclosed probes', len(rows), 'run-length histogram', dict(sorted(hist.items())),
          f'| depth check: {n} stored tiles, {bad} disagree with geometry')
    for r in rows[:int(sys.argv[3]) if len(sys.argv) > 3 else 0]:
        p = r['p']
        print(f"probe {r['pi']} map ({p[0]:.1f},{p[1]:.1f},{p[2]:.1f}) runs",
              [(round(ts[a], 4), round(ts[b], 4), b - a + 1) for a, b in r['runs']])
