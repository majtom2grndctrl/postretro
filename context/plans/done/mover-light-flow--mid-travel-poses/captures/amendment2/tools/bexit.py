# Big-room enclosure exits and entries (2026-11-13), fexit.py's metric on setpiece_room_big.
#   python bexit.py list          -> every predicted crossing, its bracketing knots, views, window size
#   python bexit.py ts <view>     -> comma-separated capture t list for every window that view sees
#   python bexit.py analyze       -> tables + amend3/results/bexit.json
# A crossing is a room plane passing a lattice z layer as the room rises (+272 u over t' 0..1).
# Its window runs from MARGIN before the knot at or below the crossing to MARGIN after the knot
# above it (knots from the HEAD PRL's id 57, knots.py). Samples every FRAME/SUB t' (<= 0.0005), so a
# lag of SUB samples is one 60 fps frame span at every phase.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import blib, knots  # noqa: E402

FRAME = blib.FRAME_T                  # 0.002412 t'
SUB = 5
DT = FRAME / SUB                      # 0.000482 t'
MARGIN = 0.003
PRL = os.path.join(blib.A3, 'ws', 'content', 'dev', 'maps', 'head-mover-setpiece.prl')
MOVER = int(os.environ.get('B3_MOVER', '2'))
LZ0, CELL = -336.0, blib.CELL          # lattice z origin and spacing (bake log)
LX0 = 2112.0                           # lattice x origin (Quake), layers at LX0 - k * CELL
# (plane name, kind as the room rises, local z or None for the arch, views: regions)
PLANES = [
    ('ceiling', 'exit', -112.0, {'bw': ['W', 'ceil'], 'bs': ['S', 'A', 'ceil']}),
    ('roof top', 'entry', -48.0, {'broof': ['topW', 'topS', 'top']}),
    ('floor top', 'entry', -272.0, {'bfloor': ['FW', 'FS', 'floor']}),
    ('arch', 'exit', None, {'bs': ['A', 'AF']}),
    ('floor bottom', 'exit', -336.0, {}),   # leaves into the sealed shaft below the room: no view sees it
]
COLS = ['head', 'ramp']


def room_knots():
    s = knots.sections(PRL)[57]
    _, ms = knots.movers(s)
    for mid, rs, re_, k in ms:
        if mid == MOVER:
            return np.array(k)
    raise SystemExit(f'mover {MOVER} not in {PRL}')


def arch_columns():
    """Lattice x columns under the arch (x 1678-1778) with the arch bottom above each, local z."""
    out = []
    for k in range(60):
        x = LX0 - k * CELL
        if 1678 < x < 1778:
            out.append((x, float(blib.arch_bottom(x))))
    return out


def crossings():
    K = room_knots()
    out = []
    for name, kind, zp, views in PLANES:
        planes = [(None, zp)] if zp is not None else arch_columns()
        for col, z in planes:
            for i in range(200):
                zl = LZ0 + i * CELL
                t = (zl - z) / blib.TRAVEL
                if not (0.0 < t < 1.0):
                    continue
                lo = K[K <= t + 1e-6]; hi = K[K >= t - 1e-6]
                k0, k1 = float(lo.max()), float(hi.min())
                if k1 - k0 < 1e-6:     # a knot sits on the crossing: take its neighbours
                    k0 = float(K[K < t - 1e-6].max()); k1 = float(K[K > t + 1e-6].min())
                lab = name if col is None else f'arch x {col:.1f}'
                out.append(dict(plane=lab, kind=kind, layer=round(zl, 2), t=round(t, 5), k0=k0, k1=k1,
                                views=views))
    return sorted(out, key=lambda c: c['t'])


def window_ts(k0, k1):
    n = int(np.ceil((k1 - k0 + 2 * MARGIN) / DT))
    return [round(k0 - MARGIN + i * DT, 5) for i in range(n + 1)]


def analyze_one(c, view, region, col):
    ts = window_ts(c['k0'], c['k1'])
    means, bl = blib.series(view, region, col, ts)
    if any(b is None for b in bl):
        return dict(missing=int(sum(b is None for b in bl)))
    tt = np.array(ts)
    if c['kind'] == 'exit':
        iref = int(np.argmin(np.abs(tt - c['k0'])))
        ref = bl[iref]; rng = range(iref, len(ts))
    else:
        ref = bl[-1]; rng = range(0, len(ts))
    dips = [(float((bl[i] - ref).min()), ts[i]) for i in rng]
    dip = min(dips)
    # Transient dip: worst block below the chord from the window's reference sample to its last
    # sample, so a steady lighting trend across the window (a lit face rising through a gradient)
    # does not read as a dip. 25c's flat bands make the two metrics agree.
    i0 = rng[0]; i1 = len(ts) - 1
    cd = [(float((bl[i] - (bl[i0] + (bl[i1] - bl[i0]) * (i - i0) / max(1, i1 - i0))).min()), ts[i]) for i in rng]
    cdip = min(cd)
    # Undershoot: worst block below both its reference and its settled (last) value, so a monotone
    # level change reads 0 and only a fall that recovers (25c's exit-dip artifact) reads negative.
    floor_ = np.minimum(bl[i0], bl[i1])
    us = min((float(np.minimum(bl[i] - floor_, 0).min()), ts[i]) for i in rng)
    st = [None] * len(ts)
    for i in range(SUB, len(ts)):
        st[i] = float(np.abs(bl[i] - bl[i - SUB]).max())
    smax = max((s, t) for s, t in zip(st, ts) if s is not None)
    return dict(bl=bl, iref=rng[0], dip=dip[0], dip_t=dip[1], cdip=cdip[0], cdip_t=cdip[1], under=us[0], under_t=us[1], step=smax[0], step_t=smax[1], means=means, steps=st, ts=ts,
                ref_mean=float(np.mean(ref)))


