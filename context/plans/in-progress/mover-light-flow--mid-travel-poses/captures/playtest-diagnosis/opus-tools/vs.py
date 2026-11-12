# Per-t′ block error of columns against a reference column (16x16 px blocks,
# luma), plus each frame's global mean and the worst block's position.
#   python -I vs.py view ref cols lo:hi:step [x0 y0 x1 y1]
import os, sys
import numpy as np
from PIL import Image
O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import label, parse_ts

B = 16


def lumab(view, t, col, roi):
    a = np.asarray(Image.open(os.path.join(O, 'cap', label(view, t, col) + '.png')).convert('RGB'), dtype=np.float64)
    x0, y0, x1, y1 = roi
    L = (0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2])[y0:y1, x0:x1]
    h, w = L.shape[0] // B * B, L.shape[1] // B * B
    return L[:h, :w].reshape(h // B, B, w // B, B).mean(axis=(1, 3))


if __name__ == '__main__':
    view, ref, cols, grid = sys.argv[1], sys.argv[2], sys.argv[3].split(','), sys.argv[4]
    roi = tuple(map(int, sys.argv[5:9])) if len(sys.argv) > 8 else (0, 0, 960, 540)
    ts = parse_ts(grid)
    print('t       ' + ' '.join(f'{c:>22s}' for c in cols) + '   (mean|d|, max|d| @blk x,y)')
    agg = {c: [] for c in cols}
    for t in ts:
        r = lumab(view, t, ref, roi)
        row = []
        for c in cols:
            d = lumab(view, t, c, roi) - r
            j = np.unravel_index(np.abs(d).argmax(), d.shape)
            agg[c].append((np.abs(d).mean(), d[j]))
            row.append(f'{np.abs(d).mean():5.2f} {d[j]:+6.1f}@{j[1] * B + roi[0]:3d},{j[0] * B + roi[1]:3d}')
        print(f'{t:.4f} ' + ' '.join(row))
    for c in cols:
        m = np.array([a for a, _ in agg[c]]); w = np.array([abs(b) for _, b in agg[c]])
        print(f'{c}: mean|d| {m.mean():.2f}, worst block {w.max():.1f} at t {ts[int(w.argmax())]}')
