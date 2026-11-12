# Phone-readable charts for Ruling B option (ii).  python dcharts.py <out_dir>
import json, os, sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from PIL import Image, ImageDraw, ImageFont
HERE = os.path.dirname(os.path.abspath(__file__))
# Scratch root (ws/, cap/, darkb/): A2_ROOT, else the scratch layout <A>/darkb/tools.
A = os.environ.get('A2_ROOT') or os.path.dirname(os.path.dirname(HERE))
os.environ.setdefault('A2_ROOT', A)
os.environ.setdefault('A2_REGIONS', os.path.join(HERE, 'dregions.json'))
sys.path.insert(0, os.path.join(A, 'tools'))
sys.path.insert(0, HERE)  # acap.py / aseries.py sit beside these in the repo copy
sys.path.insert(0, HERE)
import dcap  # noqa: F401,E402
import acap, aseries  # noqa: E402

OUT = sys.argv[1]
S = json.load(open(os.path.join(A, 'darkb', 'results', 'dsummary.json'), encoding='utf-8'))
INK, INK2, GRID = '#0b0b0b', '#52514e', '#e4e3df'
STY = {
    'head/cur': ('#2a78d6', '-', 2.5, 'today'),
    'head/r40': ('#eda100', '-', 2.5, 'r40 alone (ruling A)'),
    'dark/cur': ('#8f3fd0', '-', 2.0, 'dark state, today\'s backface'),
    'dark/r40': ('#d93b3b', '-', 3.0, 'dark state + r40'),
    'truth/r40': ('#52514e', '--', 1.8, 'dense truth, today\'s rule, r40'),
    'dtruth/r40': ('#d93b3b', ':', 2.2, 'dense truth, dark rule, r40'),
    'shift/cur': ('#1baf7a', '-', 2.0, 'lattice shifted 32 u (option i), today'),
    'shift/r40': ('#1baf7a', '--', 2.0, 'lattice shifted 32 u + r40'),
}
plt.rcParams.update({'font.size': 15, 'axes.edgecolor': INK2, 'axes.labelcolor': INK, 'xtick.color': INK2,
                     'ytick.color': INK2, 'axes.titleweight': 'bold', 'axes.titlesize': 16,
                     'figure.facecolor': '#fcfcfb', 'axes.facecolor': '#fcfcfb'})


def style(a):
    a.grid(color=GRID, lw=1); a.set_axisbelow(True)
    for s in ('top', 'right'):
        a.spines[s].set_visible(False)


def plot(ax, ts, cols, keys, field='series'):
    for k in keys:
        if k in cols:
            c, ls, lw, lab = STY[k]
            y = [np.nan if v is None else v for v in cols[k][field]]
            ax.plot(ts[-len(y):], y, color=c, ls=ls, lw=lw, label=lab)


# 1. The west ceiling band (the Ruling B chart).
b = S['band02']; ts = np.array(b['ts'])
full = S['door_ceiling/B/0:1:101']
fig, (a1, a2) = plt.subplots(2, 1, figsize=(9, 11), sharex=True)
plot(a1, ts, b['cols'], ['head/cur', 'head/r40', 'dark/cur', 'dark/r40', 'shift/r40', 'dtruth/r40', 'truth/r40'])
a1.set_ylabel('Band luma, static indirect only')
a1.set_title('Ruling B (ii) prototyped: west-wall ceiling band\n(segment B, light-term mask 0x02)', loc='left')
a1.legend(fontsize=10.5, frameon=False, loc='upper left')
plot(a2, np.array(full['ts']), full['cols'], ['head/cur', 'head/r40', 'dark/r40', 'dtruth/r40', 'truth/r40'])
a2.set_ylabel('Band luma, full frame')
a2.legend(fontsize=10.5, frameon=False, loc='upper right')
a2.annotate('termini read the base:\nthe band re-lights\ninto rest', xy=(0.985, 5.0), xytext=(0.62, 9.5), fontsize=11,
            arrowprops=dict(arrowstyle='->', color=INK2), color=INK2)
