# Sweeper sighting: face-anchored east-face series, HEAD vs truth (mask 02), from the owner's view.
#   python m9sweep.py <view> [mask]
import json, sys
import numpy as np
import m9cap, m9face

view = sys.argv[1] if len(sys.argv) > 1 else 'sweeper_east'
mask = sys.argv[2] if len(sys.argv) > 2 else '02'
ts = m9cap.parse_ts('step:0:1:0.0029')
FRAME = 4.5 / 60.0 / (256 * 0.0254)       # one 60 fps frame in t' (0.01153)
lag = int(round(FRAME / 0.0029))
Hs = m9face.series(view, 'sweep_east', 'head', ts, mask)
Ts = m9face.series(view, 'sweep_east', 'truth', ts, mask)
us, vs = m9face.grid('sweep_east')
np.savez(f'../{view}-{mask}-series.npz', ts=ts, head=Hs, truth=Ts, us=us, vs=vs)
D = Hs - Ts
print(f'{view} m{mask}: {len(ts)} samples, lag {lag} = one 60 fps frame; cells {len(us)}x{len(vs)} (16 u)')
# worst HEAD - truth per sample
k = np.nanargmax(np.nanmax(D.reshape(len(ts), -1), axis=1))
i, j = np.unravel_index(np.nanargmax(D[k]), D[k].shape)
print(f'  worst HEAD-truth {D[k, i, j]:+.2f} at t {ts[k]:.4f}, cell y {us[i]:.0f} z {vs[j]:.0f} '
      f'(HEAD {Hs[k, i, j]:.1f}, truth {Ts[k, i, j]:.1f})')
kk = np.nanargmin(np.nanmin(D.reshape(len(ts), -1), axis=1))
i2, j2 = np.unravel_index(np.nanargmin(D[kk]), D[kk].shape)
print(f'  most negative {D[kk, i2, j2]:+.2f} at t {ts[kk]:.4f}, cell y {us[i2]:.0f} z {vs[j2]:.0f}')
print(f'  mean |HEAD-truth| {np.nanmean(np.abs(D)):.2f}')
# corner: y 64-128, z 64-96 (owner's upper left)
cm = (us < 128)[:, None] & (vs > 64)[None, :]
for name, A in (('head', Hs), ('truth', Ts)):
    c = np.array([np.nanmean(a[cm]) for a in A])
    st = np.abs(A[lag:] - A[:-lag]).reshape(len(ts) - lag, -1)
    stc = np.abs(A[lag:] - A[:-lag])[:, cm]
    k = np.nanargmax(np.nanmax(st, axis=1)); kc = np.nanargmax(np.nanmax(stc, axis=1))
    print(f'  {name}: corner mean range {np.nanmin(c):.1f}-{np.nanmax(c):.1f}; largest 1-frame cell step '
          f'{np.nanmax(st):.2f} at t {ts[k + lag]:.4f}; in corner {np.nanmax(stc):.2f} at t {ts[kc + lag]:.4f}')
# bumps: cell value above the mean of its values one frame before and after
for name, A in (('head', Hs), ('truth', Ts)):
    bump = A[lag:-lag] - 0.5 * (A[:-2 * lag] + A[2 * lag:])
    k = np.nanargmax(np.nanmax(bump.reshape(len(bump), -1), axis=1))
    i, j = np.unravel_index(np.nanargmax(bump[k]), bump[k].shape)
    print(f'  {name}: largest transient bump {bump[k, i, j]:+.2f} at t {ts[k + lag]:.4f} cell y {us[i]:.0f} z {vs[j]:.0f}')
