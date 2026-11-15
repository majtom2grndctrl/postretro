# Per-frame series over a fixed screen ROI: luma mean, chroma (colour noise),
# frame-to-frame mean |delta| (per 8x8 block), per column.
#   python -I series.py <view> <cols> <lo:hi:step> x0 y0 x1 y1 [out.json]
import glob, json, os, sys
import numpy as np
from PIL import Image

O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
sys.path.insert(0, O + '/tools')
from cap import parse_ts, label  # noqa


def load(view, t, col):
    p = os.path.join(O, 'cap', label(view, t, col) + '.png')
    return np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)


def stats(a):
    luma = 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
    # chroma: distance of rgb from its own grey
    chroma = np.sqrt(((a - luma[..., None]) ** 2).mean(axis=-1))
    return luma, chroma


def blocks(x, b=8):
    h, w = x.shape[0] // b * b, x.shape[1] // b * b
    return x[:h, :w].reshape(h // b, b, w // b, b).mean(axis=(1, 3))


if __name__ == '__main__':
    view, cols, grid = sys.argv[1], sys.argv[2].split(','), sys.argv[3]
    x0, y0, x1, y1 = map(int, sys.argv[4:8])
    ts = parse_ts(grid)
    out = {}
    for col in cols:
        rows = []
        prev = None
        for t in ts:
            a = load(view, t, col)[y0:y1, x0:x1]
            luma, chroma = stats(a)
            bl = blocks(luma)
            d = np.abs(bl - prev) if prev is not None else np.zeros_like(bl)
            rows.append(dict(t=t, luma=float(luma.mean()), chroma=float(chroma.mean()),
                             chroma_p95=float(np.percentile(chroma, 95)),
                             dmean=float(d.mean()), dmax=float(d.max()), dp95=float(np.percentile(d, 95))))
            prev = bl
        out[col] = rows
    if len(sys.argv) > 8:
        json.dump(out, open(sys.argv[8], 'w'), indent=0)
    for col in cols:
        print(f'== {col}')
        print('   t      luma  chroma c95   dmean  dp95  dmax')
        for r in out[col]:
            print(f"{r['t']:.4f} {r['luma']:6.2f} {r['chroma']:5.2f} {r['chroma_p95']:5.2f} {r['dmean']:6.2f} {r['dp95']:5.2f} {r['dmax']:5.1f}")
