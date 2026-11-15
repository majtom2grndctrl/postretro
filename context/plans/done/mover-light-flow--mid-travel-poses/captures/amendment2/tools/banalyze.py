# Big-room re-capture tables (2026-11-13): in-wall band, edge copy, dark blotch, re-light into rest,
# hand-over, rest frames. Writes amend3/results/bsummary.json and prints tables.
#   python banalyze.py [band] [blotch] [relight] [handover] [rest]
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, blib  # noqa: E402

OUT = os.path.join(blib.A3, 'results')
TRIP = blib.trip_ts()                    # 416 samples, one per 60 fps frame (0.002412 t')
COLS = ('head', 'truth', 'ramp')
# In-wall bands (view, region, what) - row 1 / 6 / 3.
BANDS = [
    ('bw', 'W', 'ceiling strip over the west wall (layer x 1521.5)'),
    ('bw', 'WF', 'west wall face'),
    ('bs', 'S', 'ceiling strip over the solid south wall (layer y 1354.0)'),
    ('bs', 'A', 'ceiling strip over the arch'),
    ('bs', 'AF', 'south wall face above the arch'),
    ('bs', 'SF', 'south wall face beside the arch'),
    ('bfloor', 'FW', 'floor strip along the west wall'),
    ('bfloor', 'FS', 'floor strip along the south wall'),
]
# Mover faces and fixed views for the blotch row: (view, region, t-spec).
BLOTCH = [('bw', 'ceil', 'trip'), ('bw', 'WF', 'trip'), ('bs', 'ceil', 'trip'), ('bs', 'SF', 'trip'),
          ('bs', 'AF', 'trip'), ('bfloor', 'floor', 'trip'), ('bfloor', 'WF', 'trip'), ('broof', 'top', '0:1:101'),
          ('barch', 'frame', '0:1:101'), ('bch', 'frame', '0:1:101'), ('bhall', 'frame', '0:1:101')]
HANDOVER = [
    ('walledge', 'end', '0.1:0.45:146', 1, 'Wall door trailing end face (2 m/s)'),
    ('walledge', 'north', '0.1:0.45:146', 1, 'Wall door north face by the edge'),
    ('door_ceiling', 'A', '0.1:0.6:135', 1, 'Small room ceiling above the door (1 m/s)'),
    ('door_ceiling', 'B', '0.1:0.6:135', 1, 'Small room ceiling over the west wall'),
    ('bw', 'ceil', 'trip', 1, 'Big room ceiling, west view (1 m/s)'),
    ('bs', 'ceil', 'trip', 1, 'Big room ceiling, south view'),
]
REST = [('bw', 'frame'), ('bs', 'frame'), ('bfloor', 'frame'), ('broof', 'frame'), ('barch', 'frame'),
        ('bch', 'frame'), ('bhall', 'frame'), ('door_ceiling', 'frame'), ('walledge', 'frame')]


def tspec(s):
    return TRIP if s == 'trip' else acap.parse_ts(s)


def steps(bl, lag=1):
    out = [None] * len(bl)
    for i in range(lag, len(bl)):
        if bl[i] is not None and bl[i - lag] is not None:
            out[i] = float(np.abs(bl[i] - bl[i - lag]).max())
    return out


def smax(st, ts, keep=lambda t: True):
    v = [(s, t) for s, t in zip(st, ts) if s is not None and keep(t)]
    return (None, None, 0) if not v else (*max(v), len(v))


def vs(a, b, ts):
    """Over the series: mean of per-frame block mean |a-b|, worst block |a-b|, most negative block a-b."""
    rows = [(float(np.abs(x - y).mean()), float(np.abs(x - y).max()), float((x - y).min()), t)
            for x, y, t in zip(a, b, ts) if x is not None and y is not None]
    if not rows:
        return None
    w = max(rows, key=lambda r: r[1]); d = min(rows, key=lambda r: r[2])
    return dict(mean=float(np.mean([r[0] for r in rows])), worst=w[1], worst_t=w[3], darkest=d[2], darkest_t=d[3],
                frames_lt3=int(sum(1 for r in rows if r[2] < -3)), n=len(rows))


def band():
    res = {}
    print('\n## Row 1/6: in-wall bands, whole trip at 60 fps frame spacing (416 samples)')
    for mask in ('02', None):
        for view, region, what in BANDS:
            data = {c: blib.series(view, region, c, TRIP, mask) for c in COLS}
            row = {}
            for c in COLS:
                m, bl = data[c]
                mm = np.array([np.nan if x is None else x for x in m])
                if np.isnan(mm).all():
                    continue
                st = steps(bl)
                r = dict(series=m, steps=st, mean=float(np.nanmean(mm)), max=float(np.nanmax(mm)),
                         max_t=TRIP[int(np.nanargmax(mm))], missing=int(np.isnan(mm).sum()))
                r['step'] = smax(st, TRIP)
                r['relight_end'] = smax(st, TRIP, lambda t: t >= 0.95)
                r['relight_start'] = smax(st, TRIP, lambda t: t <= 0.05)
                if c != 'truth':
                    r['vs_truth'] = vs(bl, data['truth'][1], TRIP)
                if c == 'head':
                    r['vs_ramp'] = vs(bl, data['ramp'][1], TRIP)
                row[c] = r
            res[f'{view}/{region}/m{mask}'] = row
            print(f'  [{view}/{region} m{mask}] {what}')
            for c, r in row.items():
                s = r['step']
                line = (f"    {c:6s} mean {r['mean']:6.2f} max {r['max']:6.2f}@{r['max_t']:.4f} step {s[0]:5.2f}@{s[1]:.4f}"
                        f" | relight end {r['relight_end'][0]:.2f}@{r['relight_end'][1]:.4f} start {r['relight_start'][0]:.2f}")
                for k in ('vs_truth', 'vs_ramp'):
                    v = r.get(k)
                    if v:
                        line += f" | {k} mean {v['mean']:.2f} worst {v['worst']:.1f}@{v['worst_t']:.4f} darkest {v['darkest']:+.1f}@{v['darkest_t']:.4f}"
                if r['missing']:
                    line += f" miss {r['missing']}"
                print(line)
    return res


