import sys; sys.path.insert(0, sys.argv[1])
from prl57 import *
M = sys.argv[2]
for name, mid in (('head-enemy-movers', 1), ('head-mover-setpiece', 1)):
    g, t59, (poses, knots, recs) = load(f'{M}/{name}.prl')
    o, c, d = g[0], g[1], g[2]
    for pose in (0, poses[mid]-1):
        dark = []
        for (m, p), (marks, tl) in recs.items():
            if m == mid and pose in tl and tl[pose][:,0].max() < 0.02:
                x = p % d[0]; y = (p // d[0]) % d[1]; z = p // (d[0]*d[1])
                dark.append((o[0]+x, o[1]+y, o[2]+z))
        a = np.array(dark)
        print(name, 'pose', pose, 'dark probes', len(dark), 'min', a.min(0).round(2) if len(dark) else None, 'max', a.max(0).round(2) if len(dark) else None, 'in quake units', (a.min(0)*39.37).round(0) if len(dark) else None, (a.max(0)*39.37).round(0) if len(dark) else None)
