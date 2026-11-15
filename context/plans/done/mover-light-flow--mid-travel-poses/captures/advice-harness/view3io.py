# Round 3: candidates vs a fine-pose truth bake, at every fine knot time.
import sys; sys.path.insert(0, sys.argv[1])
from prl57 import *
A = sys.argv[1]; M = sys.argv[2]; F = A + '/ws/content/dev/maps'; U = 39.37
src = open(A+'/view.py').read()
exec("def lerp" + src.split("def lerp")[1].split("def run(")[0])   # lerp, ms, slide, compose, oct, tap, cheb
def dark(t): return t[:, 0].max() < 0.02
def stab(intervals, have):
    todo = sorted([iv for iv in intervals if not any(iv[0] <= h <= iv[1] for h in have)], key=lambda iv: iv[1])
    picks = []
    while todo:
        e = todo[0][1]; grp = [iv for iv in todo if iv[0] <= e]
        lo = max(iv[0] for iv in grp); picks.append((lo + e)//2)      # centre in the common intersection
        todo = [iv for iv in todo if iv[0] > e]
    return picks
def run(mapname, mid, travel, surfaces, fine='xfine'):
    g, t59, (_, hk, hrec) = load(f'{M}/head-{mapname}.prl')
    _, _, (_, dk, drec) = load(f'{M}/dense-{mapname}.prl')
    _, _, (_, fk, frec) = load(f'{F}/fine-{mapname}.prl')
    _, _, (_, xk, xrec) = load(f'{F}/{fine}-{mapname}.prl')
    o, c, d, valid, bm, bq = g; hk, dk, fk, xk = hk[mid], dk[mid], fk[mid], xk[mid]; nx = len(xk)
    idx_of = lambda t: min(range(nx), key=lambda j: abs(xk[j]-t))
    hidx = [idx_of(t) for t in hk]; assert all(abs(xk[j]-t) < 1e-4 for j, t in zip(hidx, hk))
    # enclosure intervals (fine-knot indices) per probe, from the truth bake's dark tiles
    ivs = []; never = 0
    for (m, p), (marks, tl) in xrec.items():
        if m != mid: continue
        run_ = None
        for j in range(nx + 1):
            dk_ = j < nx and marks[j] == 1 and dark(tl[j])
            if dk_ and run_ is None: run_ = j
            if not dk_ and run_ is not None: ivs.append((run_, j-1)); run_ = None
    picks = stab([(a-1, a-1) for a, b in ivs if a >= 1] + [(b+1, b+1) for a, b in ivs if b+1 < nx] + [(a, a) for a, b in ivs] + [(b, b) for a, b in ivs], hidx)
    union = sorted(set(hidx) | set(picks)); uk = [xk[j] for j in union]
    lens = [(b-a+1) for a, b in ivs]
    print(f'== {mapname}: head knots {len(hk)}, enclosure intervals {len(ivs)} (len in fine steps of {travel/(nx-1):.1f}u: min {min(lens)} median {int(np.median(lens))} max {max(lens)}), '
          f'already stabbed by head knots {sum(1 for iv in ivs if any(iv[0] <= h <= iv[1] for h in hidx))}, depth knots added {len(picks)} at t {[round(xk[j], 3) for j in sorted(picks)]} -> {len(union)} knots')
    # proposal records: head tiles at head knots, truth-bake depth at added knots
    prop = {}
    for p in {p for (m, p) in list(hrec) + list(xrec) if m == mid}:
        hr = hrec.get((mid, p)); xr = xrec.get((mid, p)); marks = []; tl = {}
        for u, j in enumerate(union):
            if j in hidx:
                k = hidx.index(j)
                if hr and hr[0][k] == 1: marks.append(1); tl[u] = hr[1][k]
                else: marks.append(hr[0][k] if hr else 0)
            else:
                if xr and xr[0][j] == 1: marks.append(1); tl[u] = xr[1][j]
                else: marks.append(xr[0][j] if xr else 0)
        if 1 in marks: prop[(mid, p)] = (marks, tl)
    # variant: added knots carry ONLY the dark tiles (no traced depth at other probes)
    propd = {}
    for key, (marks, tl) in prop.items():
        m2 = list(marks); t2 = dict(tl)
        for u, j in enumerate(union):
            if j not in hidx and m2[u] == 1 and not dark(tl[u]): m2[u] = 0; del t2[u]
            elif j not in hidx and m2[u] == 2: m2[u] = 0
        if 1 in m2: propd[key] = (m2, t2)
    cands = {'head-cur': (hrec, hk, 'cur'), 'head-1a': (hrec, hk, '1a'), f'dense{len(dk)}-1a': (drec, dk, '1a'), f'fine{len(fk)}-1a': (frec, fk, '1a'),
             f'proposal{len(union)}-1a': (prop, uk, '1a'), f'prop-darkonly{len(union)}-1a': (propd, uk, '1a'), f'proposal{len(union)}-cur': (prop, uk, 'cur')}
    base = lambda p: t59[p] if p in t59 else np.tile(np.array([[bm[p], bq[p]]]), (100, 1))
    for sname, (pts_fn, nq) in surfaces.items():
        ne = np.array([-nq[1], nq[2], -nq[0]], float)
        far = {cn: [[] for _ in range(nx)] for cn in list(cands) + ['truth']}; tv = {cn: [] for cn in cands}
        for j in range(nx):
            tm = xk[j]; cache = {}
            def tile(cn, p):
                if (cn, p) not in cache:
                    b = base(p)
                    if cn == 'truth':
                        r = xrec.get((mid, p)); cache[(cn, p)] = compose('cur', r[0], r[1], b, xk, tm, 1) if r else b
                    else:
                        recs, kn, rule = cands[cn]; r = recs.get((mid, p)); cache[(cn, p)] = compose(rule, r[0], r[1], b, kn, tm, 1) if r else b
                return cache[(cn, p)]
            for q in pts_fn(tm):
                pe = np.array([-q[1], q[2], -q[0]])/U; gp = pe - np.array(o); gi = np.floor(gp).astype(int); fr = gp - gi
                ws = {cn: [] for cn in far}; isfar = []
                for cz in (0, 1):
                    for cy in (0, 1):
                        for cx in (0, 1):
                            idx = gi + np.array([cx, cy, cz])
                            if (idx < 0).any() or (idx >= np.array(d)).any(): continue
                            p = int(idx[0] + d[0]*(idx[1] + d[1]*idx[2]))
                            if valid[p] == 0: continue
                            off = np.array([cx, cy, cz]); tri = np.prod(np.where(off > 0, fr, 1-fr)); dtp = off - fr; dist = np.linalg.norm(dtp)
                            cosv = float(np.clip(np.dot(dtp/dist, ne), -1, 1)) if dist > 1e-6 else 1.0
                            bf = ((cosv+1)/2)**8 + 0.03
                            for cn in far: ws[cn].append(tri*bf*cheb(tap(tile(cn, p), -dtp), dist))
                            isfar.append(cosv < 0)
                if not isfar or sum(ws['truth']) <= 0: continue
                isfar = np.array(isfar); wt = np.array(ws['truth']); wt /= wt.sum(); far['truth'][j].append(wt[isfar].sum())
                for cn in cands:
                    w = np.array(ws[cn]); w /= w.sum(); far[cn][j].append(w[isfar].sum()); tv[cn].append(0.5*np.abs(w-wt).sum())
        print(f'-- {sname}: far-side share over {nx} times (mean | worst time), TV from truth')
        for cn in far:
            pt = [np.mean(x) for x in far[cn] if x]
            print(f'   {cn:22s} {np.mean(pt):.3f} | {np.max(pt):.3f}' + (f'   TV {np.mean(tv[cn]):.3f}' if cn in tv else ''))
def gridpts(x0, x1, y0, y1, n=7):
    return [(x, y) for x in np.linspace(x0, x1, n) for y in np.linspace(y0, y1, n)]
which = sys.argv[3] if len(sys.argv) > 3 else 'both'
if which in ('room', 'both'):
    run('mover-setpiece', 1, 176, {
        'ceiling': (lambda t: [(x, y, -16+176*t-0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, -1)),
        'floor': (lambda t: [(x, y, -176+176*t+0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, 1))})
if which in ('sweeper', 'both'):
    run('enemy-movers', 1, 256, {
        'east face': (lambda t: [(1312+256*t+0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (1, 0, 0)),
        'west face': (lambda t: [(1280+256*t-0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (-1, 0, 0))})
