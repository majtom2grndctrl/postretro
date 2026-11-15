# Round 4: value (direct q x own tile + bounce) error per receiver vs the fine bake, depth weights from truth.
import sys, json; sys.path.insert(0, sys.argv[1])
from prl57 import *
from val57 import loadv
A = sys.argv[1]; M = sys.argv[2]; F = A + '/ws/content/dev/maps'; U = 39.37
exec("def lerp" + open(A+'/view.py').read().split("def lerp")[1].split("def run(")[0])
exec("def dark" + open(A+'/view3.py').read().split("def dark")[1].split("def run(")[0])
LUM = np.array([0.2126, 0.7152, 0.0722])
def itap(tile, n):       # 6x6 bordered irradiance tile (36,3) toward engine normal n -> luminance
    uv = 1 + oct(n)*4 - 0.5; i = np.floor(uv).astype(int); f = uv - i; T = tile.reshape(6, 6, 3)
    g = lambda r, c: T[min(max(r, 0), 5), min(max(c, 0), 5)]
    v = (g(i[1], i[0])*(1-f[0]) + g(i[1], i[0]+1)*f[0])*(1-f[1]) + (g(i[1]+1, i[0])*(1-f[0]) + g(i[1]+1, i[0]+1)*f[0])*f[1]
    return float(v @ LUM)
def bracket(knots, t, defined):
    n = len(knots)
    if t <= 0: return 0, 0, 0.0
    if t >= 1: return n-1, n-1, 0.0
    at = max(i for i in range(n) if knots[i] <= t + 1e-6); lo = at
    while not (lo == 0 or defined(lo)): lo -= 1
    if abs(knots[lo]-t) < 1e-5: return lo, lo, 0.0
    hi = at+1
    while not (hi == n-1 or defined(hi)): hi += 1
    return lo, hi, (t-knots[lo])/(knots[hi]-knots[lo])
def qval(q, knots, t):
    lo, hi, w = bracket(knots, t, lambda i: not np.isnan(q[i]))
    a = q[lo] if not np.isnan(q[lo]) else 1.0; b = q[hi] if not np.isnan(q[hi]) else 1.0
    return a + w*(b-a)
def bval(marks, tl, knots, t, n):   # -> (known luminance part, base coefficient)
    lo, hi, w = bracket(knots, t, lambda i: marks[i] != 0)
    k = 0.0; bc = 0.0
    for i, c in ((lo, 1-w), (hi, w)):
        if c == 0: continue
        if marks[i] == 1: k += c*itap(tl[i], n)
        else: bc += c
    return k, bc
