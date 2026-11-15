# Task 15b: per-variant ceiling and east numbers against the truth.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import vcap  # registers variant columns
SP = vcap.SP
sys.path.insert(0, SP + '/t15/tools')
import tanalyze as ta
out = {}
for col in ('head', 'amend', 'o20', 'o10', 'o005'):
    out[col] = {}
    for view in ('ceiling', 'east'):
        tr, trc = ta.series(view, 'truth', 'truth')
        r, c = ta.series(view, 'truth', col)
        trm = np.array([x['mean'] for x in tr]); m = np.array([x['mean'] for x in r])
        d = m - trm
        tts = ta.tcap.ts_for(view, 'truth')
        o = dict(mean_abs=float(np.mean(np.abs(d))), max_abs=float(np.max(np.abs(d))),
                 t_max=tts[int(np.argmax(np.abs(d)))],
                 cell_max=float(np.nanmax(np.abs(c - trc))),
                 top=[(round(tts[i], 3), round(float(d[i]), 2)) for i in np.argsort(-np.abs(d))[:4]])
        if view == 'ceiling':
            sr, _ = ta.series(view, 'std', col) if col != 'head' else ta.series(view, 'std', 'head')
            pk = [x['12c']['peaks'] for x in sr if 0.1 <= x['t'] <= 0.95]
            o['12c_peaks_median'] = float(np.median(pk)); o['12c_peaks_max'] = int(max(pk))
        out[col][view] = o
        print(col, view, json.dumps(o), flush=True)
json.dump(out, open(SP + '/t15b/variants.json', 'w'), indent=1)
