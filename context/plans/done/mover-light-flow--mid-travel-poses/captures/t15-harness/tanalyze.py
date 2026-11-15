# Task 15 motion analysis: crop means, per-cell steps, 12c, knot flash,
# sawtooth, and |X - truth| at the truth bake's knots; charts and sheets.
#   venv/bin/python -I tanalyze.py
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from PIL import Image, ImageDraw
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tcap
mcap = tcap.mcap

T = tcap.T
RES = os.path.join(T, 'results')
CH = os.path.join(T, 'charts')
SH = os.path.join(T, 'sheets')
SWEEPER_AMEND = [0.0, 0.07631, 0.0924, 0.10848, 0.14286, 0.2301, 0.24619, 0.26227, 0.28571, 0.38389,
                 0.39998, 0.41606, 0.42857, 0.53768, 0.55376, 0.56985, 0.69147, 0.70755, 0.71429,
                 0.72364, 0.84526, 0.85714, 0.86134, 0.87743, 1.0]
ROOM_HEAD = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
SWEEPER_HEAD = [k / 7 for k in range(8)]
GAIN = {'floor': 3.0, 'ceiling': 6.0, 'interior': 4.0}
COLORS = {'amend': '#7b2cbf', 'head': '#d1495b', 'two': '#3b8b3b', 'truth': '#111111'}
NAMES = {'amend': 'amend (HEAD: geometry knots + mean/variance depth)',
         'head': 'head (pre-amendment: even poses, through-base depth)',
         'two': 'light_poses 2 (pre-brief)', 'truth': 'truth (light_poses 41 / 57, at its knots)'}


def room(view):
    return view in tcap.ROOM_VIEWS


def amend_knots(view):
    return tcap.AMEND_ROOM_KNOTS if room(view) else SWEEPER_AMEND


def head_knots(view):
    return ROOM_HEAD if room(view) else SWEEPER_HEAD


def frame(view, t, col):
    if col == 'two':
        p = os.path.join(tcap.OLD_CAP, mcap.label(view, t, col) + '.png')
    else:
        p = tcap.path_for(view, t, col)
    return mcap.read_luma(p)


def cells(view, spec, luma, mask):
    if view in ('west', 'east'):
        return mcap.face_cells(spec, luma)
    bm = mcap.block_means(luma)
    return bm if mask is None else np.where(mask, bm, np.nan)


def measure(view, t, col):
    spec = mcap.VIEWS[view](t)
    lum = frame(view, t, col)
    mask = mcap.crop_blocks(spec) if spec['plane'] is not None else None
    out = {'t': t}
    if mask is not None:
        bm = mcap.block_means(lum)
        out['mean'] = float(bm[mask].mean())
        if view in ('floor', 'ceiling'):
            out['12c'] = mcap.excess_metric(bm, mask)
    else:
        out['mean'] = float(lum.mean())
    return out, cells(view, spec, lum, mask)


def series(view, grid, col):
    ts = tcap.ts_for(view, grid)
    rows, cs = [], []
    for t in ts:
        r, c = measure(view, t, col)
        rows.append(r)
        cs.append(c)
    return rows, np.stack(cs)


def steps(ts, arr):
    out = []
    for i in range(len(ts) - 1):
        d = np.abs(arr[i + 1] - arr[i])
        v = d[np.isfinite(d)]
        out.append(dict(t0=ts[i], t1=ts[i + 1], max=float(v.max()), p95=float(np.percentile(v, 95))))
    return out


def kinks(ts, arr):
    out = []
    for i in range(1, len(ts) - 1):
        d2 = np.abs(arr[i + 1] - 2 * arr[i] + arr[i - 1])
        v = d2[np.isfinite(d2)]
        out.append(dict(t=ts[i], max=float(v.max())))
    return out


def ticks(ax, view):
    for x in amend_knots(view):
        ax.axvline(x, color='#7b2cbf', lw=0.6, alpha=0.35)
    for x in head_knots(view):
        ax.axvline(x, color='#d1495b', lw=0.8, ls=':', alpha=0.6)


def analyse(view):
    std = {c: series(view, 'std', c) for c in ('amend', 'head', 'two')}
    tk = {c: series(view, 'truth', c) for c in ('amend', 'head', 'truth')}
    ts = tcap.ts_for(view, 'std')
    tts = tcap.ts_for(view, 'truth')
    res = {'view': view, 'std_ts': ts, 'truth_ts': tts,
           'std': {c: r for c, (r, _) in std.items()}, 'truthgrid': {c: r for c, (r, _) in tk.items()},
           'steps': {c: steps(ts, a) for c, (_, a) in std.items()},
           'kinks': {c: kinks(ts, a) for c, (_, a) in std.items()},
           'truth_steps': {c: steps(tts, a) for c, (_, a) in tk.items()}}
    # |X - truth| at the truth knots: crop mean, and per cell.
    tr_mean = np.array([r['mean'] for r in tk['truth'][0]])
    res['vs_truth'] = {}
    for c in ('amend', 'head'):
        m = np.array([r['mean'] for r in tk[c][0]])
        d = np.abs(tk[c][1] - tk['truth'][1])
        per_t = [float(np.nanmax(x)) for x in d]
        res['vs_truth'][c] = dict(mean_abs=float(np.mean(np.abs(m - tr_mean))),
                                  max_abs=float(np.max(np.abs(m - tr_mean))),
                                  t_max=tts[int(np.argmax(np.abs(m - tr_mean)))],
                                  signed_mean=float(np.mean(m - tr_mean)),
                                  cell_mean_abs=float(np.nanmean(d)), cell_max=float(np.max(per_t)),
                                  cell_t_max=tts[int(np.argmax(per_t))])
    if view == 'ceiling':
        fl = {}
        for k in tcap.AMEND_ROOM_KNOTS[1:-1]:
            at = measure(view, round(k, 6), 'amend')[0]['mean']
            lo = measure(view, round(k - 0.001, 6), 'amend')[0]['mean']
            hi = measure(view, round(k + 0.001, 6), 'amend')[0]['mean']
            fl[k] = dict(at=at, minus=lo, plus=hi, drop=0.5 * (lo + hi) - at)
        res['flash'] = fl
    return res


