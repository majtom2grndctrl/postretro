# Series metrics over acap.py captures.
#   python aseries.py <view> <region> <bakes> <forms> <t-spec> [mask] [attr]  -> results/<view>-<region>.json
# Per (bake, form): region mean luma per t; per consecutive pair the largest
# 8x8-block |dLuma| (the diagnosis's step metric) and the region-mean step;
# vs the truth bake under the same form: mean / max block |d| per t; vs the
# current form on the same bake: mean / max block |d| per t (look change).
import json, os, sys
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap

H, W, B = acap.H, acap.W, 8
yy, xx = np.mgrid[0:H, 0:W]


def poly(pts):
    """Pixel mask inside a convex polygon (clockwise or not)."""
    m = np.ones((H, W), bool)
    n = len(pts)
    area = sum(pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1] for i in range(n))
    s = 1.0 if area > 0 else -1.0
    for i in range(n):
        (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % n]
        c = (x1 - x0) * (yy - y0) - (y1 - y0) * (xx - x0)
        m &= (c * s) >= 0
    return m


def rect(x0, y0, x1, y1):
    return (xx >= x0) & (xx < x1) & (yy >= y0) & (yy < y1)


# door_ceiling band segments: opus-tools/band.py (A above the door, B above the solid wall).
_line = 140 + (xx - 40) * (218.0 / 690.0)
_band = (yy >= _line - 45) & (yy <= _line - 3)
REGIONS = {
    ('door_ceiling', 'A'): _band & (xx >= 380) & (xx <= 600),
    ('door_ceiling', 'B'): _band & (xx >= 80) & (xx <= 300),
    ('door_ceiling', 'band'): _band & (xx >= 80) & (xx <= 600),
    ('door_ceiling', 'frame'): np.ones((H, W), bool),
}
if os.environ.get('A2_REGIONS'):
    for k, v in json.load(open(os.environ['A2_REGIONS'], encoding='utf-8')).items():
        view, reg = k.split('/')
        REGIONS[(view, reg)] = poly(v['poly']) if 'poly' in v else rect(*v['rect'])


def lin(a):
    a = a / 255.0
    return np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)


def load(view, t, bake, form, mask=None, attr=None, linear=False):
    p = os.path.join(acap.CAP, acap.label(view, t, bake, form, mask, attr) + '.png')
    if not os.path.exists(p):
        return None
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)
    if linear:
        a = lin(a)
    return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]


def blocks(L, m):
    hb, wb = H // B, W // B
    Lb = L[:hb * B].reshape(hb, B, wb, B).mean(axis=(1, 3))
    mb = m[:hb * B].reshape(hb, B, wb, B).mean(axis=(1, 3)) >= 0.75
    return Lb[mb]


def series(view, region, bake, form, ts, mask=None, attr=None):
    m = REGIONS[(view, region)]
    means, blks = [], []
    for t in ts:
        L = load(view, t, bake, form, mask, attr)
        if L is None:
            means.append(None); blks.append(None); continue
        means.append(float(L[m].mean())); blks.append(blocks(L, m))
    return means, blks


def run(view, region, bakes, forms, ts, mask=None, attr=None):
    out = dict(view=view, region=region, ts=ts, cols={})
    data = {}
    for b in bakes:
        for f in forms:
            data[(b, f)] = series(view, region, b, f, ts, mask, attr)
    for (b, f), (means, blks) in data.items():
        col = dict(mean=means, step_blk=[], step_mean=[], vs_truth=[], vs_truth_max=[], vs_cur=[], vs_cur_max=[])
        for i in range(1, len(ts)):
            if blks[i] is None or blks[i - 1] is None:
                col['step_blk'].append(None); col['step_mean'].append(None); continue
            col['step_blk'].append(float(np.abs(blks[i] - blks[i - 1]).max()))
            col['step_mean'].append(float(abs(means[i] - means[i - 1])))
        for ref, key in ((('truth', f), 'vs_truth'), ((b, 'cur'), 'vs_cur')):
            if ref not in data or ref == (b, f):
                continue
            rb = data[ref][1]
            for i in range(len(ts)):
                if blks[i] is None or rb[i] is None:
                    col[key].append(None); col[key + '_max'].append(None); continue
                d = blks[i] - rb[i]
                col[key].append(float(np.abs(d).mean())); col[key + '_max'].append(float(np.abs(d).max()))
        out['cols'][f'{b}/{f}'] = col
    return out


def summarize(res, step_bar=3.0):
    rows = []
    for name, c in res['cols'].items():
        st = [s for s in c['step_blk'] if s is not None]
        sm = [s for s in c['step_mean'] if s is not None]
        vt = [v for v in c['vs_truth'] if v is not None]
        vc = [v for v in c['vs_cur'] if v is not None]
        rows.append(dict(col=name, step_max=max(st) if st else None,
                         step_p95=float(np.percentile(st, 95)) if st else None,
                         steps_over=sum(s > step_bar for s in st),
                         mean_step_max=max(sm) if sm else None,
                         vs_truth_mean=float(np.mean(vt)) if vt else None,
                         vs_truth_worst=max(v for v in c['vs_truth_max'] if v is not None) if vt else None,
                         vs_cur_mean=float(np.mean(vc)) if vc else None,
                         rest0=(c['vs_cur'][0], c['vs_cur_max'][0]) if vc else None,
                         rest1=(c['vs_cur'][-1], c['vs_cur_max'][-1]) if vc else None))
    return rows


if __name__ == '__main__':
    view, region = sys.argv[1], sys.argv[2]
    bakes, forms, ts = sys.argv[3].split(','), sys.argv[4].split(','), acap.parse_ts(sys.argv[5])
    mask = sys.argv[6] if len(sys.argv) > 6 and sys.argv[6] != '-' else None
    attr = sys.argv[7] if len(sys.argv) > 7 else None
    res = run(view, region, bakes, forms, ts, mask, attr)
    os.makedirs(os.path.join(acap.A, 'results'), exist_ok=True)
    tag = f'{view}-{region}' + (f'-m{mask}' if mask else '') + (f'-a{attr}' if attr else '')
    json.dump(res, open(os.path.join(acap.A, 'results', tag + '.json'), 'w', encoding='utf-8'))
    for r in summarize(res):
        f = lambda v: '-' if v is None else (f'{v:.2f}' if isinstance(v, float) else (f'{v[0]:.2f}/{v[1]:.1f}' if isinstance(v, tuple) else str(v)))
        print(f"{r['col']:14s} step max {f(r['step_max'])} p95 {f(r['step_p95'])} n>3 {f(r['steps_over'])} | mean-step max {f(r['mean_step_max'])}"
              f" | vs truth {f(r['vs_truth_mean'])} worst {f(r['vs_truth_worst'])} | vs cur {f(r['vs_cur_mean'])} rest0 {f(r['rest0'])} rest1 {f(r['rest1'])}")
