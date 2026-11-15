# Crop means (and 12c peaks for floor/ceiling, 12d vs strip for faces) for
# head | nodepth | dense | densenodepth | two | strip, per view and t'.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcap
COLS = ['head', 'nodepth', 'dense', 'densenodepth', 'two', 'strip']
out = {}
for view in sys.argv[1:] or list(mcap.VIEWS):
    rows = []
    cells = {c: [] for c in COLS}
    for t in mcap.ts_for(view):
        spec = mcap.VIEWS[view](t)
        lum = {c: mcap.read_luma(os.path.join(mcap.CAP, mcap.label(view, t, c) + '.png')) for c in COLS}
        if spec['plane'] is not None:
            m = mcap.crop_blocks(spec)
            bm = {c: mcap.block_means(l) for c, l in lum.items()}
            row = dict(t=t, mean={c: float(bm[c][m].mean()) for c in COLS})
            if view in ('floor', 'ceiling'):
                row['peaks'] = {c: mcap.excess_metric(bm[c], m)['peaks'] for c in COLS}
                row['bright'] = {c: mcap.excess_metric(bm[c], m)['bright'] for c in COLS}
            else:
                row['d12'] = {c: mcap.excess_metric(bm[c] - bm['strip'], m) for c in COLS if c != 'strip'}
            for c in COLS:
                cells[c].append(mcap.face_cells(spec, lum[c]) if view in ('west', 'east') else np.where(m, bm[c], np.nan))
        else:
            row = dict(t=t, mean={c: float(l.mean()) for c, l in lum.items()})
            for c in COLS:
                cells[c].append(mcap.block_means(lum[c]))
        rows.append(row)
    steps = {}
    for c in COLS:
        a = np.stack(cells[c])
        d = np.abs(np.diff(a, axis=0))
        steps[c] = [float(np.nanmax(x)) for x in d]
    out[view] = dict(rows=rows, steps=steps)
    print(view, 'done', flush=True)
json.dump(out, open(os.path.join(mcap.M, 'results', 'dense.json'), 'w'), indent=1)
