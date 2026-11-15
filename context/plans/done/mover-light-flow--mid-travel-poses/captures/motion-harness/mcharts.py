# Charts and contact sheets from motion/results/<view>.json.
#   venv/bin/python -I mcharts.py
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from PIL import Image, ImageDraw
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcap

RES = os.path.join(mcap.M, 'results')
CH = os.path.join(mcap.M, 'charts')
SH = os.path.join(mcap.M, 'sheets')
KNOTS = json.load(open(os.path.join(RES, 'knots.json')))  # {view: [knots]}
GAIN = {'floor': 3.0, 'ceiling': 6.0, 'interior': 4.0}
EXTRA_SHEETS = {'ceiling': [0.4], 'floor': [0.6], 'interior': [0.2], 'west': [2 / 7], 'east': [1 / 7]}
COLORS = {'head': '#d1495b', 'nodepth': '#2e86ab', 'two': '#3b8b3b', 'strip': '#888888'}
NAMES = {'head': 'head (mid poses)', 'nodepth': 'head, id-57 depth stripped',
         'two': 'light_poses 2 (pre-brief blend)', 'strip': 'id 57 stripped (mover-absent)'}


def ticks(ax, view):
    k = KNOTS[view]
    for i, x in enumerate(k):
        ax.axvline(x, color='#444', lw=0.8, ls='-', alpha=0.5)
        if i + 1 < len(k):
            ax.axvline(0.5 * (x + k[i + 1]), color='#999', lw=0.6, ls=':', alpha=0.7)


def luma_chart(view, R):
    ts = R['ts']
    fig, axes = plt.subplots(2, 1, figsize=(11, 7.5), sharex=True,
                             gridspec_kw=dict(height_ratios=[3, 2]))
    ax = axes[0]
    for c in mcap.COLUMNS:
        ax.plot(ts, [r['crop_mean'][c] for r in R['rows']], marker='o', ms=2.5, lw=1.3,
                color=COLORS[c], label=NAMES[c])
    ticks(ax, view)
    ax.set_ylabel('crop mean luma (Rec. 709, 0-255)')
    ax.set_title(f'{view}: crop mean luma vs t\' (solid: knots, dotted: interval midpoints)')
    ax.legend(fontsize=8)
    ax = axes[1]
    for c, ref in (('two', 'head - two'), ('nodepth', 'head - nodepth'), ('strip', 'head - strip')):
        ax.plot(ts, [r['crop_mean']['head'] - r['crop_mean'][c] for r in R['rows']], marker='o', ms=2.5,
                lw=1.2, color=COLORS[c], label=ref)
    ax.axhline(0, color='k', lw=0.6)
    ticks(ax, view)
    ax.set_xlabel("t'")
    ax.set_ylabel('crop mean difference')
    ax.legend(fontsize=8)
    fig.tight_layout()
    p = os.path.join(CH, f'{view}-luma.png')
    fig.savefig(p, dpi=110)
    plt.close(fig)
    return p


def step_chart(view, R):
    fig, axes = plt.subplots(3, 1, figsize=(11, 9), sharex=True)
    for c in mcap.COLUMNS:
        s = R['steps'][c]
        mid = [0.5 * (x['t0'] + x['t1']) for x in s]
        axes[0].plot(mid, [x['max'] for x in s], marker='o', ms=2.5, lw=1.2, color=COLORS[c], label=NAMES[c])
        axes[1].plot(mid, [x['p95'] for x in s], marker='o', ms=2.5, lw=1.2, color=COLORS[c], label=NAMES[c])
        k = R['kinks'][c]
        axes[2].plot([x['t'] for x in k], [x['max'] for x in k], marker='o', ms=2.5, lw=1.2,
                     color=COLORS[c], label=NAMES[c])
    cell = 'face cell (8x8 u)' if view in ('west', 'east') else '8x8 block'
    axes[0].set_ylabel(f'max |dLuma| per {cell}')
    axes[1].set_ylabel('p95 |dLuma|')
    axes[2].set_ylabel('max |2nd difference|')
    axes[0].set_title(f"{view}: per-cell change between consecutive t' samples (plotted at the pair's midpoint)")
    for ax in axes:
        ticks(ax, view)
    axes[2].set_xlabel("t'")
    axes[0].legend(fontsize=8)
    fig.tight_layout()
    p = os.path.join(CH, f'{view}-steps.png')
    fig.savefig(p, dpi=110)
    plt.close(fig)
    return p


def sheet(view, R, t_center=None, gain=1.0):
    ts = R['ts']
    if t_center is None:
        s = R['steps']['head']
        i = int(np.argmax([x['max'] for x in s]))
        t_center = s[i]['t1']
    i = ts.index(t_center)
    sel = [ts[j] for j in range(max(0, i - 2), min(len(ts), i + 3))]
    cols = ['head', 'nodepth', 'two']
    tw, th = 480, 270
    img = Image.new('RGB', (tw * len(sel) + 90, th * len(cols) + 24), 'white')
    d = ImageDraw.Draw(img)
    for x, t in enumerate(sel):
        d.text((90 + x * tw + 4, 4), f"t'={t:.3f}" + (' *' if t == t_center else ''), fill='black')
        for y, c in enumerate(cols):
            im = Image.open(os.path.join(mcap.CAP, mcap.label(view, t, c) + '.png')).convert('RGB')
            if gain != 1.0:
                im = im.point(lambda v: min(255, int(v * gain)))
            img.paste(im.resize((tw, th), Image.LANCZOS), (90 + x * tw, 24 + y * th))
    for y, c in enumerate(cols):
        d.text((4, 24 + y * th + th // 2), c + (f' x{gain:g}' if gain != 1.0 else ''), fill='black')
    p = os.path.join(SH, f'{view}-sheet-t{t_center:.3f}' + (f'-x{gain:g}' if gain != 1.0 else '') + '.png')
    img.save(p)
    return p


if __name__ == '__main__':
    os.makedirs(CH, exist_ok=True)
    os.makedirs(SH, exist_ok=True)
    views = sys.argv[1:] or list(mcap.VIEWS)
    for v in views:
        R = json.load(open(os.path.join(RES, f'{v}.json')))
        print(luma_chart(v, R))
        print(step_chart(v, R))
        g = GAIN.get(v, 1.0)
        print(sheet(v, R, gain=g))
        for tc in EXTRA_SHEETS.get(v, []):
            print(sheet(v, R, t_center=min(R['ts'], key=lambda t: abs(t - tc)), gain=g))
