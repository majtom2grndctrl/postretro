import sys; sys.path.insert(0, sys.argv[1])
exec(open(sys.argv[1]+'/view.py').read().split("def gridpts")[0].replace("M = sys.argv[2]; U = 39.37", "M = sys.argv[2]; U = 39.37"))
from collections import Counter
def diag(mapname, mid, travel, q_fn, nq, tms):
    g, t59, (hp, hk, hrec) = load(f'{M}/head-{mapname}.prl')
    _, _, (dp, dk, drec) = load(f'{M}/dense-{mapname}.prl')
    o, c, d, valid, bm, bq = g; hk = hk[mid]; dk = dk[mid]
    ne = np.array([-nq[1], nq[2], -nq[0]], float)
    base = lambda p: t59[p] if p in t59 else np.tile(np.array([[bm[p], bq[p]]]), (100, 1))
    for tm in tms:
        q = q_fn(tm); pe = np.array([-q[1], q[2], -q[0]])/U
        gp = pe - np.array(o); gi = np.floor(gp).astype(int); fr = gp - gi
        print(f'-- {mapname} t={tm} receiver frac {fr.round(2)}')
        for cy in (0, 1):
          for cx, cz in ((0, 0),):
            idx = gi + np.array([cx, cy, cz]); p = int(idx[0] + d[0]*(idx[1] + d[1]*idx[2]))
            off = np.array([cx, cy, cz]); dtp = off - fr; dist = np.linalg.norm(dtp); to_s = -dtp
            hr = hrec.get((mid, p)); dr = drec.get((mid, p)); b = base(p)
            f = lambda t_: tap(t_, to_s).round(2)
            print(f'   corner y{cy} probe {p} valid {valid[p]} dist {dist:.2f} side {"far" if np.dot(dtp, ne) < 0 else "near"} tiled59 {p in t59} base {f(b)}')
            if hr: print('     head marks', hr[0], {k: tuple(f(v)) for k, v in hr[1].items()})
            if dr: print('     dense marks', dr[0], {k: tuple(f(v)) for k, v in dr[1].items()})
            for r in ('cur', '1a'):
                if hr:
                    tc = compose(r, hr[0], hr[1], b, hk, tm, 1); print(f'     {r}: moments {f(tc)} W {cheb(tap(tc, to_s), dist):.2f}')
            tt = compose('cur', dr[0], dr[1], b, dk, tm, 1) if dr else b
            print(f'     truth: moments {f(tt)} W {cheb(tap(tt, to_s), dist):.2f}')
diag('mover-setpiece', 1, 176, lambda t: (992, 320, -16+176*t-0.5), (0, 0, -1), (0.3, 0.5, 0.7))
