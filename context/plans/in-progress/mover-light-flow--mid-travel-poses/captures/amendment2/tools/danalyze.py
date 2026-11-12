# Ruling B option (ii): tables from the captures. Writes darkb/results/dsummary.json.
import json, os, sys
import numpy as np
HERE = os.path.dirname(os.path.abspath(__file__))
# Scratch root (ws/, cap/, darkb/): A2_ROOT, else the scratch layout <A>/darkb/tools.
A = os.environ.get('A2_ROOT') or os.path.dirname(os.path.dirname(HERE))
os.environ.setdefault('A2_ROOT', A)
os.environ.setdefault('A2_REGIONS', os.path.join(HERE, 'dregions.json'))
sys.path.insert(0, os.path.join(A, 'tools'))
sys.path.insert(0, HERE)  # acap.py / aseries.py sit beside these in the repo copy
sys.path.insert(0, HERE)
import dcap  # noqa: F401,E402  (registers views)
import acap, aseries  # noqa: E402

OUT = os.path.join(A, 'darkb', 'results')
COLS = [('head', 'cur'), ('head', 'r40'), ('dark', 'cur'), ('dark', 'r40'),
        ('truth', 'cur'), ('truth', 'r40'), ('dtruth', 'cur'), ('dtruth', 'r40')]
# (view, region, t-spec, lag, label)
SPECS = [
    ('door_ceiling', 'B', '0.1:0.6:135', 1, 'Room ceiling band over the west wall (1 m/s)'),
    ('door_ceiling', 'A', '0.1:0.6:135', 1, 'Room ceiling above the door (1 m/s)'),
    ('door_ceiling', 'B', '0:1:101', 1, 'Room ceiling band B, full trip (0.01 steps)'),
    ('door_ceiling', 'A', '0:1:101', 1, 'Room ceiling A, full trip (0.01 steps)'),
    ('sweepfix', 'face', '0:0.5:129', 3, 'Sweeper east face (4.5 m/s)'),
    ('sweepwest', 'face', '0:0.5:129', 3, 'Sweeper west face (4.5 m/s)'),
    ('walledge', 'end', '0.1:0.45:146', 1, 'Wall trailing end face (2 m/s)'),
    ('walledge', 'north', '0.1:0.45:146', 1, 'Wall north face by the edge (2 m/s)'),
    ('east', 'frame', '0:1:101', 1, 'Sweeper, fixed camera east (face, floor, walls)'),
    ('west', 'frame', '0:1:101', 1, 'Sweeper, fixed camera west (face, floor, walls)'),
    ('floor', 'frame', '0,0.2,0.4,0.5,0.6,0.7,0.75,0.8,0.82,0.84,0.86,0.88,0.9,0.92,0.95,0.97,1.0', 1, 'Room floor view'),
]
DARK_BAR = 3.0


def blk_series(view, region, bake, form, ts, mask=None):
    return aseries.series(view, region, bake, form, ts, mask)


def compare(a, b):
    """Per t: (mean |a-b|, worst |a-b|, most negative a-b, blocks with a-b < -DARK_BAR)."""
    out = []
    for x, y in zip(a, b):
        if x is None or y is None:
            out.append(None); continue
        d = x - y
        out.append((float(np.abs(d).mean()), float(np.abs(d).max()), float(d.min()), int((d < -DARK_BAR).sum()), len(d)))
    return out


def worst(cmp, ts):
    v = [(c[2], t, c[3]) for c, t in zip(cmp, ts) if c is not None]
    if not v:
        return None
    lo = min(v)
    return dict(min=lo[0], t=lo[1], n=lo[2], mean_abs=float(np.mean([c[0] for c in cmp if c])),
                worst_abs=float(max(c[1] for c in cmp if c)),
                frames_with_blotch=int(sum(1 for c in cmp if c and c[3] > 0)))


def stats(view, region, tspec, lag, mask=None):
    ts = acap.parse_ts(tspec)
    data = {}
    for b, f in COLS:
        m, bl = blk_series(view, region, b, f, ts, mask)
        if all(x is None for x in m):
            continue
        data[(b, f)] = (m, bl)
    out = {}
    for (b, f), (m, bl) in data.items():
        st = [float(np.abs(bl[i] - bl[i - lag]).max()) for i in range(lag, len(ts)) if bl[i] is not None and bl[i - lag] is not None]
        mv = [x for x in m if x is not None]
        r = dict(series=m, steps=st, step_max=max(st) if st else None, steps_over3=int(sum(s > 3 for s in st)),
                 mean=float(np.mean(mv)), max=float(max(mv)), min=float(min(mv)), range=float(max(mv) - min(mv)),
                 missing=int(sum(x is None for x in m)))
        for ref in ('truth', 'dtruth', 'head'):
            if (ref, f) in data and ref != b:
                c = compare(bl, data[(ref, f)][1])
                r[f'vs_{ref}'] = worst(c, ts)
        out[f'{b}/{f}'] = r
    return ts, out


