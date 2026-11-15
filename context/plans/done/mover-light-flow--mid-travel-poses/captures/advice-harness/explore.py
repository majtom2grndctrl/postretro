import sys; sys.path.insert(0, sys.argv[1])
from prl57 import *
M = sys.argv[2]
for name in sys.argv[3:]:
    g, t59, (poses, knots, recs) = load(f'{M}/{name}.prl')
    print(name, 'origin', g[0], 'cell', g[1], 'dims', g[2], 'tiles59', len(t59), 'poses', poses)
    from collections import Counter
    c = Counter(mid for mid, _ in recs)
    print('  depth records per mover', dict(c))
    for mid in poses:
        mk = Counter()
        for (m, p), (marks, tl) in recs.items():
            if m == mid:
                for k, x in enumerate(marks): mk[(k, x)] += 1
        print('  mover', mid, [round(k,3) for k in knots[mid]], sorted(mk.items()))