a2.set_title('The same band, every light, whole trip', loc='left')
a2.set_xlabel('t′ (room travel fraction)')
for a in (a1, a2):
    style(a)
fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-ruling-b-dark-band.png'), dpi=90); plt.close(fig)

# 2. Sweeper faces: mean and per-frame steps.
fig, axes = plt.subplots(4, 1, figsize=(9, 15), sharex=True, gridspec_kw=dict(height_ratios=[1.2, 0.8, 1.2, 0.8]))
for i, (view, name) in enumerate((('sweepfix', 'east'), ('sweepwest', 'west'))):
    s = S[f'{view}/face/0:0.5:129']; t = np.array(s['ts'])
    plot(axes[2 * i], t, s['cols'], ['head/cur', 'head/r40', 'dark/r40', 'dtruth/r40', 'truth/r40'])
    plot(axes[2 * i + 1], t, s['cols'], ['head/cur', 'head/r40', 'dark/r40', 'dtruth/r40'], 'steps')
    axes[2 * i].set_ylabel('Face mean luma'); axes[2 * i + 1].set_ylabel('Largest block step\nper 60 fps frame')
    axes[2 * i].set_title(f'em_sweeper {name} face, first half of the trip', loc='left')
    axes[2 * i + 1].axhline(3, color=INK2, lw=1, ls=':')
axes[0].legend(fontsize=10.5, frameon=False, loc='lower left')
axes[-1].set_xlabel('t′ (sweeper travel fraction)')
for a in axes:
    style(a)
fig.suptitle('Ruling B (ii) prototyped: sweeper faces', x=0.02, ha='left', fontsize=17, fontweight='bold')
fig.tight_layout(rect=(0, 0, 1, 0.97)); fig.savefig(os.path.join(OUT, 'a2-ruling-b-dark-sweeper.png'), dpi=90); plt.close(fig)

# 3. Dark blotches: the most-darkened 8x8 block per view (dark - today, same form), and vs the dark-rule truth.
views = [('door_ceiling/B/0.1:0.6:135', 'Ceiling band B'), ('door_ceiling/A/0.1:0.6:135', 'Ceiling above door'),
         ('sweepfix/face/0:0.5:129', 'Sweeper east face'), ('sweepwest/face/0:0.5:129', 'Sweeper west face'),
         ('east/frame/0:1:101', 'Sweeper, fixed cam east'), ('west/frame/0:1:101', 'Sweeper, fixed cam west'),
         ('walledge/end/0.1:0.45:146', 'Wall end face'), ('walledge/north/0.1:0.45:146', 'Wall north face'),
         ([k for k in S if k.startswith('floor/frame/')][0], 'Room floor view')]
fig, ax = plt.subplots(figsize=(9, 11))
y = np.arange(len(views))[::-1] * 1.2
get = lambda k, col, ref: -((S[k]['cols'].get(col, {}).get(ref)) or {'min': np.nan})['min']
v1 = [get(k, 'dark/r40', 'vs_head') for k, _ in views]
v2 = [get(k, 'dark/r40', 'vs_dtruth') for k, _ in views]
v3 = [get(k, 'head/r40', 'vs_truth') for k, _ in views]
for off, vals, c, lab in ((0.32, v1, '#d93b3b', 'dark+r40 darker than r40 alone'),
                          (0.0, v2, '#8f3fd0', 'dark+r40 darker than its dense truth (dark rule)'),
                          (-0.32, v3, '#9a9893', 'r40 alone darker than its dense truth (today\'s own miss)')):
    ax.barh(y + off, vals, height=0.3, color=c, label=lab)
    for yi, v in zip(y, vals):
        if not np.isnan(v):
            ax.text(v + 0.2, yi + off, f'{v:.1f}', va='center', fontsize=11)