def analyze():
    res = []
    for c in crossings():
        row = dict(c, views=None, res={})
        for view, regs in c['views'].items():
            for region in regs:
                row['res'][f'{view}/{region}'] = {col: analyze_one(c, view, region, col) for col in COLS}
        # Relative dip: head's blocks minus ramp alone's, from the reference sample on, against that
        # difference at the reference (exit) or at the settled end (entry). Lighting the two builds
        # share cancels; what remains is the dark state and edge copy.
        for key, cols in row['res'].items():
            h, o = cols['head'], cols['ramp']
            if 'bl' not in h or 'bl' not in o:
                continue
            d = [a - b for a, b in zip(h['bl'], o['bl'])]
            ref = d[h['iref']] if c['kind'] == 'exit' else d[-1]
            rel = [(float((d[i] - ref).min()), h['ts'][i]) for i in range(h['iref'], len(d))]
            h['reldip'], h['reldip_t'] = min(rel)
        for cols in row['res'].values():
            for v in cols.values():
                v.pop('bl', None)
        res.append(row)
    print(f"\n## Enclosure exits and entries on setpiece_room_big: dip below the inside-pose (exit) or settled (entry)"
          f" value, worst 8x8 block; largest block step over one frame span ({FRAME:.5f} t', any phase, samples every {DT:.5f})")
    worst = {}
    for r in res:
        for key, cols in r['res'].items():
            h, o = cols['head'], cols['ramp']
            if 'dip' not in h or 'dip' not in o:
                print(f"  {r['kind']:5s} {r['plane']:14s} z {r['layer']:8.2f} t {r['t']:.5f} {key:12s} missing {h.get('missing')}/{o.get('missing')}")
                continue
            ex = h['step'] - o['step']
            rd = h.get('reldip', float('nan'))
            print(f"  {r['kind']:5s} {r['plane']:14s} z {r['layer']:8.2f} t {r['t']:.5f} knots {r['k0']:.5f}-{r['k1']:.5f}"
                  f" {key:12s} head dip {h['dip']:+6.2f} chord {h['cdip']:+6.2f} step {h['step']:5.2f} | ramp dip {o['dip']:+6.2f} chord {o['cdip']:+6.2f} step {o['step']:5.2f}"
                  f" | step - ramp {ex:+5.2f} | under head {h['under']:+5.2f} ramp {o['under']:+5.2f}")
            w = worst.setdefault('us', (0, None)); worst['us'] = min(w, (h['under'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('rus', (0, None)); worst['rus'] = min(w, (o['under'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('rel', (0, None)); worst['rel'] = min(w, (rd, f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('dip', (0, None)); worst['dip'] = min(w, (h['dip'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('ex', (-99, None)); worst['ex'] = max(w, (ex, f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('cdip', (0, None)); worst['cdip'] = min(w, (h['cdip'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('rcdip', (0, None)); worst['rcdip'] = min(w, (o['cdip'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('dd', (99, None)); worst['dd'] = min(w, (h['dip'] - o['dip'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
            w = worst.setdefault('rdip', (0, None)); worst['rdip'] = min(w, (o['dip'], f"{r['plane']} {r['t']:.4f} {key}"), key=lambda p: p[0])
    print('  worst head undershoot', worst.get('us'), '| worst ramp undershoot', worst.get('rus'))
    print('  worst head dip vs ramp alone', worst.get('rel'))
    print('  worst head chord dip', worst.get('cdip'), '| worst ramp chord dip', worst.get('rcdip'), '| worst head dip - ramp dip', worst.get('dd'))
    print('  worst head dip', worst.get('dip'), '| worst ramp dip', worst.get('rdip'), '| worst head step over ramp', worst.get('ex'))
    os.makedirs(os.path.join(blib.A3, 'results'), exist_ok=True)
    json.dump(res, open(os.path.join(blib.A3, 'results', 'bexit.json'), 'w', encoding='utf-8'))


if __name__ == '__main__':
    if sys.argv[1] == 'list':
        K = room_knots()
        print(f'{len(K)} knots:', ' '.join(f'{k:.5f}' for k in K))
        n = 0
        for c in crossings():
            w = len(window_ts(c['k0'], c['k1']))
            n += w * sum(1 for _ in c['views'])
            print(f"  {c['kind']:5s} {c['plane']:14s} layer z {c['layer']:8.2f} t' {c['t']:.5f} knots {c['k0']:.5f}-{c['k1']:.5f}"
                  f" gap {c['k1'] - c['k0']:.5f} samples {w} views {','.join(c['views']) or '-'}")
        print('samples per column (view x window):', n)
    elif sys.argv[1] == 'ts':
        v = sys.argv[2]
        ts = sorted({t for c in crossings() if v in c['views'] for t in window_ts(c['k0'], c['k1'])})
        print(','.join(f'{t:.5f}' for t in ts))
    else:
        analyze()
