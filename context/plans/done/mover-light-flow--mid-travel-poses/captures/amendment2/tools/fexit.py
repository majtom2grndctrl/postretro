# d. Enclosure exits and entries on mover-setpiece's door lintel (room mover), above-door band A.
#   python fexit.py ts          -> print the capture t list (comma-separated) for every window
#   python fexit.py analyze     -> tables + final/results/fexit.json
# Windows come from the room's geometry knots around each lintel crossing (knots.py on the
# final head PRL): an entry is (outside knot, inside knot) as the roof top passes a layer;
# an exit is (inside knot, outside knot) as the lintel bottom passes it. Samples every
# FRAME/8 t' from MARGIN before the first knot to MARGIN after the second, so a lag of 8
# samples is one 60 fps frame at every phase.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flib  # noqa: E402

FRAME = 176.0 / 60.0 / 176.0 * (1.0 / 1.0)  # placeholder, replaced below
# Room: 176 u at speed 1 (39.37 u/s): 0.66 u per 60 fps frame = 1/268 t'.
FRAME = 1.0 / 268.0
SUB = 8
DT = FRAME / SUB
MARGIN = 0.0035
# (kind, first knot, second knot, crossing name) from the final head PRL's room knots.
WINDOWS = [
    ('entry', 0.02210, 0.03002, 'roof top past z 4.85'),
    ('exit', 0.07413, 0.08205, 'lintel bottom past z -34.52'),
    ('entry', 0.24580, 0.25371, 'roof top past z 44.22'),
    ('exit', 0.29782, 0.30574, 'lintel bottom past z 4.85'),
    ('entry', 0.46949, 0.47741, 'roof top past z 83.59'),
    ('exit', 0.52152, 0.52944, 'lintel bottom past z 44.22'),
    ('entry', 0.69318, 0.70110, 'roof top past z 122.96'),
    ('exit', 0.74521, 0.75313, 'lintel bottom past z 83.59'),
    ('entry', 0.91688, 0.92480, 'roof top past z 162.33'),
    ('exit', 0.96891, 0.97682, 'lintel bottom past z 122.96'),
]
COLS = ['head', 'ohead', 'odark']


def window_ts(k0, k1):
    n = int(np.ceil((k1 - k0 + 2 * MARGIN) / DT))
    return [round(k0 - MARGIN + i * DT, 5) for i in range(n + 1)]


def analyze(region):
    res = []
    for kind, k0, k1, name in WINDOWS:
        ts = window_ts(k0, k1)
        row = dict(kind=kind, k0=k0, k1=k1, name=name, cols={})
        for col in COLS:
            means, bl = flib.series('door_ceiling', region, col, ts)
            if any(b is None for b in bl):
                row['cols'][col] = dict(missing=int(sum(b is None for b in bl)))
                continue
            tt = np.array(ts)
            # inside pose: k0 for an exit, k1 for an entry; reference = that sample (exit) or the settled tail (entry)
            if kind == 'exit':
                iref = int(np.argmin(np.abs(tt - k0)))
                ref = bl[iref]
                rng = range(iref, len(ts))
            else:
                iref = int(np.argmin(np.abs(tt - k1)))
                ref = bl[-1]
                rng = range(0, len(ts))
            dips = [(float((bl[i] - ref).min()), ts[i]) for i in rng]
            dip = min(dips)
            mdips = [(float(means[i] - float(np.mean(ref))), ts[i]) for i in rng]
            st = flib.steps(bl, SUB)
            stv = [(s, t) for s, t in zip(st, ts) if s is not None]
            smax = max(stv)
            row['cols'][col] = dict(dip=dip[0], dip_t=dip[1], mean_dip=min(mdips)[0], step=smax[0], step_t=smax[1],
                                    means=means, steps=st, ts=ts,
                                    ref_mean=float(np.mean(ref)), end_mean=means[-1], start_mean=means[0])
        res.append(row)
    return res


def table(res, region):
    print(f'\n## d. Lintel crossings, band {region}: worst block below the inside-pose value (exit) or settled value (entry); '
          f'largest block step over one frame span ({FRAME:.5f} t\', any phase)')
    print(f"{'kind':5s} {'knots':17s} " + ''.join(f'{c:>34s}' for c in COLS) + '   head-ohead (dip, step)')
    for r in res:
        line = f"{r['kind']:5s} {r['k0']:.5f}-{r['k1']:.5f} "
        for c in COLS:
            v = r['cols'].get(c, {})
            if 'dip' not in v:
                line += f"{'miss ' + str(v.get('missing')):>34s}"; continue
            line += f"   dip {v['dip']:+5.2f}@{v['dip_t']:.4f} step {v['step']:4.2f}@{v['step_t']:.4f}"
        h, o = r['cols'].get('head', {}), r['cols'].get('ohead', {})
        if 'dip' in h and 'dip' in o:
            line += f"   {h['dip'] - o['dip']:+.2f}, {h['step'] - o['step']:+.2f}"
        print(line)


if __name__ == '__main__':
    if sys.argv[1] == 'ts':
        allts = sorted({t for _, k0, k1, _ in WINDOWS for t in window_ts(k0, k1)})
        print(','.join(f'{t:.5f}' for t in allts))
    else:
        out = {}
        for region in ('A', 'B', 'band'):
            res = analyze(region)
            table(res, region)
            out[region] = res
        json.dump(out, open(os.path.join(flib.F, 'results', 'fexit.json'), 'w', encoding='utf-8'))
