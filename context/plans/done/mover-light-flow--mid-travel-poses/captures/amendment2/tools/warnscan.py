# Ruling B (i): which probe layers would an authoring warning name?
# Rule tested: a lattice layer, along an axis the mover never travels on, that
# lies inside one of its brushes (strictly, past a 0.1 u band) or within k
# cells in front of a face of that brush, where at least one VALID probe of
# that layer lies inside the brush's swept footprint (the other two axes).
# Faces that move along their own normal cross every layer and are not
# warned (that is ruling A's hand-over).
#   python warnscan.py <maps-dir> [k,k,...]
import os, sys, struct, glob
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import a2map

U = a2map.U
BAND = 0.1  # map units (2.54 mm)


def validity(path):
    s = a2map.sections(path)[34]
    dims = struct.unpack_from('<3I', s, 28)
    n = dims[0] * dims[1] * dims[2]
    return dims, [s[84 + 8 * i] for i in range(n)]


def probe_map(lat, ix, iy, iz):
    eo, ec = lat['engine_origin'], lat['engine_cell']
    ex, ey, ez = eo[0] + ix * ec[0], eo[1] + iy * ec[1], eo[2] + iz * ec[2]
    return (-ez / U, -ex / U, ey / U)


def scan_map(mapfile, prl, ks):
    import numpy as np
    lat = a2map.lattice(prl)
    dims, valid = validity(prl)
    cell = 1.0 / U
    v = np.frombuffer(bytes(valid), dtype=np.uint8).reshape(dims[2], dims[1], dims[0])
    iz, iy, ix = np.nonzero(v)
    eo, ec = lat['engine_origin'], lat['engine_cell']
    ex, ey, ez = eo[0] + ix * ec[0], eo[1] + iy * ec[1], eo[2] + iz * ec[2]
    P = np.stack([-ez / U, -ex / U, ey / U], axis=1)   # map x, y, z of valid probes
    out = []
    for mv in a2map.movers(mapfile):
        wps = mv['waypoints']
        if len(wps) < 2:
            continue
        offs = [tuple(w[a] - wps[0][a] for a in range(3)) for w in wps]
        travel = [any(abs(o[a]) > 1e-6 for o in offs) for a in range(3)]
        for bi, (lo, hi) in enumerate(mv['brushes']):
            if not all(abs(q) < 1e9 for q in lo + hi):
                continue
            slo = [lo[a] + min(o[a] for o in offs) for a in range(3)]
            shi = [hi[a] + max(o[a] for o in offs) for a in range(3)]
            for a in range(3):
                if travel[a]:
                    continue
                thin = (hi[a] - lo[a]) < cell
                others = [b for b in range(3) if b != a]
                foot = np.ones(len(P), bool)
                for b in others:
                    foot &= (P[:, b] >= slo[b] - 1e-3) & (P[:, b] <= shi[b] + 1e-3)
                for k in ks:
                    reach = k * cell
                    l = P[foot, a]
                    inside = (l > lo[a] + BAND) & (l < hi[a] - BAND)
                    near = ((l >= lo[a] - reach) & (l <= lo[a] + BAND)) | ((l >= hi[a] - BAND) & (l <= hi[a] + reach))
                    for kind, sel in (('inside', inside), ('near-face', near & ~inside)):
                        vals, counts = np.unique(np.round(l[sel], 2), return_counts=True)
                        for lv, n in zip(vals, counts):
                            gap = min(abs(lv - lo[a]), abs(lv - hi[a]))
                            out.append(dict(mover=mv['name'], brush=bi, axis='xyz'[a], layer=float(lv), kind=kind,
                                            k=k, probes=int(n), gap_u=round(float(gap), 2), thin=thin,
                                            slab=(lo[a], hi[a])))
    return out


if __name__ == '__main__':
    d = sys.argv[1]
    ks = [float(v) for v in (sys.argv[2] if len(sys.argv) > 2 else '0.1,0.25,0.4').split(',')]
    for mapfile in sorted(glob.glob(os.path.join(d, '*.map'))):
        if 'kinematic_mover' not in open(mapfile, encoding='utf-8').read():
            continue
        prl = mapfile[:-4] + '.prl'
        name = os.path.basename(mapfile)[:-4]
        if not os.path.exists(prl):
            print(f'{name}: no PRL, skipped'); continue
        rows = scan_map(mapfile, prl, ks)
        for k in ks:
            r = [x for x in rows if x['k'] == k]
            thick = {(x['mover'], x['axis'], x['layer']) for x in r if not x['thin'] and x['kind'] == 'inside'}
            r = [x for x in r if x['thin']]
            uniq = {(x['mover'], x['axis'], x['layer'], x['kind']) for x in r}
            ins = {u for u in uniq if u[3] == 'inside'}
            print(f'{name} k={k}: {len(uniq)} warned layer(s) on thin slabs ({len(ins)} inside, {len(uniq) - len(ins)} near a face); {len(thick)} layer(s) inside slabs 1 cell or thicker (not warned)')
            seen = set()
            for x in r:
                u = (x['mover'], x['axis'], x['layer'], x['kind'])
                if u in seen: continue
                seen.add(u)
                print(f"    {x['mover']} brush {x['brush']} {x['axis']}={x['layer']} {x['kind']} gap {x['gap_u']} u, slab {x['slab']}, {x['probes']} valid probes")
