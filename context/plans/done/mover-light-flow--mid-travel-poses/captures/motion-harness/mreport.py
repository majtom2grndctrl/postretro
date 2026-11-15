# Text summaries from motion/results/<view>.json.
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcap

RES = os.path.join(mcap.M, 'results')
KN = json.load(open(os.path.join(RES, 'knots.json')))


def where(view, t):
    k = KN[view]
    mids = [0.5 * (a + b) for a, b in zip(k, k[1:])]
    dk = min(abs(t - x) for x in k)
    dm = min(abs(t - x) for x in mids)
    return f'knot{"" if dk < 1e-3 else f"+-{dk:.3f}"}' if dk <= dm else f'mid{"" if dm < 1e-3 else f"+-{dm:.3f}"}'


def main(view):
    R = json.load(open(os.path.join(RES, f'{view}.json')))
    ts = R['ts']
    print(f'== {view} ({len(ts)} samples)')
    rows = R['rows']
    # Crop means at knots and midpoints.
    k = KN[view]
    marks = sorted(set([min(ts, key=lambda t: abs(t - x)) for x in k + [0.5 * (a + b) for a, b in zip(k, k[1:])]]))
    print('t      where    head   nodep  two    strip  | h-two  h-nod  h-strip' + ('  | 12c head bright/peaks  two  nodepth' if view in ('floor', 'ceiling') else '  | 12d head mean/dark/pits  two  nodepth' if 'west' == view or 'east' == view else ''))
    for r in rows:
        if r['t'] not in marks:
            continue
        cm = r['crop_mean']
        line = f"{r['t']:.3f}  {where(view, r['t']):8s} " + ' '.join(f'{cm[c]:6.2f}' for c in mcap.COLUMNS)
        line += f" | {cm['head'] - cm['two']:+6.2f} {cm['head'] - cm['nodepth']:+6.2f} {cm['head'] - cm['strip']:+6.2f}"
        if '12c' in r:
            line += '  | ' + '  '.join(f"{r['12c'][c]['bright']:.1f}/{r['12c'][c]['peaks']}" for c in ('head', 'two', 'nodepth'))
        elif '12d' in r:
            line += '  | ' + '  '.join(f"{r['12d'][c]['mean']:+.1f}/{r['12d'][c]['dark']:.1f}/{r['12d'][c]['pits']}" for c in ('head', 'two', 'nodepth'))
        print(line)
    for c in mcap.COLUMNS:
        s = R['steps'][c]
        mx = np.array([x['max'] for x in s])
        p95 = np.array([x['p95'] for x in s])
        order = np.argsort(-mx)[:6]
        print(f'  {c:8s} steps: median max {np.median(mx):.2f}, median p95 {np.median(p95):.2f}; top:',
              ', '.join(f"{s[i]['t0']:.3f}-{s[i]['t1']:.3f}({where(view, s[i]['t1']) if abs(s[i]['t1']-min(KN[view], key=lambda x: abs(x-s[i]['t1'])))<abs(s[i]['t0']-min(KN[view], key=lambda x: abs(x-s[i]['t0']))) else where(view, s[i]['t0'])}) {mx[i]:.1f}/p95 {p95[i]:.2f}" for i in order))
        kk = R['kinks'][c]
        kx = np.array([x['max'] for x in kk])
        order = np.argsort(-kx)[:6]
        print(f'  {c:8s} kinks: median max {np.median(kx):.2f}; top:',
              ', '.join(f"{kk[i]['t']:.3f}({where(view, kk[i]['t'])}) {kx[i]:.1f}" for i in order))


if __name__ == '__main__':
    for v in sys.argv[1:] or list(mcap.VIEWS):
        main(v)
