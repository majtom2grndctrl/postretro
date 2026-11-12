# Ceiling-edge band luma per t′ for the door_ceiling view, two segments:
# A above the door, B above the solid west wall.
#   python -I band.py cols lo:hi:step [out.json]
import json, os, sys
import numpy as np
from PIL import Image
O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import label, parse_ts

H, W = 540, 960
yy, xx = np.mgrid[0:H, 0:W]
line = 140 + (xx - 40) * (218.0 / 690.0)
band = (yy >= line - 45) & (yy <= line - 3)
SEG = {'A': band & (xx >= 380) & (xx <= 600), 'B': band & (xx >= 80) & (xx <= 300)}


def luma(view, t, col):
    a = np.asarray(Image.open(os.path.join(O, 'cap', label(view, t, col) + '.png')).convert('RGB'), dtype=np.float64)
    return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]


if __name__ == '__main__':
    cols = sys.argv[1].split(','); ts = parse_ts(sys.argv[2])
    view = os.environ.get('VIEW', 'door_ceiling')
    out = {c: {s: [] for s in SEG} for c in cols}
    for c in cols:
        for t in ts:
            L = luma(view, t, c)
            for s, m in SEG.items():
                out[c][s].append(float(L[m].mean()))
    if len(sys.argv) > 3:
        json.dump(dict(ts=ts, data=out), open(sys.argv[3], 'w'))
    print('t      ' + ' '.join(f'{c[:6]:>6s}A {c[:6]:>6s}B' for c in cols))
    for i, t in enumerate(ts):
        print(f'{t:.4f} ' + ' '.join(f'{out[c]["A"][i]:7.2f} {out[c]["B"][i]:7.2f}' for c in cols))
