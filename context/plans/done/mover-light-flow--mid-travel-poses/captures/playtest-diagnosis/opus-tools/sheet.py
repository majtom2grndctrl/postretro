# Crop sheet: rows = columns, cols = t; chroma gain g, luma gain lg.
#   python -I sheet.py view cols t,t,... x0 y0 x1 y1 out.png [chroma_gain] [luma_gain]
import os, sys
import numpy as np
from PIL import Image, ImageDraw
O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import label, parse_ts
view, cols, ts = sys.argv[1], sys.argv[2].split(','), parse_ts(sys.argv[3])
x0, y0, x1, y1 = map(int, sys.argv[4:8]); out = sys.argv[8]
g = float(sys.argv[9]) if len(sys.argv) > 9 else 1.0
lg = float(sys.argv[10]) if len(sys.argv) > 10 else 1.0
w, h = x1 - x0, y1 - y0
S = Image.new('RGB', (w * len(ts), (h + 14) * len(cols)), (255, 255, 255))
d = ImageDraw.Draw(S)
for r, c in enumerate(cols):
    for i, t in enumerate(ts):
        a = np.asarray(Image.open(os.path.join(O, 'cap', label(view, t, c) + '.png')).convert('RGB'), dtype=np.float64)[y0:y1, x0:x1]
        L = (0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2])[..., None]
        b = np.clip(L * lg + (a - L) * g, 0, 255).astype(np.uint8)
        S.paste(Image.fromarray(b), (i * w, r * (h + 14) + 14))
        d.text((i * w + 2, r * (h + 14)), f'{c} {t:.3f}', fill=(0, 0, 0))
S.save(out)
