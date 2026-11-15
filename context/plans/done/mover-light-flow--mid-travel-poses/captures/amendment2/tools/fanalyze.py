# Task 25c tables a, b, c, e, f. Writes final/results/fsummary.json and prints tables.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flib, acap  # noqa: E402

OUT = os.path.join(flib.F, 'results')
TRIP = acap.parse_ts('0:1:269')          # room frame spacing, 0.00373 t' (0.66 u at 1 m/s)

# (view, region, t-spec, lag, label)
HANDOVER = [
    ('sweepfix', 'face', '0:0.5:129', 3, 'Sweeper east face (4.5 m/s)'),
    ('walledge', 'end', '0.1:0.45:146', 1, 'Wall trailing end face (2 m/s)'),
    ('walledge', 'north', '0.1:0.45:146', 1, 'Wall north face by the edge (2 m/s)'),
    ('door_ceiling', 'A', '0.1:0.6:135', 1, 'Room ceiling above the door (1 m/s)'),
    ('door_ceiling', 'B', '0.1:0.6:135', 1, 'Room ceiling band over the west wall (1 m/s)'),
]
BLOTCH = [
    ('sweepfix', 'face', '0:0.5:129'), ('sweepwest', 'face', '0:0.5:129'),
    ('walledge', 'end', '0.1:0.45:146'), ('walledge', 'north', '0.1:0.45:146'),
    ('east', 'frame', '0:1:101'), ('west', 'frame', '0:1:101'),
]
REST_VIEWS = [('door_ceiling', 'frame'), ('door_ceiling', 'A'), ('door_ceiling', 'B'), ('sweepfix', 'face'),
              ('sweepwest', 'face'), ('walledge', 'end'), ('walledge', 'north'), ('floor', 'frame'),
              ('east', 'frame'), ('west', 'frame'), ('wall_owner', 'frame'), ('sweeper_east', 'face')]


def colstats(view, region, ts, col, lag, ref=None, mask=None):
    m, bl = flib.series(view, region, col, ts, mask)
    if all(x is None for x in m):
        return None
    st = flib.steps(bl, lag)
    r = dict(series=m, steps=st, step=flib.summarize_steps(st, ts),
             missing=int(sum(x is None for x in m)))
    mv = [x for x in m if x is not None]
    r['mean'] = float(np.mean(mv)); r['max'] = float(max(mv)); r['min'] = float(min(mv))
    if ref:
        rm, rb = flib.series(view, region, ref, ts, mask)
        r['vs_' + ref] = flib.summarize_vs(flib.vs(bl, rb), ts)
        # step excess over the reference's step at the same t
        rst = flib.steps(rb, lag)
        ex = [(a - b, t) for a, b, t in zip(st, rst, ts) if a is not None and b is not None]
        if ex:
            e = max(ex)
            r['step_excess_vs_' + ref] = dict(max=e[0], at=e[1])
    return r


def handover():
    res = {}
    for view, region, tspec, lag, label in HANDOVER:
        ts = acap.parse_ts(tspec)
        cols = {}
        for col, ref in (('head', 'truth'), ('truth', None), ('P:dark-r40', 'P:dtruth-r40'),
                         ('P:dtruth-r40', None), ('P:head-r40', 'P:truth-r40'), ('P:head-cur', 'P:truth-cur')):
            r = colstats(view, region, ts, col, lag, ref)
            if r:
                cols[col] = r
        res[f'{view}/{region}'] = dict(label=label, ts=ts, lag=lag, cols=cols)
        print(f'\n## a. {label} [{view}/{region} {tspec} lag {lag}]')
        for c, r in cols.items():
            s = r['step']
            line = f"  {c:14s} step max {s['max']:6.2f} at {s['at']:.4f} (n>3 {s['over']:3d}/{s['n']})"
            for k, v in r.items():
                if k.startswith('vs_') and v:
                    line += f" | {k} mean {v['mean']:.2f} worst {v['worst']:.1f} at {v['worst_t']:.4f}"
                if k.startswith('step_excess') and v:
                    line += f" | step-over-ref max {v['max']:+.2f} at {v['at']:.4f}"
            if r['missing']:
                line += f" miss {r['missing']}"
            print(line)
    return res