def run(mapname, mid, travel, surfaces, calib=None):
    g, t59, (_, hk, hrec) = load(f'{M}/head-{mapname}.prl'); _, _, (_, xk, xrec) = load(f'{F}/xfine-{mapname}.prl')
    _, (_, _, hD, hB, _), _ = loadv(f'{M}/head-{mapname}.prl'); _, (_, dkn, dD, dB, _), _ = loadv(f'{M}/dense-{mapname}.prl'); _, (_, _, xD, xB, _), _ = loadv(f'{F}/xfine-{mapname}.prl')
    o, c, d, valid, bm, bq = g; hk, xk, dk = hk[mid], xk[mid], dkn[mid]; nx = len(xk); hD, hB, dD, dB, xD, xB = hD[mid], hB[mid], dD[mid], dB[mid], xD[mid], xB[mid]
    idx_of = lambda t: min(range(nx), key=lambda j: abs(xk[j]-t)); hidx = [idx_of(t) for t in hk]
    ivs = []
    for (m, p), (marks, tl) in xrec.items():
        if m != mid: continue
        r_ = None
        for j in range(nx+1):
            dk_ = j < nx and marks[j] == 1 and dark(tl[j])
            if dk_ and r_ is None: r_ = j
            if not dk_ and r_ is not None: ivs.append((r_, j-1)); r_ = None
    picks = stab([(a-1, a-1) for a, b in ivs if a >= 1] + [(b+1, b+1) for a, b in ivs if b+1 < nx], hidx)
    union = sorted(set(hidx) | set(picks)); uk = [xk[j] for j in union]
    # (ii): head states at head knots, fine-bake states at added knots
    def merge_q(hq, xq):
        return np.array([(hq[hidx.index(j)] if hq is not None else 1.0) if j in hidx else (xq[j] if xq is not None else 1.0) for j in union])
    pD = {}
    for p in set(hD) | set(xD):
        hl = {(ns, l): (q, t) for ns, l, q, t in hD.get(p, [])}; xl = {(ns, l): (q, t) for ns, l, q, t in xD.get(p, [])}
        pD[p] = [(k[0], k[1], merge_q(hl.get(k, (None,))[0], xl.get(k, (None,))[0]), (xl.get(k) or hl.get(k))[1]) for k in set(hl) | set(xl)]
    pB = {}
    for p in set(hB) | set(xB):
        hm, ht = hB.get(p, ([0]*len(hk), {})); xm, xt = xB.get(p, ([0]*nx, {})); marks = []; tl = {}
        for u, j in enumerate(union):
            if j in hidx:
                k = hidx.index(j); marks.append(hm[k])
                if hm[k] == 1: tl[u] = ht[k]
            else:
                marks.append(xm[j])
                if xm[j] == 1: tl[u] = xt[j]
        pB[p] = (marks, tl)
    cands = {'(i) head': (hD, hB, hk), f'(ii) head+OUTSIDE-ends [{len(union)} knots]': (pD, pB, uk), f'(iii) dense [{len(dk)}]': (dD, dB, dk)}
    base = lambda p: t59[p] if p in t59 else np.tile(np.array([[bm[p], bq[p]]]), (100, 1))
    out = {}
    for sname, (pts_fn, nq) in surfaces.items():
        ne = np.array([-nq[1], nq[2], -nq[0]], float)
        err = {cn: [[] for _ in range(nx)] for cn in cands}; ref = [[] for _ in range(nx)]; unk = {cn: 0 for cn in cands}; tot = 0; noT = 0
        for j in range(nx):
            tm = xk[j]; pc = {}
            def pv(p):      # per probe: truth and candidate value luminance toward the normal (direct loss + bounce), None if base-dependent
                if p in pc: return pc[p]
                res = {}
                xl = {(ns, l): (q, t) for ns, l, q, t in xD.get(p, [])}
                dirs = {}
                for cn, (D, B, kn) in cands.items():
                    tot_d = 0.0
                    for ns, l, q, t in D.get(p, []):
                        tt = t if t is not None else None
                        if tt is None: continue
                        xq = xl.get((ns, l)); qt = qval(xq[0], xk, tm) if xq else 1.0
                        tot_d += (qval(q, kn, tm) - qt) * itap(tt, ne)
                    for (ns, l), (xq, t) in xl.items():     # lights the candidate does not gate here
                        if t is not None and not any(a == ns and b == l for a, b, _, _ in D.get(p, [])): tot_d += (1.0 - qval(xq, xk, tm)) * itap(t, ne)
                    kt, bt = bval(*xB[p], xk, tm, ne) if p in xB else (0.0, 1.0)
                    kc, bc = bval(*B[p], kn, tm, ne) if p in B else (0.0, 1.0)
                    res[cn] = (tot_d + kc - kt, bc - bt)
                gated = sum((1.0) * itap(t, ne) for (ns, l), (xq, t) in xl.items() if t is not None)
                pc[p] = (res, gated, (kt if bt < 0.02 else None)); return pc[p]
            for q in pts_fn(tm):
                pe = np.array([-q[1], q[2], -q[0]])/U; gp = pe - np.array(o); gi = np.floor(gp).astype(int); fr = gp - gi
                ws = []; ps = []
                for cz in (0, 1):
                    for cy in (0, 1):
                        for cx in (0, 1):
                            idx = gi + np.array([cx, cy, cz])
                            if (idx < 0).any() or (idx >= np.array(d)).any(): continue
                            p = int(idx[0] + d[0]*(idx[1] + d[1]*idx[2]))
                            if valid[p] == 0: continue
                            off = np.array([cx, cy, cz]); tri = np.prod(np.where(off > 0, fr, 1-fr)); dtp = off - fr; dist = np.linalg.norm(dtp)
                            cosv = float(np.clip(np.dot(dtp/dist, ne), -1, 1)) if dist > 1e-6 else 1.0
                            r = xrec.get((mid, p)); tt = compose('cur', r[0], r[1], base(p), xk, tm, 1) if r else base(p)
                            ws.append(tri*(((cosv+1)/2)**8 + 0.03)*cheb(tap(tt, -dtp), dist)); ps.append(p)
                if not ws or sum(ws) <= 0: continue
                w = np.array(ws)/sum(ws); tot += 1
                for cn in cands:
                    e = 0.0; bdep = 0.0
                    for wi, p in zip(w, ps):
                        (dv, db) = pv(p)[0][cn]; e += wi*dv; bdep += wi*abs(db)
                    if bdep > 0.05: unk[cn] += 1
                    err[cn][j].append(e)
        out[sname] = {cn: [float(np.mean(x)) if x else 0.0 for x in err[cn]] for cn in cands}
        print(f'-- {mapname} {sname}: linear-irradiance value error vs truth (signed mean over receivers per time); base-dependent receiver-times excluded-from-trust: ' + ', '.join(f'{cn}: {100*unk[cn]/max(tot,1):.0f}%' for cn in cands))
        for cn in cands:
            pt = np.array(out[sname][cn]); allr = np.concatenate([np.array(x) for x in err[cn] if x])
            print(f'   {cn:38s} mean|err| {np.abs(allr).mean():.4f}  worst time-mean {np.abs(pt).max():.4f} (t={xk[int(np.abs(pt).argmax())]:.3f})  worst receiver {np.abs(allr).max():.4f}')
            print('      per time x1000:', ' '.join(f'{int(round(1000*v))}' for v in pt))
    return out
def gridpts(x0, x1, y0, y1, n=6):
    return [(x, y) for x in np.linspace(x0, x1, n) for y in np.linspace(y0, y1, n)]
which = sys.argv[3]
if which == 'sweeper':
    run('enemy-movers', 1, 256, {
        'east face': (lambda t: [(1312+256*t+0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (1, 0, 0)),
        'west face': (lambda t: [(1280+256*t-0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (-1, 0, 0))})
else:
    run('mover-setpiece', 1, 176, {
        'ceiling': (lambda t: [(x, y, -16+176*t-0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, -1)),
        'floor': (lambda t: [(x, y, -176+176*t+0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, 1))})
