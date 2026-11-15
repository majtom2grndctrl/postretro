# Metrics over mcap.py's captures.
#   venv/bin/python -I mmetrics.py [view ...]   -> motion/results/<view>.json
# Per view, column and t': crop mean luma; 12c (floor, ceiling) or 12d
# against strip (faces); per consecutive pair the per-cell |dLuma| max / p95
# and its argmax; per interior sample the per-cell |second difference| max /
# p95 (kinks). Cells: 8x8 pixel blocks for the room views (camera rides the
# room, so blocks are surface-fixed), CELL x CELL face-space cells for the
# fixed-camera face views (cap.py's views; texture would otherwise swim).
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcap

OUT = os.path.join(mcap.M, 'results')


def cells_for(view, spec, luma, mask):
    if view in ('west', 'east'):
        return mcap.face_cells(spec, luma)
    bm = mcap.block_means(luma)
    if mask is None:
        return bm
    return np.where(mask, bm, np.nan)


def view_metrics(view):
    ts = mcap.ts_for(view)
    make = mcap.VIEWS[view]
    rows = []
    cells = {c: [] for c in mcap.COLUMNS}
    for t in ts:
        spec = make(t)
        lum = {c: mcap.read_luma(os.path.join(mcap.CAP, mcap.label(view, t, c) + '.png'))
               for c in mcap.COLUMNS}
        row = {'t': t, 'frame_mean': {c: float(l.mean()) for c, l in lum.items()}}
        mask = mcap.crop_blocks(spec) if spec['plane'] is not None else None
        if mask is not None:
            bm = {c: mcap.block_means(l) for c, l in lum.items()}
            row['blocks'] = int(mask.sum())
            row['crop_mean'] = {c: float(b[mask].mean()) for c, b in bm.items()}
            if view in ('floor', 'ceiling'):
                row['12c'] = {c: mcap.excess_metric(bm[c], mask) for c in mcap.COLUMNS}
            row['12d'] = {}
            for c in ('head', 'nodepth', 'two'):
                diff = bm[c] - bm['strip']
                e = mcap.excess_metric(diff, mask)
                e['mean'] = float(diff[mask].mean())
                e['worst_dark'] = float(diff[mask].min())
                e['worst_bright'] = float(diff[mask].max())
                row['12d'][c] = e
        else:
            row['crop_mean'] = dict(row['frame_mean'])
        for c in mcap.COLUMNS:
            cells[c].append(cells_for(view, spec, lum[c], mask))
        rows.append(row)
        print(view, t, json.dumps(row['crop_mean']), flush=True)
    # Steps and kinks.
    steps = {c: [] for c in mcap.COLUMNS}
    kinks = {c: [] for c in mcap.COLUMNS}
    for c in mcap.COLUMNS:
        arr = np.stack(cells[c])  # (T, ...)
        for i in range(len(ts) - 1):
            d = np.abs(arr[i + 1] - arr[i])
            v = d[np.isfinite(d)]
            am = np.nanargmax(np.where(np.isfinite(d), d, -1))
            steps[c].append(dict(t0=ts[i], t1=ts[i + 1], max=float(v.max()), p95=float(np.percentile(v, 95)),
                                 mean=float(v.mean()), argmax=[int(x) for x in np.unravel_index(am, d.shape)],
                                 n=int(v.size)))
        for i in range(1, len(ts) - 1):
            d2 = np.abs(arr[i + 1] - 2 * arr[i] + arr[i - 1])
            v = d2[np.isfinite(d2)]
            kinks[c].append(dict(t=ts[i], max=float(v.max()), p95=float(np.percentile(v, 95))))
        np.save(os.path.join(OUT, f'{view}-{c}-cells.npy'), arr)
    res = dict(view=view, ts=ts, rows=rows, steps=steps, kinks=kinks)
    with open(os.path.join(OUT, f'{view}.json'), 'w') as f:
        json.dump(res, f, indent=1)


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    for v in sys.argv[1:] or list(mcap.VIEWS):
        view_metrics(v)