ax.set_yticks(y); ax.set_yticklabels([n for _, n in views], fontsize=13)
ax.axvline(3, color=INK2, lw=1, ls=':')
ax.set_xlabel('Worst 8×8-block darkening over the series (luma)')
ax.set_title('Ruling B (ii): worst darkened block per view\n(ceiling rows: the band going away, intended)', loc='left')
ax.legend(fontsize=11, frameon=False, loc='upper center', bbox_to_anchor=(0.4, -0.07), ncol=1); style(ax)
fig.tight_layout(); fig.savefig(os.path.join(OUT, 'a2-ruling-b-dark-blotch.png'), dpi=90); plt.close(fig)

# 4. Contact sheet at each view's most-darkened t' (dark vs today's build, same backface form).
try:
    FONT = ImageFont.truetype('arialbd.ttf', 22); SMALL = ImageFont.truetype('arial.ttf', 18)
except OSError:
    FONT = SMALL = ImageFont.load_default()
sheet_views = [('sweepfix/face/0:0.5:129', 'Sweeper east face'), ('sweepwest/face/0:0.5:129', 'Sweeper west face'),
               ('east/frame/0:1:101', 'Sweeper, fixed cam east'), ('west/frame/0:1:101', 'Sweeper, fixed cam west'),
               ('walledge/end/0.1:0.45:146', 'Wall end + north'), ('door_ceiling/B/0.1:0.6:135', 'Ceiling over the door')]
rows = []
for k, name in sheet_views:
    for form, fname in (('cur', "today's backface"), ('r40', 'r40')):
        w = S[k]['cols'].get(f'dark/{form}', {}).get('vs_head')
        if w:
            rows.append((k.split('/')[0], form, w['t'], f'{name}\n{fname}', w['min']))
cw, ch, lw, th = 300, 169, 270, 104
W, H = lw + cw * 4, th + ch * len(rows)
sh = Image.new('RGB', (W, H), (252, 252, 251)); d = ImageDraw.Draw(sh)
d.text((12, 10), 'Ruling B (ii): each view at its most-darkened t′, dark state vs today\'s bake (same backface form)', font=FONT, fill=(11, 11, 11))
d.text((12, 42), 'Last column: dark minus today, x8; red = darker, green = brighter. Worst 8x8 block given at left.', font=SMALL, fill=(82, 81, 78))
for j, c in enumerate(['today\'s bake', 'dark state', 'dense truth, dark rule', 'difference x8']):
    d.text((lw + j * cw + 6, 76), c, font=SMALL, fill=(11, 11, 11))
for i, (view, form, t, name, lo) in enumerate(rows):
    d.text((8, th + i * ch + 40), f'{name}\nt′ {t:.4f}\nworst {lo:+.1f}', font=SMALL, fill=(11, 11, 11))
    ims = []
    for j, bake in enumerate(('head', 'dark', 'dtruth')):
        p = os.path.join(acap.CAP, acap.label(view, t, bake, form) + '.png')
        im = Image.open(p).convert('RGB') if os.path.exists(p) else None
        ims.append(im)
        if im is not None:
            sh.paste(im.resize((cw - 4, ch - 4)), (lw + j * cw + 2, th + i * ch + 2))
    if ims[0] is not None and ims[1] is not None:
        a = np.asarray(ims[0], dtype=np.float32).mean(axis=2); b2 = np.asarray(ims[1], dtype=np.float32).mean(axis=2)
        dd = (b2 - a) * 8
        rgb = np.zeros(a.shape + (3,), np.uint8)
        rgb[..., 0] = np.clip(-dd, 0, 255); rgb[..., 1] = np.clip(dd, 0, 255)
        sh.paste(Image.fromarray(rgb).resize((cw - 4, ch - 4)), (lw + 3 * cw + 2, th + i * ch + 2))
sh.save(os.path.join(OUT, 'a2-ruling-b-dark-sheet.png'), optimize=True)
print('charts written')
