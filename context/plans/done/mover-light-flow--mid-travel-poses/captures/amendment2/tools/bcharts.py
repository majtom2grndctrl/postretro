# Phone-readable charts for the big-room re-capture (2026-11-13).  python bcharts.py <out_dir>
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import blib  # noqa: E402

OUT = sys.argv[1]
S = json.load(open(os.path.join(blib.A3, 'results', 'bsummary.json'), encoding='utf-8'))
X = json.load(open(os.path.join(blib.A3, 'results', 'bexit.json'), encoding='utf-8'))
INK, INK2, GRID = '#0b0b0b', '#52514e', '#e4e3df'
STY = {
    'head': ('#d93b3b', '-', 2.6, 'HEAD (dark state + edge copy + ramp)'),
    'truth': ('#52514e', '--', 1.8, 'dense truth, same rule (room_big 98)'),
    'ramp': ('#eda100', '-', 1.8, 'ramp alone (r40, pre-dark bake)'),
}
plt.rcParams.update({'font.size': 14, 'axes.edgecolor': INK2, 'axes.labelcolor': INK, 'xtick.color': INK2,
                     'ytick.color': INK2, 'axes.titleweight': 'bold', 'axes.titlesize': 15,
                     'figure.facecolor': '#fcfcfb', 'axes.facecolor': '#fcfcfb'})
TRIP = blib.trip_ts()


def style(a):
    a.grid(color=GRID, lw=1); a.set_axisbelow(True)
    for s in ('top', 'right'):
        a.spines[s].set_visible(False)


def plot(a, ts, ys, col, **kw):
    c, ls, lw, lab = STY[col]
    y = np.array([np.nan if v is None else v for v in ys], float)
    a.plot(ts, y, color=c, ls=ls, lw=lw, label=kw.get('label', lab))


def band_chart():
    keys = [('bw/W/m02', 'Ceiling strip over the west wall (mask 02)'),
            ('bs/S/m02', 'Ceiling strip over the south wall (mask 02)'),
            ('bs/AF/m02', 'South wall face above the arch (mask 02)')]
    fig, axs = plt.subplots(len(keys) + 1, 1, figsize=(9, 5 * (len(keys) + 1)))
    for a, (k, title) in zip(axs, keys):
        for col in ('ramp', 'truth', 'head'):
            if col in S['band'][k]:
                plot(a, TRIP, S['band'][k][col]['series'], col)
        a.set_title(title); a.set_xlabel("t′ (whole trip, one sample per 60 fps frame)"); a.set_ylabel('mean luma')
        style(a)
    axs[0].legend(fontsize=11, loc='upper left')
    a = axs[-1]
    for col in ('ramp', 'head'):
        plot(a, TRIP, S['band']['bs/S/m02'][col]['steps'], col)
    a.axhline(1.5, color=INK2, lw=1, ls='--')
    a.set_title('South strip (mask 02): largest 8×8-block step per frame'); a.set_xlabel('t′'); a.set_ylabel('luma / frame')
    style(a); a.legend(fontsize=11)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a3-band.png'), dpi=100); plt.close(fig)


def exit_chart():
    # (plane, view/region, crossing t'): the hand-over fail, the deepest literal dips, one in-wall exit
    picks = [('ceiling', 'bs/A', 0.76864), ('arch x 1757.7', 'bs/AF', 0.91509), ('arch x 1718.3', 'bs/AF', 0.44980),
             ('floor top', 'bfloor/floor', 0.77790), ('roof top', 'broof/topS', 0.24386), ('ceiling', 'bs/S', 0.33441)]
    fig, axs = plt.subplots(3, 2, figsize=(11, 13))
    for a, (plane, key, tc) in zip(axs.flat, picks):
        rows = [r for r in X if r['plane'] == plane and abs(r['t'] - tc) < 1e-4 and 'means' in r['res'][key]['head']]
        if not rows:
            continue
        r = rows[0]
        for col in ('ramp', 'head'):
            v = r['res'][key][col]
            plot(a, v['ts'], v['means'], col)
        for kt in (r['k0'], r['k1']):
            a.axvline(kt, color=INK2, lw=1, ls=':')
        a.set_title(f"{r['kind']} {plane}, z {r['layer']:.1f}\n{key}, knots {r['k0']:.4f}–{r['k1']:.4f}", fontsize=12)
        a.set_xlabel("t′"); a.set_ylabel('mean luma'); style(a)
        a.ticklabel_format(axis='x', useOffset=False)
    axs[0][0].legend(fontsize=10)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a3-exits.png'), dpi=100); plt.close(fig)


def handover_chart():
    keys = [('bs/ceil', TRIP), ('walledge/end', blib.acap.parse_ts('0.1:0.45:146')),
            ('door_ceiling/A', blib.acap.parse_ts('0.1:0.6:135'))]
    fig, axs = plt.subplots(len(keys), 1, figsize=(9, 5 * len(keys)))
    for a, (k, ts) in zip(axs, keys):
        h = S['handover'][k]
        for col in ('ramp', 'truth', 'head'):
            if col in h['cols']:
                plot(a, ts, h['cols'][col]['steps'], col)
        a.set_title(h['label'] + ': step per frame'); a.set_xlabel('t′'); a.set_ylabel('largest block step')
        style(a)
    axs[0].legend(fontsize=11)
    fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a3-handover.png'), dpi=100); plt.close(fig)


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    band_chart(); exit_chart(); handover_chart()
    print('charts written to', OUT)