def blotch():
    res = {}
    print('\n## Row 2: dark blotches, most negative 8x8 block (head - ref), frames with a block < -3')
    for view, region, s in BLOTCH:
        ts = tspec(s)
        _, h = blib.series(view, region, 'head', ts)
        row = {}
        for ref in ('ramp', 'truth'):
            _, b = blib.series(view, region, ref, ts)
            v = vs(h, b, ts)
            if v:
                row[ref] = v
        _, r = blib.series(view, region, 'ramp', ts)
        _, tr = blib.series(view, region, 'truth', ts)
        v = vs(r, tr, ts)
        if v:
            row['ramp_vs_truth'] = v
        # Darker than both: per block, the smaller darkening of HEAD against ramp alone and against the
        # dense same-rule truth. A ramp-alone leak that HEAD and the truth both remove cancels here;
        # a dark-state blotch the truth lacks does not.
        both = [(float(np.maximum(x - y, x - z).min()), t) for x, y, z, t in zip(h, r, tr, ts)
                if x is not None and y is not None and z is not None]
        if both:
            m = min(both)
            row['both'] = dict(darkest=m[0], darkest_t=m[1], frames_lt3=int(sum(1 for b, _ in both if b < -3)), n=len(both),
                               mean=0.0, worst=0.0, worst_t=0.0)
        res[f'{view}/{region}'] = row
        print(f'  {view}/{region} {s}: ' + ' | '.join(
            f"{k} darkest {v['darkest']:+.2f}@{v['darkest_t']:.4f} ({v['frames_lt3']} fr) mean|d| {v['mean']:.2f}" for k, v in row.items()))
    return res


def handover():
    res = {}
    print('\n## Row 5: hand-over, largest 8x8-block step per 60 fps frame; head step minus truth step at the same t')
    for view, region, s, lag, label in HANDOVER:
        ts = tspec(s)
        data = {c: blib.series(view, region, c, ts) for c in COLS}
        row = {}
        tst = steps(data['truth'][1], lag)
        for c in COLS:
            m, bl = data[c]
            if all(x is None for x in m):
                continue
            st = steps(bl, lag)
            r = dict(steps=st, step=smax(st, ts), over3=int(sum(1 for x in st if x is not None and x > 3)))
            if c != 'truth':
                ex = [(a - b, t) for a, b, t in zip(st, tst, ts) if a is not None and b is not None]
                if ex:
                    r['excess'] = max(ex)
                r['vs_truth'] = vs(bl, data['truth'][1], ts)
            row[c] = r
        res[f'{view}/{region}'] = dict(label=label, cols=row)
        print(f'  {label} [{view}/{region} {s}]')
        for c, r in row.items():
            line = f"    {c:6s} step {r['step'][0]:5.2f}@{r['step'][1]:.4f} (n>3 {r['over3']})"
            if 'excess' in r:
                line += f" | over truth {r['excess'][0]:+.2f}@{r['excess'][1]:.4f}"
            if r.get('vs_truth'):
                v = r['vs_truth']; line += f" | vs truth {v['mean']:.2f}/{v['worst']:.1f}"
            print(line)
    return res


def rest():
    res = {}
    print('\n## Row 7: rest frames, head against ramp alone (and truth) at t 0 and 1')
    for view, region in REST:
        m = blib.regions()[(view, region)] if (view, region) in blib.regions() else np.ones((blib.H, blib.W), bool)
        for t in (0.0, 1.0):
            a = blib.rgb(view, t, 'head')
            if a is None:
                continue
            row = {}
            for ref in ('ramp', 'truth'):
                b = blib.rgb(view, t, ref)
                if b is None:
                    continue
                px = int((a != b).any(axis=2).sum())
                d = blib.blocks(blib.luma(view, t, 'head'), m) - blib.blocks(blib.luma(view, t, ref), m)
                row[ref] = dict(px=px, worst=float(np.abs(d).max()), mean=float(d.mean()))
            res[f'{view}@{t:g}'] = row
            print(f'  {view} t{t:g}: ' + ' | '.join(f"{k}: {v['px']} px differ, worst block {v['worst']:.2f}" for k, v in row.items()))
    return res


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    parts = sys.argv[1:] or ['band', 'blotch', 'relight', 'handover', 'rest']
    path = os.path.join(OUT, 'bsummary.json')
    out = json.load(open(path, encoding='utf-8')) if os.path.exists(path) else {}
    for p, f in (('band', band), ('blotch', blotch), ('handover', handover), ('rest', rest)):
        if p in parts:
            out[p] = f()
    json.dump(out, open(path, 'w', encoding='utf-8'))
