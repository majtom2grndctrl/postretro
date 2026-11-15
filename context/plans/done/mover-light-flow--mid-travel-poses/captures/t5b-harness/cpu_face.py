# Task 5b: a CPU model of em_sweeper's face views from the probe dump
# (`diag_sweeper_probe_dump`): the face sampled on an 8-unit grid as
# `sh_sample.wgsl` reads it (0.1-cell normal offset, trilinear x validity x
# smooth mover backface, no Chebyshev depth), composed and static-reference
# luma per point, and the 12c/12d-style excess metric over the grid: own
# excess of each, and composed minus reference.
#
#   python3 -I cpu_face.py <sweeper.jsonl> east|west
import json, sys
sys.path.insert(0, __file__.rsplit('/', 1)[0])
from cpu_interior import load, Grid, weights, luma, INDIRECT, NORMALS, valid_fn, value
sys.path.insert(0, '/'.join(__file__.split('/')[:-2]) + '/t5-harness')
from cap import excess_metric

path, side = sys.argv[1], sys.argv[2]
header, probes = load(path)
grid = Grid(probes)
n = (1.0, 0.0, 0.0) if side == 'east' else (-1.0, 0.0, 0.0)
ni = NORMALS.index((1, 0, 0) if side == 'east' else (-1, 0, 0))
STEP = 8.0
print(f'{side} face, 8-unit grid over y 72-440, z 8-88: luma (display levels); 12d-style vs reference: mean, -dark max/pits, +bright/peaks')
print('t      | composed  ref | comp-ref mean  dark/pits  bright/peaks | ref own dark/pits | comp own dark/pits')
for ti, t in enumerate(header['ts']):
    x = (1312.0 + 256.0 * t) if side == 'east' else (1280.0 + 256.0 * t)
    whichs = ('new', 'ref', 'today', 'strip') if any('today' in p for p in probes) else ('new', 'ref')
    vals = {w: {} for w in whichs}
    for iy in range(int((440 - 72) / STEP)):
        for iz in range(int((88 - 8) / STEP)):
            P = (x, 72 + (iy + 0.5) * STEP, 8 + (iz + 0.5) * STEP)
            s = tuple(P[a] + n[a] * 0.1 * grid.cell[a] for a in range(3))
            corners, frac = grid.corners(s)
            for which in whichs:
                ws = weights(corners, frac, s, n, valid_fn(which, ti))
                tot = sum(ws)
                if tot <= 0:
                    continue
                rgb = [0.0, 0.0, 0.0]
                for p, w in zip(corners, ws):
                    if w == 0:
                        continue
                    dv, bv = value(p, ti, which, ni)
                    for c in range(3):
                        rgb[c] += w / tot * (dv[c] + INDIRECT * bv[c])
                vals[which][(iy, iz)] = luma(rgb)
    keys = [k for k in vals['new'] if all(k in vals[w] for w in whichs)]
    new = {k: vals['new'][k] for k in keys}
    ref = {k: vals['ref'][k] for k in keys}
    diff = {k: new[k] - ref[k] for k in keys}
    d = excess_metric(diff)
    rn, nn = excess_metric(ref), excess_metric(new)
    extra = ''
    if 'strip' in vals:
        for w in ('new', 'today', 'ref'):
            dd = {k: vals[w][k] - vals['strip'][k] for k in keys}
            m = excess_metric(dd)
            extra += f" | {w}-strip {sum(dd.values())/len(keys):6.2f} {-m['dark']:6.2f}/{m['pits']:<3d}"
    print(f"{t:.3f}  | {sum(new.values())/len(keys):7.2f} {sum(ref.values())/len(keys):6.2f} | {sum(diff.values())/len(keys):8.2f}  {-d['dark']:6.2f}/{d['pits']:<3d} {d['bright']:6.2f}/{d['peaks']:<3d} | {-rn['dark']:6.2f}/{rn['pits']:<3d} | {-nn['dark']:6.2f}/{nn['pits']:<3d}{extra}")
