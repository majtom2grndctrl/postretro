# Big-room sighting: riding views (pixels fixed to the room), 8x8 blocks, HEAD vs truth per sample.
#   python m9room.py <view> <capdir: m9|a3> [mask] [t-spec]
import sys
import numpy as np
from PIL import Image
import os
import m9cap, blib

view, src = sys.argv[1], sys.argv[2]
mask = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] != '-' else None
ts = m9cap.parse_ts(sys.argv[4]) if len(sys.argv) > 4 else blib.trip_ts(1)
B = 8


def path(t, col):
    return m9cap.path(view, t, col, mask) if src == 'm9' else blib.path(view, t, col, mask)


def blk(t, col):
    p = path(t, col)
    if not os.path.exists(p): return None
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)
    L = 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
    h, w = L.shape
    return L[:h // B * B, :w // B * B].reshape(h // B, B, w // B, B).mean(axis=(1, 3))


Hb = np.array([blk(t, 'head') for t in ts]); Tb = np.array([blk(t, 'truth') for t in ts])
np.savez(f'../{view}-{mask}-blocks.npz', ts=ts, head=Hb, truth=Tb)
D = Hb - Tb
nt = len(ts)
print(f'{view} m{mask}: {nt} samples t {ts[0]:.4f}-{ts[-1]:.4f}; mean |HEAD-truth| {np.abs(D).mean():.3f}')
for sign, name in ((1, 'HEAD brighter'), (-1, 'HEAD darker')):
    k = np.argmax((sign * D).reshape(nt, -1).max(1)); i, j = np.unravel_index(np.argmax(sign * D[k]), D[k].shape)
    print(f'  {name}: {D[k, i, j]:+.2f} at t {ts[k]:.4f} block row {i} col {j} (px {j*B},{i*B}); HEAD {Hb[k,i,j]:.1f} truth {Tb[k,i,j]:.1f}')
    cnt = ((sign * D) > 3).reshape(nt, -1).any(1).sum()
    print(f'     samples with a block {name.split()[1]} by > 3: {cnt}')
for name, A in (('head', Hb), ('truth', Tb)):
    st = np.abs(A[1:] - A[:-1]).reshape(nt - 1, -1)
    k = st.max(1).argmax()
    print(f'  {name}: largest per-sample block step {st.max():.2f} at t {ts[k+1]:.4f}')
sh = np.abs(Hb[1:] - Hb[:-1]) - np.abs(Tb[1:] - Tb[:-1])
k = sh.reshape(nt - 1, -1).max(1).argmax(); i, j = np.unravel_index(sh[k].argmax(), sh[k].shape)
print(f'  HEAD step over truth step, worst {sh[k,i,j]:+.2f} at t {ts[k+1]:.4f} block ({i},{j})')
