import sys; sys.path.insert(0, sys.argv[1])
from prl57 import *
M = sys.argv[2]
U = 39.37
INT = np.array([r*10+c for r in range(1,9) for c in range(1,9)])
DS = np.array([0.5, 1.0, 1.5])
def W(t, d):  # t: (n,2) -> (n, len(d))
    mu = t[:, 0:1]; var = np.maximum(t[:, 1:2] - mu*mu, 1e-4)
    delta = np.maximum(d[None, :] - mu - 0.05, 0.0)
    return np.clip(np.where(delta > 0, var/(var+delta*delta), 1.0), 0.03, 1.0)
def lerp(a, b, s): return a + s*(b-a)
def ms(t):
    mu = t[:, 0]; return mu, np.maximum(t[:, 1]-mu*mu, 0.0)
def slide(a, b, s):
    ma, va = ms(a); mb, vb = ms(b)
    mu = lerp(ma, mb, s); v = lerp(va, vb, s)
    return np.stack([mu, v+mu*mu], 1)
def guard(a, b, s, lim, soft=False):
    ma, va = ms(a); mb, vb = ms(b)
    mu = lerp(ma, mb, s); v = lerp(va, vb, s)
    dm = np.abs(mb-ma)
    if soft: k = np.clip((dm-lim)/lim, 0, 1); k = k*k*(3-2*k)
    else: k = (dm > lim).astype(float)
    v = v + k*s*(1-s)*(mb-ma)**2     # k=1 is exactly the raw lerp's variance
    return np.stack([mu, v+mu*mu], 1)
def analyse(mapname, mid, travel_units, sets):
    g, t59, (hp, hk, hrec) = load(f'{M}/head-{mapname}.prl')
    _, _, (dp, dk, drec) = load(f'{M}/dense-{mapname}.prl')
    o, c, d = g[0], g[1], g[2]
    hk = hk[mid]; dk = dk[mid]; n = len(hk)
    step = travel_units/U/(n-1)
    def base(p):
        if p in t59: return t59[p][INT]
        return np.tile(np.array([[g[4][p], g[5][p]]]), (64, 1))
    def pos(p):
        x = p % d[0]; y = (p // d[0]) % d[1]; z = p // (d[0]*d[1])
        return (-(o[2]+z)*U, -(o[0]+x)*U, (o[1]+y)*U)   # quake x, y, z
    rows = {}
    for k in range(n-1):
        tm = 0.5*(hk[k]+hk[k+1])
        dj = min(range(len(dk)), key=lambda j: abs(dk[j]-tm))
        assert abs(dk[dj]-tm) < 1e-4
        for (m, p), (marks, tl) in hrec.items():
            if m != mid: continue
            dr = drec.get((mid, p))
            b = base(p)
            # truth
            if dr is None: truth = b; tk = 'norecord'
            else:
                dm_, dt_ = dr
                if dm_[dj] == 1: truth = dt_[dj][INT]; tk = 'stored'
                elif dm_[dj] == 2: truth = b; tk = 'base'
                else: tk = 'skipped'; truth = None
            if truth is None: 
                rows.setdefault(('skipped', k), 0); rows[('skipped', k)] += 1; continue
            defined = lambda i: i in (0, n-1) or marks[i] != 0
            lo = k
            while not defined(lo): lo -= 1
            hi = k+1
            while not defined(hi): hi += 1
            w = (tm-hk[lo])/(hk[hi]-hk[lo])
            st = lambda i: tl[i][INT] if marks[i] == 1 else b
            a, bb = st(lo), st(hi)
            termini = (lo == 0 and hi == n-1)
            cur = lerp(a, b, 2*w) if w <= .5 else lerp(b, bb, 2*w-1)
            lim = 2.0*step*(hi-lo)
            cands = {'cur': cur,
                     '1a': cur if termini else slide(a, bb, w),
                     '1a-tb': cur if termini else (slide(a, b, 2*w) if w <= .5 else slide(b, bb, 2*w-1)),
                     'guard2': cur if termini else guard(a, bb, w, lim),
                     'soft': cur if termini else guard(a, bb, w, 1.5*step*(hi-lo), True)}
            wt = W(truth, DS)
            q = pos(p)
            for sname, f in sets.items():
                if not f(q, tm): continue
                for cn, ct in cands.items():
                    rows.setdefault((sname, cn), []).append(W(ct, DS) - wt)
                rows.setdefault((sname, 'n'), []).append((tk, termini, marks[lo] if lo not in (0,n-1) or marks[lo] else 0))
    print('==', mapname, 'mover', mid, 'head knots', n, 'step m', round(step, 3), 'dense-skipped (excluded) records', sum(v for k_, v in rows.items() if k_[0] == 'skipped'))
    for sname in sets:
        meta = rows.get((sname, 'n'), [])
        if not meta: print(sname, 'empty'); continue
        nt = sum(1 for m_ in meta if m_[1])
        print(f'-- {sname}: {len(meta)} probe-midpoints ({nt} termini-only; truth stored {sum(1 for m_ in meta if m_[0]=="stored")}, base {sum(1 for m_ in meta if m_[0] in ("base","norecord"))})')
        E = {cn: np.stack(rows[(sname, cn)]) for cn in ('cur', '1a', '1a-tb', 'guard2', 'soft')}
        for cn, e in E.items():
            line = f'   {cn:7s}'
            for i, dd in enumerate(DS):
                x = e[:, :, i]
                line += f' | d={dd}: mean {x.mean():+.3f} abs {np.abs(x).mean():.3f} leak>.25 {100*(x > .25).mean():4.1f}% dark<-.25 {100*(x < -.25).mean():4.1f}%'
            print(line)
        for cn in ('1a', 'guard2', 'soft'):
            line = f'   {cn} further below truth than cur by >0.1:'
            for i, dd in enumerate(DS):
                sh = (-E[cn][:, :, i]) - np.maximum(-E['cur'][:, :, i], 0)
                msk = sh > 0.1
                line += f' d={dd}: {100*msk.mean():4.1f}% (mean shortfall {sh[msk].mean() if msk.any() else 0:.2f})'
            print(line)
box = lambda q, x0, x1, y0, y1: x0-20 <= q[0] <= x1+20 and y0-20 <= q[1] <= y1+20
analyse('enemy-movers', 1, 256, {
    'east-face column (<=1 cell of face)': lambda q, t: abs(q[0]-(1312+256*t)) <= U and 44 <= q[1] <= 468 and -5 <= q[2] <= 136,
    'all sweeper records': lambda q, t: True})
analyse('mover-setpiece', 1, 176, {
    'ceiling layers': lambda q, t: box(q, 848, 1136, 176, 464) and abs(q[2]-(-16+176*t)) <= U,
    'floor layers': lambda q, t: box(q, 848, 1136, 176, 464) and abs(q[2]-(-176+176*t)) <= U,
    'all room records': lambda q, t: True})
