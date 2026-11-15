import sys; sys.path.insert(0, sys.argv[1])
from val57 import *
from collections import Counter
for path in sys.argv[2:]:
    g, (poses, knots, direct, bounce, at), fmt = loadv(path)
    print(path.split('/')[-1], 'irr fmt', fmt, 'poses', poses)
    for m in poses:
        c = Counter(); n = 0; acr = 0
        for p, lst in direct[m].items():
            for ns, light, q, t in lst: c[(ns, light, t is not None)] += 1; acr += int(np.isnan(q).sum())
        bm = Counter(x for p, (marks, tl) in bounce[m].items() for x in marks)
        print('  mover', m, 'direct probes', len(direct[m]), 'records by (ns, light, own)', dict(c), 'across marks', acr, '| bounce probes', len(bounce[m]), 'marks', dict(bm))
