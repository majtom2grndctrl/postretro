# Ruling A / B tables from the captures. Writes results/summary.json and prints tables.
import json, os, sys
import numpy as np
os.environ.setdefault('A2_REGIONS', os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'regions.json'))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, aseries

FORMS = ['cur', 'r10', 'r25', 'r40', 'o40']
# (view, region, t-spec, frame lag in samples, label)
SPECS = [
    ('sweepfix', 'face', '0:0.5:129', 3, 'Sweeper east face (4.5 m/s)'),
    ('walledge', 'end', '0.1:0.45:146', 1, 'Wall trailing end face (2 m/s)'),
    ('walledge', 'north', '0.1:0.45:146', 1, 'Wall north face by the edge (2 m/s)'),
    ('door_ceiling', 'A', '0.1:0.6:135', 1, 'Room ceiling above the door (1 m/s)'),
    ('door_ceiling', 'B', '0.1:0.6:135', 1, 'Room ceiling band, west wall (1 m/s)'),
]
REST = {'sweepfix': ('0', '1'), 'walledge': ('0', '1'), 'door_ceiling': ('0', '1')}


def stats(view, region, tspec, lag, bakes=('head', 'truth'), forms=FORMS):
    ts = acap.parse_ts(tspec)
    out = {}
    data = {}
    for b in bakes:
        for f in forms:
            m, bl = aseries.series(view, region, b, f, ts)
            if all(x is None for x in m):
                continue
            data[(b, f)] = (m, bl)
    for (b, f), (m, bl) in data.items():
        st = [float(np.abs(bl[i] - bl[i - lag]).max()) for i in range(lag, len(ts)) if bl[i] is not None and bl[i - lag] is not None]
        ms = [abs(m[i] - m[i - lag]) for i in range(lag, len(ts)) if m[i] is not None and m[i - lag] is not None]
        r = dict(step_max=max(st), steps_over3=int(sum(s > 3 for s in st)), steps_over2=int(sum(s > 2 for s in st)),
                 mean_step_max=max(ms), mean=float(np.mean([x for x in m if x is not None])),
                 range=float(max(x for x in m if x is not None) - min(x for x in m if x is not None)), n=len(st),
                 series=m, steps=st)
        if b != 'truth' and ('truth', f) in data:
            tb = data[('truth', f)][1]
            d = [np.abs(bl[i] - tb[i]) for i in range(len(ts)) if bl[i] is not None and tb[i] is not None]
            r['vs_truth_mean'] = float(np.mean([x.mean() for x in d]))
            r['vs_truth_worst'] = float(max(x.max() for x in d))
        if f != 'cur' and (b, 'cur') in data:
            cb = data[(b, 'cur')][1]
            d = [np.abs(bl[i] - cb[i]) for i in range(len(ts)) if bl[i] is not None and cb[i] is not None]
            r['vs_cur_mean'] = float(np.mean([x.mean() for x in d]))
        out[f'{b}/{f}'] = r
    return ts, out


def rest(view, region, forms=FORMS):
    out = {}
    for t in (0.0, 1.0):
        c = aseries.load(view, t, 'head', 'cur')
        if c is None:
            continue
        m = aseries.REGIONS[(view, region)]
        cb = aseries.blocks(c, m)
        for f in forms[1:]:
            L = aseries.load(view, t, 'head', f)
            if L is None:
                continue
            d = aseries.blocks(L, m) - cb
            out[f'{f}@{t:g}'] = dict(mean=float(d.mean()), absmean=float(np.abs(d).mean()), worst=float(np.abs(d).max()),
                                     cur=float(c[m].mean()), form=float(L[m].mean()))
    return out


if __name__ == '__main__':
    summary = {}
    for view, region, tspec, lag, label in SPECS:
        ts, s = stats(view, region, tspec, lag, bakes=('head', 'truth', 'shift'))
        summary[f'{view}/{region}'] = dict(label=label, ts=ts, lag=lag, cols=s, rest=rest(view, region))
        print(f'\n## {label}  [{view}/{region}, {tspec}, lag {lag}]')
        print('col           step max  n>3  n>2  mean-step  range  vs truth (mean/worst)  vs cur')
        for k, r in s.items():
            vt = f"{r.get('vs_truth_mean', float('nan')):.2f}/{r.get('vs_truth_worst', float('nan')):.1f}"
            print(f"{k:12s} {r['step_max']:8.2f} {r['steps_over3']:4d} {r['steps_over2']:4d} {r['mean_step_max']:9.2f} {r['range']:6.2f}  {vt:>20s}  {r.get('vs_cur_mean', float('nan')):.2f}")
        for k, r in summary[f'{view}/{region}']['rest'].items():
            print(f"  rest {k}: mean d {r['mean']:+.2f}, |d| {r['absmean']:.2f}, worst block {r['worst']:.1f} (cur {r['cur']:.2f} -> {r['form']:.2f})")
    os.makedirs(os.path.join(acap.A, 'results'), exist_ok=True)
    json.dump(summary, open(os.path.join(acap.A, 'results', 'summary.json'), 'w', encoding='utf-8'))
