# View-faithful: per receiver on the surface, the shader's probe weights under each depth rule
# vs the exact (dense stored) tile. Reports TV distance of normalised weights and far-side share.
import sys; sys.path.insert(0, sys.argv[1])
from prl57 import *
M = sys.argv[2]; U = 39.37
def lerp(a, b, s): return a + s*(b-a)
def ms(t): mu = t[:, 0]; return mu, np.maximum(t[:, 1]-mu*mu, 0.0)
def slide(a, b, s, k=0.0):
    ma, va = ms(a); mb, vb = ms(b); mu = lerp(ma, mb, s); v = lerp(va, vb, s) + k*s*(1-s)*(mb-ma)**2
    return np.stack([mu, v+mu*mu], 1)
def compose(rule, marks, tl, b, knots, t, step):
    n = len(knots)
    if t <= 0: return tl[0] if marks[0] == 1 else b
    if t >= 1: return tl[n-1] if marks[n-1] == 1 else b
    defined = lambda i: i in (0, n-1) or marks[i] != 0
    at = max(i for i in range(n) if knots[i] <= t + 1e-6)
    lo = at
    while not defined(lo): lo -= 1
    st = lambda i: tl[i] if marks[i] == 1 else b
    if abs(knots[lo]-t) < 1e-5: return st(lo)
    hi = at+1
    while not defined(hi): hi += 1
    w = (t-knots[lo])/(knots[hi]-knots[lo]); a, bb = st(lo), st(hi)
    cur = lerp(a, b, 2*w) if w <= .5 else lerp(b, bb, 2*w-1)
    if rule == 'cur' or (lo == 0 and hi == n-1): return cur
    if rule == '1a': return slide(a, bb, w)
    if rule == 'guard':
        ma, _ = ms(a); mb, _ = ms(bb)
        return slide(a, bb, w, (np.abs(mb-ma) > 2.0*step*(hi-lo)).astype(float))
    if rule == 'hold': return a if w < .5 else bb
def oct(d):
    p = d[:2]/max(abs(d).sum(), 1e-6)
    if d[2] < 0:
        p = np.array([(1-abs(p[1]))*(1 if p[0] >= 0 else -1), (1-abs(p[0]))*(1 if p[1] >= 0 else -1)])
    return p*0.5+0.5
def tap(tile, to_sample):
    uv = 1 + oct(to_sample)*8 - 0.5
    i = np.floor(uv).astype(int); f = uv - i
    T = tile.reshape(10, 10, 2)
    g = lambda r, c: T[min(max(r, 0), 9), min(max(c, 0), 9)]
    return (g(i[1], i[0])*(1-f[0]) + g(i[1], i[0]+1)*f[0])*(1-f[1]) + (g(i[1]+1, i[0])*(1-f[0]) + g(i[1]+1, i[0]+1)*f[0])*f[1]
def cheb(m, dist):
    var = max(m[1]-m[0]*m[0], 1e-4); delta = max(dist-m[0]-0.05, 0.0)
    return min(max(var/(var+delta*delta) if delta > 0 else 1.0, 0.03), 1.0)
def run(mapname, mid, travel, surfaces, rules=('cur', '1a', 'guard', 'hold')):
    g, t59, (hp, hk, hrec) = load(f'{M}/head-{mapname}.prl')
    _, _, (dp, dk, drec) = load(f'{M}/dense-{mapname}.prl')
    o, c, d, valid, bm, bq = g; hk = hk[mid]; dk = dk[mid]; step = travel/U/(len(hk)-1); dstep = travel/U/(len(dk)-1)
    def base(p):
        return t59[p] if p in t59 else np.tile(np.array([[bm[p], bq[p]]]), (100, 1))
    for sname, (pts_fn, nq) in surfaces.items():
        ne = np.array([-nq[1], nq[2], -nq[0]], float)
        acc = {r: [] for r in rules}; far = {r: [] for r in rules + ('truth',)}; per_mid = {}
        for k in range(len(hk)-1):
            tm = 0.5*(hk[k]+hk[k+1]); tvs = {r: [] for r in rules}
            for q in pts_fn(tm):
                pe = np.array([-q[1], q[2], -q[0]])/U
                gp = pe - np.array(o); gi = np.floor(gp).astype(int); fr = gp - gi
                ws = {r: [] for r in rules + ('truth',)}; isfar = []
                for cz in (0, 1):
                    for cy in (0, 1):
                        for cx in (0, 1):
                            idx = gi + np.array([cx, cy, cz])
                            if (idx < 0).any() or (idx >= np.array(d)).any(): continue
                            p = int(idx[0] + d[0]*(idx[1] + d[1]*idx[2]))
                            if valid[p] == 0: continue
                            off = np.array([cx, cy, cz])
                            tri = np.prod(np.where(off > 0, fr, 1-fr))
                            dtp = off - fr; dist = np.linalg.norm(dtp)
                            cosv = float(np.clip(np.dot(dtp/dist, ne), -1, 1)) if dist > 1e-6 else 1.0
                            bf = ((cosv+1)/2)**8 + 0.03
                            b = base(p); to_s = -dtp
                            hr = hrec.get((mid, p)); dr = drec.get((mid, p))
                            tt = compose('cur', dr[0], dr[1], b, dk, tm, dstep) if dr else b
                            ws['truth'].append(tri*bf*cheb(tap(tt, to_s), dist))
                            for r in rules:
                                tc = compose(r, hr[0], hr[1], b, hk, tm, step) if hr else b
                                ws[r].append(tri*bf*cheb(tap(tc, to_s), dist))
                            isfar.append(cosv < 0)
                if not isfar or sum(ws['truth']) <= 0: continue
                isfar = np.array(isfar); wt = np.array(ws['truth']); wt /= wt.sum(); far['truth'].append(wt[isfar].sum())
                for r in rules:
                    w = np.array(ws[r]); w /= w.sum()
                    tv = 0.5*np.abs(w-wt).sum(); acc[r].append(tv); tvs[r].append(tv); far[r].append(w[isfar].sum())
            per_mid[round(tm, 3)] = {r: round(float(np.mean(tvs[r])), 3) for r in rules}
        print(f'== {mapname} {sname}: receivers {len(far["truth"])}')
        print('   far-side (behind-surface) weight share: truth %.3f' % np.mean(far['truth']), ' '.join(f'{r} {np.mean(far[r]):.3f}' for r in rules))
        print('   weight-vector TV distance from truth: ', ' '.join(f'{r} mean {np.mean(acc[r]):.3f} p90 {np.percentile(acc[r], 90):.3f}' for r in rules))
        print('   per midpoint TV:', per_mid)
def gridpts(x0, x1, y0, y1, n=10):
    return [(x, y) for x in np.linspace(x0, x1, n) for y in np.linspace(y0, y1, n)]
run('mover-setpiece', 1, 176, {
    'ceiling': (lambda t: [(x, y, -16+176*t-0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, -1)),
    'floor': (lambda t: [(x, y, -176+176*t+0.5) for x, y in gridpts(860, 1124, 188, 452)], (0, 0, 1))})
run('enemy-movers', 1, 256, {
    'east face': (lambda t: [(1312+256*t+0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (1, 0, 0)),
    'west face': (lambda t: [(1280+256*t-0.5, y, z) for y, z in gridpts(72, 440, 8, 88)], (-1, 0, 0))})
