import csv, sys
d = sys.argv[1] if len(sys.argv) > 1 else 'out_alt'
rows = list(csv.DictReader(open(f'{d}/alt_sweep.csv')))
for r in rows:
    for k, v in r.items():
        try:
            r[k] = float(v) if k in ('x', 'y', 'z', 'yaw', 'pitch') else int(v)
        except ValueError:
            pass
vs = [k[:-6] for k in rows[0] if k.endswith('_steps') and k != 'ex_steps' and not k.endswith('exact_steps')]

def pct(v, p):
    v = sorted(v)
    return v[round((len(v) - 1) * p)] if v else 0

def box(lo, hi, m=(8, 2, 8)):
    return lambda r: all(lo[i] - m[i] <= r[('x', 'y', 'z')[i]] <= hi[i] + m[i] for i in range(3))

shA = box((-46.13, 18.49, 17.27), (-38.4, 33.73, 24.99))
shB = box((80.67, 18.49, -109.52), (88.39, 33.73, -101.8))
shC = box((-46.13, 18.49, -109.52), (-38.4, 33.73, -101.8))
arena = lambda r: 20 <= r['x'] <= 100 and r['y'] <= 16 and -60 <= r['z'] <= 60
groups = [
    ('overall', lambda r: True),
    ('shaftA', shA),
    ('shaftsBC', lambda r: shB(r) or shC(r)),
    ('arena', arena),
    ('exact-trip@20k', lambda r: r['ex_trip'] == 1),
]
out = []
P = lambda *a: out.append(' '.join(str(x) for x in a))
P(f'walks={len(rows)}  hyb(inf) vs exact mismatching poses={sum(r["hybinf_mismatch"]>0 for r in rows)}  exact permuted-order diff poses={sum(r["ex_permdiff"]>0 for r in rows)}')
for gname, f in groups:
    sel = [r for r in rows if f(r)]
    P(f'\n=== {gname}: walks={len(sel)} cams={len(set(r["cam_cell"] for r in sel))}')
    st = [r['ex_steps'] for r in sel]
    us = [r['ex_ns'] / 1000 for r in sel]
    uc = [r['excap_ns'] / 1000 for r in sel]
    ed = [r['ex_draw'] for r in sel]
    fa = [r['fa_draw'] for r in sel]
    far = [r['fa_draw'] / max(1, r['ex_draw']) for r in sel]
    P(f'exact(uncapped) steps p50/95/99/max {pct(st,.5)}/{pct(st,.95)}/{pct(st,.99)}/{max(st)}  us p50/99/max {pct(us,.5):.1f}/{pct(us,.99):.0f}/{max(us):.0f}  | capped20k+fallback us p50/99/max {pct(uc,.5):.1f}/{pct(uc,.99):.0f}/{max(uc):.0f}')
    P(f'exact draw p50/95/max {pct(ed,.5)}/{pct(ed,.95)}/{max(ed)}   frustum-all draw p50/95/max {pct(fa,.5)}/{pct(fa,.95)}/{max(fa)} ratio p50/95/max {pct(far,.5):.1f}/{pct(far,.95):.1f}/{max(far):.1f}  ex_hd_entries p99/max {pct([r["ex_hd_entries_max"] for r in sel],.99)}/{max(r["ex_hd_entries_max"] for r in sel)}')
    P(f'{"variant":16s} {"steps p50/95/99/max":>22s} {"us p50/99/max":>18s} {"miss r/d":>8s} {"extra p50/95/max":>16s} {"ratio p50/95/max":>17s} {"xfaces p50/95/max":>18s} {"reprop tot99/max":>16s} {"cellmax 99/max":>14s} {"hd 99/max":>9s} perm')
    for v in vs:
        s = [r[v + '_steps'] for r in sel]
        u = [r[v + '_ns'] / 1000 for r in sel]
        mr = sum(r[v + '_miss_reach'] > 0 for r in sel)
        md = sum(r[v + '_miss_draw'] > 0 for r in sel)
        ex = [r[v + '_draw'] - r['ex_draw'] for r in sel]
        ra = [r[v + '_draw'] / max(1, r['ex_draw']) for r in sel]
        xf = [r[v + '_faces'] - r['ex_faces'] for r in sel]
        rt = [r[v + '_reprop_tot'] for r in sel]
        rm = [r[v + '_reprop_max'] for r in sel]
        hd = [r[v + '_hd_reexp_max'] for r in sel]
        pd = sum(r[v + '_permdiff'] > 0 for r in sel)
        P(f'{v:16s} {pct(s,.5):>5}/{pct(s,.95)}/{pct(s,.99)}/{max(s):<6} {pct(u,.5):>6.1f}/{pct(u,.99):.0f}/{max(u):<5.0f} {mr:>3}/{md:<3} {pct(ex,.5):>5}/{pct(ex,.95)}/{max(ex):<5} {pct(ra,.5):>6.2f}/{pct(ra,.95):.2f}/{max(ra):<5.2f} {pct(xf,.5):>5}/{pct(xf,.95)}/{max(xf):<6} {pct(rt,.99):>6}/{max(rt):<6} {pct(rm,.99):>5}/{max(rm):<5} {pct(hd,.99):>3}/{max(hd):<3} {pd}')
    hy = [v for v in vs if v.startswith('hyb')]
    for v in hy:
        t = sum(r[v + '_trip'] for r in sel)
        P(f'  {v}: budget trips={t}')
    extra_cp = [r['oct256_camplane'] for r in sel] if 'oct256_camplane' in sel[0] else []
    P(f'  camera-on-portal-plane bypass poses={sum(c>0 for c in extra_cp)}; eye-plane/unstable parent-region fallbacks oct256 poses={sum(r["oct256_eyefb"]>0 for r in sel)} rect256 poses={sum(r["rect256_eyefb"]>0 for r in sel)}')
txt = '\n'.join(out)
print(txt)
open(f'{d}/alt_summary.txt', 'w').write(txt + '\n')
