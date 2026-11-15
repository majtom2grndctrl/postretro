# Contact sheets for captures/amendment2/.
import os, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFont
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, aseries, afloor
OUT = sys.argv[1]
try:
    FONT = ImageFont.truetype('arialbd.ttf', 22); SMALL = ImageFont.truetype('arial.ttf', 18)
except OSError:
    FONT = SMALL = ImageFont.load_default()
BG, INK = (252, 252, 251), (11, 11, 11)

def img(view, t, bake, form, mask=None, attr=None):
    return Image.open(os.path.join(acap.CAP, acap.label(view, t, bake, form, mask, attr) + '.png')).convert('RGB')

def sheet(title, rows, cols, cells, cw, ch, path, note=''):
    lw, th = 230, 70 + (28 if note else 0)
    W, H = lw + cw * len(cols), th + 34 + ch * len(rows)
    S = Image.new('RGB', (W, H), BG); d = ImageDraw.Draw(S)
    d.text((12, 10), title, font=FONT, fill=INK)
    if note: d.text((12, 42), note, font=SMALL, fill=(82, 81, 78))
    for j, c in enumerate(cols): d.text((lw + j * cw + 8, th + 6), c, font=SMALL, fill=INK)
    for i, r in enumerate(rows):
        d.text((10, th + 34 + i * ch + ch // 2 - 10), r, font=SMALL, fill=INK)
        for j in range(len(cols)):
            im = cells(i, j)
            if im is not None: S.paste(im.resize((cw - 4, ch - 4)), (lw + j * cw + 2, th + 34 + i * ch + 2))
    S.save(path, optimize=True)

# 1. Ceiling over the door at band peaks, full frame, top 60%.
forms = [('head', 'cur', 'today'), ('head', 'o40', 'cosine, 0.4 offset'), ('head', 'r40', 'ramp, 0.4'), ('shift', 'cur', 'lattice shifted, today')]
ts = [0.24, 0.46, 0.68]
crop = lambda im: Image.fromarray(np.clip(np.asarray(im.crop((0, 60, 960, 420)), dtype=np.float32) * 4, 0, 255).astype(np.uint8))
sheet('Ruling B: ceiling over the door at three band peaks (static indirect only, brightened 4x)', [f't\u2032 {t}' for t in ts], [f[2] for f in forms],
      lambda i, j: crop(img('door_ceiling', ts[i], forms[j][0], forms[j][1], '02')),
      300, 115, os.path.join(OUT, 'a2-sheet-ceiling.png'), note='Rising room, camera rides the room; the band is the glow along the west-wall/ceiling edge.')

# 2. Floor excess maps (task 12c metric) at t 0.8 / 0.9 per form, plus id 57 stripped.
cols = [('head', 'cur', 'today'), ('head', 'r10', 'ramp 0.1'), ('head', 'r25', 'ramp 0.25'), ('head', 'r40', 'ramp 0.4'), ('head', 'o40', 'cosine 0.4'), ('strip', 'cur', 'id 57 stripped')]
ts = [0.8, 0.9, 0.95]
def exc(i, j):
    t = ts[i]; b, f, _ = cols[j]
    m = afloor.crop(t)
    L = aseries.load('floor', t, b, f)
    bm = L[:536].reshape(67, 8, 120, 8).mean(axis=(1, 3))
    _, pk, ex = afloor.excess(bm, m)
    v = np.where(m, np.clip(ex / 8.0, 0, 1), 0)
    a = (v * 255).astype(np.uint8)
    rgb = np.stack([a, a, a], -1); rgb[~m] = (60, 60, 90)
    im = Image.fromarray(rgb).resize((240, 134), Image.NEAREST)
    ImageDraw.Draw(im).text((4, 2), f'{np.nanmax(ex[m]):.1f} / {pk}', font=SMALL, fill=(235, 104, 52))
    return im
sheet('Eighth-power redo: rising-room floor excess (black 0, white +8 levels)', [f't\u2032 {t}' for t in ts], [c[2] for c in cols], exc,
      200, 112, os.path.join(OUT, 'a2-sheet-floor-excess.png'), note='Orange: max excess / local peaks over 3 levels (task 12c metric). Lattice dots would show as a regular grid.')

# 3. Sweeper east face: change since the previous 60 fps frame, amplified.
ts = [round(i / 256, 5) for i in range(18, 33, 3)]
rows = [('head', 'cur', 'today'), ('head', 'r25', 'ramp 0.25'), ('head', 'r40', 'ramp 0.4')]
def dif(i, j):
    b, f, _ = rows[i]
    a = np.asarray(img('sweepfix', ts[j], b, f).crop((0, 148, 960, 392)), dtype=np.float32)
    p = np.asarray(img('sweepfix', round(ts[j] - 3 / 256, 5), b, f).crop((0, 148, 960, 392)), dtype=np.float32)
    d = np.clip(np.abs(a - p).mean(axis=2) * 12, 0, 255).astype(np.uint8)
    return Image.fromarray(np.stack([d, d, d], -1))
sheet('Ruling A: sweeper east face, change since the previous 60 fps frame', [r[2] for r in rows], [f't′ {t:.3f}' for t in ts],
      dif, 220, 70, os.path.join(OUT, 'a2-sheet-sweeper-crossing.png'),
      note='Per-pixel |frame - previous frame| x12 (white = 21 levels); the face crosses the x 1339.3 layer here.')
print('sheets written')