def luma_chart(view, R):
    fig, axes = plt.subplots(2, 1, figsize=(11, 7.5), sharex=True, gridspec_kw=dict(height_ratios=[3, 2]))
    ax = axes[0]
    for c in ('two', 'head', 'amend'):
        ax.plot(R['std_ts'], [r['mean'] for r in R['std'][c]], marker='o', ms=2.2, lw=1.2,
                color=COLORS[c], label=NAMES[c])
    ax.plot(R['truth_ts'], [r['mean'] for r in R['truthgrid']['truth']], marker='s', ms=3, lw=1.0,
            color=COLORS['truth'], label=NAMES['truth'])
    ticks(ax, view)
    ax.set_ylabel('crop mean luma (Rec. 709, 0-255)')
    ax.set_title(f"{view}: crop mean luma vs t' (purple: amend knots; red dotted: pre-amendment knots)")
    ax.legend(fontsize=8)
    ax = axes[1]
    tr = np.array([r['mean'] for r in R['truthgrid']['truth']])
    for c in ('head', 'amend'):
        m = np.array([r['mean'] for r in R['truthgrid'][c]])
        ax.plot(R['truth_ts'], m - tr, marker='o', ms=2.5, lw=1.2, color=COLORS[c], label=f'{c} - truth')
    ax.axhline(0, color='k', lw=0.6)
    ticks(ax, view)
    ax.set_xlabel("t'")
    ax.set_ylabel('crop mean - truth (at truth knots)')
    ax.legend(fontsize=8)
    fig.tight_layout()
    p = os.path.join(CH, f'{view}-luma.png')
    fig.savefig(p, dpi=110)
    plt.close(fig)
    return p


def step_chart(view, R):
    fig, axes = plt.subplots(2, 1, figsize=(11, 7), sharex=True)
    for c in ('two', 'head', 'amend'):
        s = R['steps'][c]
        mid = [0.5 * (x['t0'] + x['t1']) for x in s]
        axes[0].plot(mid, [x['max'] for x in s], marker='o', ms=2.2, lw=1.1, color=COLORS[c], label=NAMES[c])
        k = R['kinks'][c]
        axes[1].plot([x['t'] for x in k], [x['max'] for x in k], marker='o', ms=2.2, lw=1.1, color=COLORS[c])
    cell = 'face cell (8x8 u)' if view in ('west', 'east') else '8x8 block'
    axes[0].set_ylabel(f'max |dLuma| per {cell}')
    axes[1].set_ylabel('max |2nd difference|')
    axes[0].set_title(f"{view}: per-cell change between consecutive t' samples")
    for ax in axes:
        ticks(ax, view)
    axes[1].set_xlabel("t'")
    axes[0].legend(fontsize=8)
    fig.tight_layout()
    p = os.path.join(CH, f'{view}-steps.png')
    fig.savefig(p, dpi=110)
    plt.close(fig)
    return p


def sheet(view, t_center, gain):
    ts = tcap.ts_for(view, 'std')
    i = min(range(len(ts)), key=lambda j: abs(ts[j] - t_center))
    sel = [ts[j] for j in range(max(0, i - 2), min(len(ts), i + 3))]
    tts = tcap.ts_for(view, 'truth')
    cols = ['amend', 'head', 'two', 'truth']
    tw, th = 480, 270
    img = Image.new('RGB', (tw * len(sel) + 90, th * len(cols) + 24), 'white')
    d = ImageDraw.Draw(img)
    for x, t in enumerate(sel):
        d.text((90 + x * tw + 4, 4), f"t'={t:.3f}" + (' *' if x == i - max(0, i - 2) else ''), fill='black')
        for y, c in enumerate(cols):
            if c == 'truth' and not any(abs(t - k) < 1e-6 for k in tts):
                d.text((90 + x * tw + 8, 24 + y * th + th // 2), '(not a truth knot)', fill='black')
                continue
            im = Image.fromarray(np.asarray(Image.open(
                os.path.join(tcap.OLD_CAP, mcap.label(view, t, c) + '.png') if c == 'two'
                else tcap.path_for(view, t, c)).convert('RGB')))
            if gain != 1.0:
                im = im.point(lambda v: min(255, int(v * gain)))
            img.paste(im.resize((tw, th), Image.LANCZOS), (90 + x * tw, 24 + y * th))
    for y, c in enumerate(cols):
        d.text((4, 24 + y * th + th // 2), c + (f' x{gain:g}' if gain != 1.0 else ''), fill='black')
    p = os.path.join(SH, f'{view}-sheet-t{t_center:.3f}' + (f'-x{gain:g}' if gain != 1.0 else '') + '.png')
    img.save(p)
    return p


if __name__ == '__main__':
    for d in (RES, CH, SH):
        os.makedirs(d, exist_ok=True)
    views = sys.argv[1:] or list(mcap.VIEWS)
    for v in views:
        R = analyse(v)
        json.dump(R, open(os.path.join(RES, f'{v}.json'), 'w'), indent=1)
        print(luma_chart(v, R))
        print(step_chart(v, R))
    print(sheet('ceiling', 0.4, 6.0))
    print(sheet('east', 1 / 7, 1.0))
    print(sheet('east', 4 / 7, 1.0))
