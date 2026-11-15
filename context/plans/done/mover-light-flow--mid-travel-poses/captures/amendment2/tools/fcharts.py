# Phone-readable charts for task 25c.  python fcharts.py <out_dir>
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flib  # noqa: E402

OUT = sys.argv[1]
S = json.load(open(os.path.join(flib.F, 'results', 'fsummary.json'), encoding='utf-8'))
X = json.load(open(os.path.join(flib.F, 'results', 'fexit.json'), encoding='utf-8'))
INK, INK2, GRID = '#0b0b0b', '#52514e', '#e4e3df'
STY = {
    'head': ('#d93b3b', '-', 3.0, 'final build (dark + edge copy + ramp)'),
    'truth': ('#52514e', '--', 2.0, 'dense truth, final rule'),
    'odark': ('#8f3fd0', '-', 2.0, 'prototype dark + r40 (no edge copy)'),
    'ohead': ('#eda100', '-', 2.0, 'ramp alone (r40, pre-dark bake)'),
    'odtruth': ('#8f3fd0', ':', 2.0, 'dense truth, prototype dark rule'),
    'P:dark-r40': ('#8f3fd0', '-', 2.0, 'prototype dark + r40 (no edge copy)'),
    'P:dtruth-r40': ('#8f3fd0', ':', 2.0, 'dense truth, prototype dark rule'),
    'P:head-r40': ('#eda100', '-', 2.0, 'ramp alone (r40)'),
    'P:head-cur': ('#2a78d6', '-', 1.6, 'pre-amendment'),
    'P:truth-r40': ('#eda100', ':', 1.6, 'dense truth, pre-dark rule, r40'),
}
plt.rcParams.update({'font.size': 15, 'axes.edgecolor': INK2, 'axes.labelcolor': INK, 'xtick.color': INK2,
                     'ytick.color': INK2, 'axes.titleweight': 'bold', 'axes.titlesize': 16,
                     'figure.facecolor': '#fcfcfb', 'axes.facecolor': '#fcfcfb'})


def style(a):
    a.grid(color=GRID, lw=1); a.set_axisbelow(True)
    for s in ('top', 'right'):
        a.spines[s].set_visible(False)


def plot(a, ts, ys, col):
    c, ls, lw, lab = STY[col]
    y = np.array([np.nan if v is None else v for v in ys], float)
    a.plot(ts, y, color=c, ls=ls, lw=lw, label=lab)


def band_chart():
    trip = flib.acap.parse_ts('0:1:269')
    fig, axs = plt.subplots(3, 1, figsize=(9, 15))
    for a, key, title in ((axs[0], 'B/m02', 'West-wall band B, static indirect (mask 02)'),
                          (axs[1], 'B/mNone', 'West-wall band B, full frame')):
        for col in ('odark', 'ohead', 'truth', 'head'):
            if col in S['band'][key]:
                plot(a, trip, S['band'][key][col]['series'], col)
        a.set_title(title); a.set_xlabel("t′ (whole trip, one sample per 60 fps frame)"); a.set_ylabel('mean luma')
        style(a)
    axs[0].legend(fontsize=11, loc='upper left')
    a = axs[2]
    for col in ('odark', 'head'):
        st = S['band']['B/m02'][col]['steps']
        plot(a, trip, st, col)
    a.axhline(1.5, color=INK2, lw=1, ls='--')
    a.set_title('Band B (mask 02): largest 8×8-block step per frame'); a.set_xlabel('t′'); a.set_ylabel('luma / frame')
    style(a); a.legend(fontsize=11)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-final-band.png'), dpi=110); plt.close(fig)


def exit_chart(region='A'):
    rows = X[region]
    fig, axs = plt.subplots(5, 2, figsize=(11, 20), sharey=False)
    for k, r in enumerate(rows):
        a = axs[k // 2][k % 2]
        for col in ('ohead', 'odark', 'head'):
            v = r['cols'].get(col)
            if not v or 'means' not in v:
                continue
            plot(a, v['ts'], v['means'], col)
        for kt in (r['k0'], r['k1']):
            a.axvline(kt, color=INK2, lw=1, ls=':')
        a.set_title(f"{r['kind']} {r['k0']:.4f}–{r['k1']:.4f}", fontsize=13)
        a.tick_params(labelsize=10); style(a)
    axs[0][0].legend(fontsize=9, loc='best')
    fig.suptitle(f'Above-door band {region}: mean luma through each lintel crossing (dotted: geometry knots)', fontsize=14)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, f'a2-final-exits-{region}.png'), dpi=100); plt.close(fig)


def handover_chart():
    H = S['handover']
    keys = ['sweepfix/face', 'walledge/end', 'walledge/north', 'door_ceiling/A', 'door_ceiling/B']
    fig, axs = plt.subplots(len(keys), 1, figsize=(9, 4 * len(keys)))
    for a, k in zip(axs, keys):
        e = H[k]
        for col in ('P:head-cur', 'P:dark-r40', 'truth', 'head'):
            if col in e['cols']:
                plot(a, e['ts'], e['cols'][col]['steps'], col)
        a.set_title(e['label'] + ': largest block step per frame'); a.set_xlabel('t′'); a.set_ylabel('luma / frame')
        style(a)
    axs[0].legend(fontsize=10)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-final-handover.png'), dpi=100); plt.close(fig)


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    band_chart(); exit_chart('A'); handover_chart()
    print('charts written')
