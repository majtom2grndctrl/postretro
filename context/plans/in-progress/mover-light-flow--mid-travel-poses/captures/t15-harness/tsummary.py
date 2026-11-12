# Summary numbers from t15/results/<view>.json (after tanalyze.py).
import json, os
import numpy as np
R = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/t15/results'
SWEEPER_HEAD = [k / 7 for k in range(8)]
out = {}
for v in ('ceiling', 'floor', 'interior', 'east', 'west'):
    J = json.load(open(os.path.join(R, f'{v}.json')))
    o = {'vs_truth': J['vs_truth']}
    ts = J['std_ts']
    # Steps: on the std grid (amend/head/two) and on the truth grid (amend/head/truth).
    o['std_steps'] = {c: dict(max=max(x['max'] for x in s), median=float(np.median([x['max'] for x in s])))
                      for c, s in J['steps'].items()}
    if 'truth_steps' in J:
        o['truth_steps'] = {c: dict(max=max(x['max'] for x in s), median=float(np.median([x['max'] for x in s])))
                            for c, s in J['truth_steps'].items()}
    if v in ('ceiling', 'floor'):
        # 12c peaks between knots (t' in (0.1, 0.95)), std grid for amend/head/two, truth grid for truth/amend.
        o['12c_std'] = {c: dict(peaks_max=max(r['12c']['peaks'] for r in rows if 0.1 <= r['t'] <= 0.95),
                                peaks_median=float(np.median([r['12c']['peaks'] for r in rows if 0.1 <= r['t'] <= 0.95])),
                                bright_max=max(r['12c']['bright'] for r in rows if 0.1 <= r['t'] <= 0.95))
                        for c, rows in J['std'].items()}
        o['12c_truthgrid'] = {c: dict(peaks_max=max(r['12c']['peaks'] for r in rows if 0.1 <= r['t'] <= 0.95),
                                      peaks_median=float(np.median([r['12c']['peaks'] for r in rows if 0.1 <= r['t'] <= 0.95])),
                                      bright_max=max(r['12c']['bright'] for r in rows if 0.1 <= r['t'] <= 0.95))
                              for c, rows in J['truthgrid'].items()}
    if v == 'ceiling':
        fl = J['flash']
        drops = sorted(((d['drop'], float(k)) for k, d in fl.items()), reverse=True)
        o['flash'] = dict(max_drop=drops[0], min_drop=drops[-1], mean_drop=float(np.mean([d for d, _ in drops])))
    if v in ('east', 'west'):
        # Peak-to-trough within each pre-amendment period k/7 over t' <= 4/7, std grid.
        p2t = {}
        for c, rows in J['std'].items():
            per = []
            for k in range(4):
                seg = [r['mean'] for r in rows if SWEEPER_HEAD[k] - 1e-9 <= r['t'] <= SWEEPER_HEAD[k + 1] + 1e-9]
                per.append(max(seg) - min(seg))
            p2t[c] = [round(x, 2) for x in per]
        tr = J['truthgrid']['truth']
        per = []
        for k in range(4):
            seg = [r['mean'] for r in tr if SWEEPER_HEAD[k] - 1e-9 <= r['t'] <= SWEEPER_HEAD[k + 1] + 1e-9]
            per.append(max(seg) - min(seg))
        p2t['truth'] = [round(x, 2) for x in per]
        o['period_peak_to_trough'] = p2t
        tts = J['truth_ts']
        d = {c: np.array([r['mean'] for r in J['truthgrid'][c]]) - np.array([r['mean'] for r in tr]) for c in ('amend', 'head')}
        o['residual_vs_truth_range_t_le_0.6'] = {c: round(float(np.ptp([x for x, t in zip(d[c], tts) if t <= 0.6])), 2) for c in d}
    out[v] = o
json.dump(out, open(os.path.join(R, 'summary.json'), 'w'), indent=1)
print(json.dumps(out, indent=1, default=lambda x: round(x, 3)))