def band():
    """b and e: band B (and A) over the whole trip at room frame spacing, mask 02 and full frame."""
    res = {}
    for mask in ('02', None):
        for region in ('B', 'A'):
            cols = {}
            for col in ('head', 'truth', 'odark', 'ohead', 'odtruth'):
                m, bl = flib.series('door_ceiling', region, col, TRIP, mask)
                if all(x is None for x in m):
                    continue
                st = flib.steps(bl, 1)
                tt = np.array(TRIP); mm = np.array([np.nan if x is None else x for x in m])
                r = dict(series=m, steps=st, mean=float(np.nanmean(mm)), max=float(np.nanmax(mm)),
                         max_t=float(tt[np.nanargmax(mm)]),
                         mean_a=float(np.nanmean(mm[(tt >= 0.10) & (tt <= 0.56)])),
                         mean_b=float(np.nanmean(mm[(tt >= 0.56) & (tt <= 0.94)])),
                         step=flib.summarize_steps(st, TRIP),
                         relight=flib.summarize_steps([s if t >= 0.95 else None for s, t in zip(st, TRIP)], TRIP, 1.5),
                         missing=int(np.isnan(mm).sum()))
                for ref in ('truth', 'odark'):
                    if ref != col:
                        rm, rb = flib.series('door_ceiling', region, ref, TRIP, mask)
                        r['vs_' + ref] = flib.summarize_vs(flib.vs(bl, rb), TRIP)
                        d = [(a - b, t) for a, b, t in zip(m, rm, TRIP) if a is not None and b is not None]
                        if d:
                            r['mean_minus_' + ref] = dict(mean=float(np.mean([x for x, _ in d])),
                                                          max=max(d), min=min(d))
                cols[col] = r
            res[f'{region}/m{mask}'] = cols
            print(f'\n## b/e. Band {region}, {"mask 02 (static indirect)" if mask else "full frame"}, whole trip at frame spacing')
            for c, r in cols.items():
                s, rl = r['step'], r['relight']
                line = (f"  {c:8s} mean {r['mean']:6.2f} (0.10-0.56 {r['mean_a']:.2f} / 0.56-0.94 {r['mean_b']:.2f}) "
                        f"max {r['max']:6.2f} at {r['max_t']:.4f} | step max {s['max']:.2f} at {s['at']:.4f} n>3 {s['over']}"
                        f" | relight(t'>=0.95) max {rl['max']:.2f} at {rl['at']:.4f} n>1.5 {rl['over']}")
                for ref in ('truth', 'odark'):
                    v = r.get('vs_' + ref); d = r.get('mean_minus_' + ref)
                    if v:
                        line += (f" | vs {ref}: block mean {v['mean']:.2f} worst {v['worst']:.1f} at {v['worst_t']:.4f};"
                                 f" mean diff {d['mean']:+.2f} (max {d['max'][0]:+.2f} at {d['max'][1]:.4f}, min {d['min'][0]:+.2f} at {d['min'][1]:.4f})")
                if r['missing']:
                    line += f' miss {r["missing"]}'
                print(line)
    return res


def blotch():
    res = {}
    print('\n## c. Dark blotches: most negative 8x8 block (col - ref), t, frames with any block < -3')
    for view, region, tspec in BLOTCH:
        ts = acap.parse_ts(tspec)
        row = {}
        for col, ref in (('head', 'truth'), ('head', 'P:head-r40'), ('P:dark-r40', 'P:dtruth-r40'), ('P:dark-r40', 'P:head-r40')):
            _, a = flib.series(view, region, col, ts)
            _, b = flib.series(view, region, ref, ts)
            s = flib.summarize_vs(flib.vs(a, b), ts)
            if s:
                row[f'{col} vs {ref}'] = s
        res[f'{view}/{region}'] = row
        print(f'  {view}/{region} {tspec}')
        for k, s in row.items():
            print(f"    {k:28s} darkest {s['darkest']:+6.2f} at {s['darkest_t']:.4f} ({s['frames_blotch']} frames) | mean |d| {s['mean']:.2f} worst |d| {s['worst']:.1f} n {s['n']}")
    return res


def rest():
    res = {}
    print('\n## f. Rest frames: final HEAD against pre-amendment (P:head-cur) and ramp-alone (P:head-r40)')
    for view, region in REST_VIEWS:
        m = flib.REGIONS[(view, region)] if (view, region) in flib.REGIONS else np.ones((flib.H, flib.W), bool)
        for t in (0.0, 1.0):
            L = flib.luma(view, t, 'head')
            if L is None:
                continue
            b = flib.blocks(L, m)
            row = {}
            for ref in ('P:head-cur', 'P:head-r40'):
                R = flib.luma(view, t, ref)
                if R is None:
                    continue
                d = b - flib.blocks(R, m)
                a1 = flib.rgb(view, t, 'head'); a2 = flib.rgb(view, t, ref)
                px = int((a1 != a2).any(axis=2).sum())
                row[ref] = dict(mean=float(d.mean()), absmean=float(np.abs(d).mean()), worst=float(np.abs(d).max()),
                                px_differ=px, frac=px / (flib.H * flib.W))
            res[f'{view}/{region}@{t:g}'] = row
            s = '  '.join(f"{k}: mean {v['mean']:+.2f} |d| {v['absmean']:.2f} worst blk {v['worst']:.1f} px {v['frac']*100:.0f}%" for k, v in row.items())
            print(f'  {view}/{region} t{t:g}: {s}')
    return res


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    parts = sys.argv[1:] or ['a', 'b', 'c', 'f']
    out = {}
    path = os.path.join(OUT, 'fsummary.json')
    if os.path.exists(path):
        out = json.load(open(path, encoding='utf-8'))
    if 'a' in parts: out['handover'] = handover()
    if 'b' in parts: out['band'] = band()
    if 'c' in parts: out['blotch'] = blotch()
    if 'f' in parts: out['rest'] = rest()
    json.dump(out, open(path, 'w', encoding='utf-8'))
