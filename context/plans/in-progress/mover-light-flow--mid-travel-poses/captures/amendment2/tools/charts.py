# Phone-readable charts for captures/amendment2/.
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, aseries

OUT = sys.argv[1]
A = acap.A
S = json.load(open(os.path.join(A, 'results', 'summary.json'), encoding='utf-8'))
COL = {'cur': '#2a78d6', 'r10': '#eb6834', 'r25': '#1baf7a', 'r40': '#eda100', 'o40': '#e87ba4', 'shift': '#6250d6'}
NAME = {'cur': 'today (cosine, 0.1 cell)', 'r10': 'surface ramp, 0.1 cell', 'r25': 'surface ramp, 0.25 cell',
        'r40': 'surface ramp, 0.4 cell', 'o40': 'cosine, 0.4 cell offset only', 'shift': 'lattice shifted 32 u'}
INK, INK2, GRID = '#0b0b0b', '#52514e', '#e4e3df'
plt.rcParams.update({'font.size': 15, 'axes.edgecolor': INK2, 'axes.labelcolor': INK, 'xtick.color': INK2,
                     'ytick.color': INK2, 'axes.titleweight': 'bold', 'axes.titlesize': 16,
                     'figure.facecolor': '#fcfcfb', 'axes.facecolor': '#fcfcfb'})


def style(ax):
    ax.grid(axis='x', color=GRID, lw=1); ax.set_axisbelow(True)
    for s in ('top', 'right'): ax.spines[s].set_visible(False)


# 1. Ruling A: largest per-frame step per view and form (head bake).
views = ['sweepfix/face', 'walledge/end', 'walledge/north', 'door_ceiling/A', 'door_ceiling/B']
forms = ['cur', 'r10', 'r25', 'r40', 'o40']
fig, axes = plt.subplots(len(views), 1, figsize=(9, 3.0 * len(views) + 1.0), sharex=False)
for ax, v in zip(axes, views):
    cols = S[v]['cols']
    vals = [cols.get(f'head/{f}', {}).get('step_max', np.nan) for f in forms]
    y = np.arange(len(forms))[::-1]
    ax.barh(y, vals, color=[COL[f] for f in forms], height=0.7, edgecolor='#fcfcfb', linewidth=2)
    for yi, val, f in zip(y, vals, forms):
        n = cols.get(f'head/{f}', {}).get('steps_over3', 0)
        ax.text(val + 0.2, yi, f'{val:.1f}  ({n} frames > 3)', va='center', fontsize=13, color=INK)
    ax.set_yticks(y); ax.set_yticklabels([NAME[f] for f in forms], fontsize=12)
    ax.set_title(S[v]['label'], loc='left'); ax.set_xlim(0, max(vals) * 1.55); style(ax)
axes[-1].set_xlabel('Largest 8×8-block change per frame (luma, 0–255)')
fig.suptitle('Ruling A: how hard the mover face steps\nbetween 60 fps frames (head bake; lower is smoother)',
             x=0.02, ha='left', fontsize=17, fontweight='bold')
fig.tight_layout(rect=(0, 0, 1, 0.96))
fig.savefig(os.path.join(OUT, 'a2-ruling-a-steps.png'), dpi=90); plt.close(fig)

# 2. Ruling A: sweeper face over travel, today vs ramp 0.4, with truth.
v = 'sweepfix/face'; ts = np.array(S[v]['ts'])
fig, (a1, a2) = plt.subplots(2, 1, figsize=(9, 9), sharex=True, gridspec_kw=dict(height_ratios=[1.3, 1]))
for f in ('cur', 'r40'):
    a1.plot(ts, S[v]['cols'][f'head/{f}']['series'], color=COL[f], lw=2.5, label=f'{NAME[f]}, head')
    tr = S[v]['cols'].get(f'truth/{f}')
    if tr: a1.plot(ts, tr['series'], color=COL[f], lw=1.5, ls='--', label=f'{NAME[f]}, dense truth')
    a2.plot(ts[S[v]['lag']:], S[v]['cols'][f'head/{f}']['steps'], color=COL[f], lw=2, label=NAME[f])
a2.axhline(3, color=INK2, lw=1, ls=':'); a2.text(0.005, 3.2, '3 levels', color=INK2, fontsize=12)
a1.set_ylabel('Face mean luma'); a2.set_ylabel('Largest block step\nper 60 fps frame')
a2.set_xlabel('t′ (sweeper travel fraction)')
a1.legend(fontsize=11, loc='lower left', frameon=False)
for a in (a1, a2):
    a.grid(color=GRID, lw=1); a.set_axisbelow(True)
    for s in ('top', 'right'): a.spines[s].set_visible(False)
a1.set_title('Ruling A: em_sweeper east face, first half of the trip', loc='left')
fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-ruling-a-sweeper-series.png'), dpi=90); plt.close(fig)

# 3. Ruling B: ceiling band and its split.
B = json.load(open(os.path.join(A, 'results', 'split.json'), encoding='utf-8'))
fig, (a1, a2) = plt.subplots(2, 1, figsize=(9, 10), sharex=True)
bt = np.array(B['ts'])
for key, f, label in (('head/cur', 'cur', NAME['cur']), ('head/o40', 'o40', NAME['o40']),
                      ('head/r40', 'r40', NAME['r40']), ('shift/cur', 'shift', NAME['shift'])):
    a1.plot(bt, B['band'][key], color=COL[f], lw=2.5, label=label)
a1.plot(bt, B['dark_est']['cur'], color='#008300', lw=2, ls='--', label='dark value state for enclosed mid poses (est.)')
a1.set_ylabel('Band luma (indirect only)'); a1.legend(fontsize=10, frameon=False, loc='upper left', ncol=1)
a1.set_title('Ruling B: the west-wall ceiling band\n(segment B, static indirect only)', loc='left')
for cls, c, label in (('wenc', '#e34948', 'x 846.93 column, inside the wall/roof'),
                      ('wabove', '#eb6834', 'x 846.93 column, above the roof'),
                      ('xabove', '#1baf7a', 'room-interior x, above the roof')):
    a2.plot(bt, np.array(B['share']['cur'][cls]) * 100, color=c, lw=2.5, label=label)
    a2.plot(bt, np.array(B['share']['o40'][cls]) * 100, color=c, lw=1.5, ls='--')
a2.set_ylabel('Share of band light (%)'); a2.set_xlabel('t′ (room travel fraction)')
a2.legend(fontsize=11, frameon=False, loc='center left', bbox_to_anchor=(0, 0.45), title='solid: today; dashed: 0.4 cell offset', title_fontsize=11)
for a in (a1, a2):
    a.grid(color=GRID, lw=1); a.set_axisbelow(True)
    for s in ('top', 'right'): a.spines[s].set_visible(False)
fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-ruling-b-band-split.png'), dpi=90); plt.close(fig)
print('charts written')
