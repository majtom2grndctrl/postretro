# oan.py <view> <cols> <t0> <t1> <n> <x0,y0,x1,y1> [knots-mover-index prl]
# Per t: crop mean luma per column, frame-to-frame block steps, column differences.
import sys, os, json, numpy as np
from PIL import Image
F = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
view, cols = sys.argv[1], sys.argv[2].split(',')
t0, t1, n = float(sys.argv[3]), float(sys.argv[4]), int(sys.argv[5])
x0, y0, x1, y1 = map(int, sys.argv[6].split(','))
ts = [round(t0 + (t1 - t0) * i / (n - 1), 5) for i in range(n)]
B = 8
def load(t, c):
    p = f'{F}/cap/{view}-t{t:.5f}-{c}.png'
    if not os.path.exists(p): return None
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)[y0:y1, x0:x1]
    return a
def blocks(a):
    h, w = (a.shape[0] // B) * B, (a.shape[1] // B) * B
    return a[:h, :w].reshape(h // B, B, w // B, B, -1).mean(axis=(1, 3))
def luma(a): return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
data = {c: [] for c in cols}
for t in ts:
    for c in cols:
        a = load(t, c); data[c].append(None if a is None else blocks(a))
out = {'ts': ts}
print('t      ' + ''.join(f'{c:>9}' for c in cols) + '  | step(max blk luma) ' + ' '.join(cols) + ' | ' + ' '.join(f'{c}-{cols[0]}(mean,maxabs)' for c in cols[1:]))
for c in cols: out[c] = dict(mean=[], step=[], rgb=[])
for i, t in enumerate(ts):
    row = f'{t:.5f}'
    for c in cols:
        b = data[c][i]
        m = float('nan') if b is None else luma(b).mean()
        out[c]['mean'].append(m); out[c]['rgb'].append(None if b is None else b.mean(axis=(0, 1)).tolist())
        row += f'{m:9.2f}'
    row += '  |'
    for c in cols:
        b, p = data[c][i], data[c][i - 1] if i else None
        s = float('nan') if (b is None or p is None) else np.abs(luma(b) - luma(p)).max()
        out[c]['step'].append(s); row += f'{s:7.2f}'
    row += ' |'
    for c in cols[1:]:
        b, h = data[c][i], data[cols[0]][i]
        if b is None or h is None: row += '      -      '; continue
        d = luma(b) - luma(h); row += f' {d.mean():6.2f} {np.abs(d).max():6.2f}'
    print(row)
json.dump(out, open(f'{F}/results-{view}.json', 'w'))
