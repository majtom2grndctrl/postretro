# Door-side events on the riding m9door view: per 8x8 block, the face it shows (room-local cast) and
# the HEAD/truth steps and differences per 60 fps sample.
#   python m9doorana.py [mask 02|None] [threshold]   (after m9room.py m9door m9 <mask>)
import sys
import numpy as np
sys.path.insert(0, r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools')
import blib
blib.RIDE['m9door'] = ((1728.0, 1740.0, -196.0), 270.0, 0.0, 100.0)
face, P = blib.cast('m9door')
B = 8
hb, wb = face.shape[0] // B, face.shape[1] // B
fb = face[:hb * B, :wb * B].reshape(hb, B, wb, B).transpose(0, 2, 1, 3).reshape(hb, wb, -1)
lab = np.array([[max(set(c), key=list(c).count) for c in row] for row in fb])
Pb = P[:hb * B, :wb * B].reshape(hb, B, wb, B, 3).mean(axis=(1, 3))
NAMES = dict(W='west wall', E='east wall', S='south (door) wall', N='north wall', F='floor', C='ceiling', O='arch opening')
MASK = sys.argv[1] if len(sys.argv) > 1 else '02'
THR = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0
z = np.load(f'../m9door-{MASK}-blocks.npz'); ts = z['ts']; Hb = z['head']; Tb = z['truth']
layers = [-336 + 39.37 * k for k in range(20)]
planes = {'floor top': -272, 'ceiling': -112, 'roof top': -48, 'arch top': -142, 'floor bottom': -336}
cross = sorted(((L - z0) / 272, n, L) for n, z0 in planes.items() for L in layers if 0 < (L - z0) / 272 < 1)
sh = np.abs(Hb[1:] - Hb[:-1]); st = np.abs(Tb[1:] - Tb[:-1])


def where(i, j):
    p = Pb[i, j]
    return f"{NAMES[lab[i, j]]} local ({p[0]:.0f},{p[1]:.0f},{p[2]:.0f})"


print('steps > 3 (HEAD), per sample:')
for k in range(len(ts) - 1):
    m = sh[k].max()
    if m > THR:
        i, j = np.unravel_index(sh[k].argmax(), sh[k].shape)
        near = min(cross, key=lambda c: abs(c[0] - ts[k + 1]))
        print(f'  t {ts[k+1]:.4f} HEAD {m:.2f} truth {st[k][i, j]:.2f} block ({i},{j}) {where(i, j)}; '
              f'nearest crossing {near[1]} past z {near[2]:.1f} at t {near[0]:.4f}')
D = Hb - Tb
print('HEAD - truth beyond +-THR:')
for k in range(len(ts)):
    for sgn in (1, -1):
        m = (sgn * D[k]).max()
        if m > THR:
            i, j = np.unravel_index((sgn * D[k]).argmax(), D[k].shape)
            n = int(((sgn * D[k]) > THR).sum())
            print(f'  t {ts[k]:.4f} {sgn*m:+.2f} ({n} blocks) block ({i},{j}) {where(i, j)} HEAD {Hb[k,i,j]:.1f} truth {Tb[k,i,j]:.1f}')
# brightest moving feature by face: mean per face per t (HEAD, truth)
for f in ('S', 'O', 'F', 'C', 'W', 'E'):
    m = lab == f
    if m.sum() == 0: continue
    h = Hb[:, m].mean(1); t_ = Tb[:, m].mean(1)
    print(f'{NAMES[f]:18s} blocks {m.sum():4d}: HEAD mean {h.min():.1f}-{h.max():.1f}, truth {t_.min():.1f}-{t_.max():.1f}, '
          f'max |HEAD-truth| block {np.abs(D[:, m]).max():.2f}')
