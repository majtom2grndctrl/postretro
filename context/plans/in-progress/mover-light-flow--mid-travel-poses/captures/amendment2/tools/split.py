# Ruling B: the door-ceiling band's split by probe class (attribution
# captures, light_term_mask 0x02), and the band under each candidate.
import json, os, sys
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, aseries

ts = acap.parse_ts('0.1:0.94:43')
CLS = ('wenc', 'wabove', 'xabove')


def rgb(view, t, bake, form, attr=None):
    p = os.path.join(acap.CAP, acap.label(view, t, bake, form, '02', attr) + '.png')
    return np.asarray(Image.open(p).convert('RGB'), dtype=np.float64) if os.path.exists(p) else None


def luma(a):
    return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]


def to_srgb(l):
    l = np.clip(l, 0, 1)
    return 255 * np.where(l <= 0.0031308, 12.92 * l, 1.055 * l ** (1 / 2.4) - 0.055)


out = dict(ts=ts, band={}, share={}, dark_est={}, seg={})
for seg in ('B', 'A'):
    m = aseries.REGIONS[('door_ceiling', seg)]
    for bake, form in (('head', 'cur'), ('head', 'o40'), ('head', 'r40'), ('shift', 'cur'), ('shift', 'o40'), ('shift', 'r40')):
        out['band'][f'{bake}/{form}' + ('' if seg == 'B' else '/A')] = [
            (float(luma(a)[m].mean()) if (a := rgb('door_ceiling', t, bake, form)) is not None else None) for t in ts]
    for form in ('cur', 'o40', 'r40'):
        sh = {c: [] for c in CLS}; dark = []
        for t in ts:
            full = rgb('door_ceiling', t, 'head', form)
            if full is None:
                for c in CLS: sh[c].append(None)
                dark.append(None); continue
            lf = aseries.lin(full)
            den = luma(lf)[m].mean()
            parts = {}
            for c in CLS:
                a = rgb('door_ceiling', t, 'head', form, c)
                parts[c] = None if a is None else aseries.lin(a)
                sh[c].append(None if a is None else float(luma(parts[c])[m].mean() / max(den, 1e-9)))
            # dark value state for enclosed mid poses, upper bound: the in-wall
            # column's numerator removed, its weight (denominator) kept.
            dark.append(None if parts['wenc'] is None else float(luma(to_srgb(lf - parts['wenc']))[m].mean()))
        key = form + ('' if seg == 'B' else '/A')
        out['share'][key] = sh; out['dark_est'][key] = dark
json.dump(out, open(os.path.join(acap.A, 'results', 'split.json'), 'w', encoding='utf-8'))

def mean_over(xs, lo, hi):
    v = [x for t, x in zip(ts, xs) if x is not None and lo <= t <= hi]
    return np.mean(v) if v else float('nan')

for seg in ('B', 'A'):
    sfx = '' if seg == 'B' else '/A'
    print(f'\n## Segment {seg}: band luma (mask 0x02), mean / max over t 0.10-0.56 and 0.56-0.94')
    for k, v in out['band'].items():
        if (sfx and not k.endswith('/A')) or (not sfx and k.endswith('/A')):
            continue
        vv = [x for x in v if x is not None]
        if not vv: continue
        print(f'  {k:16s} mean {mean_over(v, 0.1, 0.56):6.2f} / {mean_over(v, 0.56, 0.94):6.2f}   max {max(vv):6.2f}  min {min(vv):6.2f}')
    for form in ('cur', 'o40', 'r40'):
        sh = out['share'][form + sfx]
        print(f'  shares under {form}: ' + ', '.join(f'{c} {mean_over(sh[c], 0.1, 0.94):.3f} (max {max([x for x in sh[c] if x is not None] or [float("nan")]):.3f})' for c in CLS))
        d = out['dark_est'][form + sfx]
        print(f'  dark-value estimate under {form}: band mean {mean_over(d, 0.1, 0.56):.2f} / {mean_over(d, 0.56, 0.94):.2f}, max {max([x for x in d if x is not None] or [float("nan")]):.2f}')
    print('  per t (cur): t  band  wenc wabove xabove  rest | o40 band  wenc wabove xabove')
    for i, t in enumerate(ts):
        f = lambda x: '   -' if x is None else f'{x:5.2f}'
        sc, so = out['share']['cur' + sfx], out['share']['o40' + sfx]
        rest = None if any(sc[c][i] is None for c in CLS) else 1 - sum(sc[c][i] for c in CLS)
        print(f'  {t:.2f} {f(out["band"]["head/cur" + sfx][i])} {f(sc["wenc"][i])} {f(sc["wabove"][i])} {f(sc["xabove"][i])} {f(rest)} | {f(out["band"]["head/o40" + sfx][i])} {f(so["wenc"][i])} {f(so["wabove"][i])} {f(so["xabove"][i])}')