def band_table():
    """Segment B, mask 02 (static indirect), 0.1:0.94:43, as results.md's Ruling B table."""
    ts = acap.parse_ts('0.1:0.94:43')
    res = {}
    for b, f in COLS + [('shift', 'cur'), ('shift', 'r40'), ('head', 'o40')]:
        m, bl = blk_series('door_ceiling', 'B', b, f, ts, '02')
        if all(x is None for x in m):
            continue
        tt = np.array(ts); mm = np.array([np.nan if x is None else x for x in m])
        st = [abs(mm[i] - mm[i - 1]) for i in range(1, len(ts))]
        res[f'{b}/{f}'] = dict(series=m, early=float(np.nanmean(mm[tt <= 0.56])), late=float(np.nanmean(mm[tt >= 0.56])),
                               max=float(np.nanmax(mm)), mean_step_max=float(np.nanmax(st)))
    return ts, res


def rest():
    """Rest frames t' 0 and 1: dark vs head per view and form, whole frame, pixel exact."""
    from PIL import Image
    out = {}
    for view in ('door_ceiling', 'sweepfix', 'sweepwest', 'walledge', 'floor', 'east', 'west'):
        for f in ('cur', 'r40'):
            for t in (0.0, 1.0):
                pa = os.path.join(acap.CAP, acap.label(view, t, 'head', f) + '.png')
                pb = os.path.join(acap.CAP, acap.label(view, t, 'dark', f) + '.png')
                if not (os.path.exists(pa) and os.path.exists(pb)):
                    continue
                a = np.asarray(Image.open(pa).convert('RGB'), dtype=np.int16)
                b = np.asarray(Image.open(pb).convert('RGB'), dtype=np.int16)
                out[f'{view}/{f}@{t:g}'] = dict(pixels_differ=int((a != b).any(axis=2).sum()), max_abs=int(np.abs(a - b).max()))
    return out


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    summary = {}
    for view, region, tspec, lag, label in SPECS:
        ts, s = stats(view, region, tspec, lag)
        key = f'{view}/{region}/{tspec}'
        summary[key] = dict(label=label, ts=ts, lag=lag, cols=s)
        print(f'\n## {label}  [{view}/{region}, {tspec}, lag {lag}]')
        print('col          step max  n>3   mean    min    max  | vs truth mean/worst | vs dtruth mean/worst | darker-than-head: min (t, blocks)  frames | darker-than-dtruth: min (t, blocks) frames')
        for k, r in s.items():
            def w(key):
                v = r.get(key)
                return '-' if not v else f"{v['mean_abs']:.2f}/{v['worst_abs']:.1f}"
            def dk(key):
                v = r.get(key)
                return '-' if not v else f"{v['min']:+.1f} ({v['t']:.4f}, {v['n']}) {v['frames_with_blotch']}"
            sm = '-' if r['step_max'] is None else f"{r['step_max']:.2f}"
            print(f"{k:12s} {sm:>8s} {r['steps_over3']:4d} {r['mean']:6.2f} {r['min']:6.2f} {r['max']:6.2f}  | {w('vs_truth'):>12s} | {w('vs_dtruth'):>12s} | {dk('vs_head'):>28s} | {dk('vs_dtruth'):>28s}  miss {r['missing']}")
    ts, band = band_table()
    summary['band02'] = dict(ts=ts, cols=band)
    print('\n## Band B, static indirect (mask 02), t 0.10-0.94')
    for k, r in band.items():
        print(f"{k:12s} mean {r['early']:.2f} / {r['late']:.2f}  max {r['max']:.2f}  mean-step max {r['mean_step_max']:.2f}")
    summary['rest'] = rest()
    print('\n## Rest frames, dark vs head (pixels differing, max |d|)')
    for k, v in summary['rest'].items():
        print(f"{k:28s} {v['pixels_differ']:7d} px, max {v['max_abs']}")
    json.dump(summary, open(os.path.join(OUT, 'dsummary.json'), 'w', encoding='utf-8'))
