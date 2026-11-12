# Temporal step analysis over a fixed ROI: luma and chroma (R-L, B-L) block
# means; per step, mean |delta|; concentration = share of total variation
# carried by the largest 10% of steps.
#   python -I steps.py <view> <cols> <lo:hi:step> x0 y0 x1 y1
import os, sys
import numpy as np
from PIL import Image

O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import parse_ts, label  # noqa


def blocks(x, b=8):
    h, w = x.shape[0] // b * b, x.shape[1] // b * b
    return x[:h, :w].reshape(h // b, b, w // b, b, *x.shape[2:]).mean(axis=(1, 3))


def frame(view, t, col, roi):
    x0, y0, x1, y1 = roi
    a = np.asarray(Image.open(os.path.join(O, 'cap', label(view, t, col) + '.png')).convert('RGB'),
                   dtype=np.float64)[y0:y1, x0:x1]
    L = 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
    C = np.stack([a[..., 0] - L, a[..., 2] - L], -1)
    return blocks(L), blocks(C)


def analyse(view, col, ts, roi):
    Ls, Cs = zip(*(frame(view, t, col, roi) for t in ts))
    Ls, Cs = np.array(Ls), np.array(Cs)
    dL = np.abs(np.diff(Ls, axis=0)).mean(axis=(1, 2))
    dC = np.linalg.norm(np.diff(Cs, axis=0), axis=-1).mean(axis=(1, 2))
    cmag = np.linalg.norm(Cs, axis=-1).mean(axis=(1, 2))
    return dL, dC, cmag


def conc(d):
    k = max(1, len(d) // 10)
    return np.sort(d)[-k:].sum() / d.sum()


if __name__ == '__main__':
    view, cols, grid = sys.argv[1], sys.argv[2].split(','), sys.argv[3]
    roi = tuple(map(int, sys.argv[4:8]))
    ts = parse_ts(grid)
    res = {c: analyse(view, c, ts, roi) for c in cols}
    print('col      sum|dL|  top10%share  max dL | sum|dC| top10%share max dC | mean |C|')
    for c, (dL, dC, cm) in res.items():
        print(f'{c:8s} {dL.sum():7.2f}  {conc(dL):5.2f}  {dL.max():6.2f} | {dC.sum():7.2f} {conc(dC):5.2f} {dC.max():6.2f} | {cm.mean():5.2f}')
    if os.environ.get('VERBOSE'):
        print('t_mid ' + ' '.join(f'{c}:dL/dC' for c in cols))
        for i in range(len(ts) - 1):
            print(f'{(ts[i] + ts[i + 1]) / 2:.4f} ' + ' '.join(
                f'{res[c][0][i]:5.2f}/{res[c][1][i]:5.2f}' for c in cols))
