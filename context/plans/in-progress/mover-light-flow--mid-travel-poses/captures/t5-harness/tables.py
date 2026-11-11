import json, os, sys
S = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
R = json.load(open(os.path.join(S, 'results.json')))
def d(e):  # 12d cell
    return f"{e['mean']:+.1f}, +{e['bright']:.1f}/{e['peaks']} −{e['dark']:.1f}/{e['pits']}"
def c(e):  # 12c cell
    return f"{e['bright']:.1f} / {e['peaks']}"
for view in ('roof', 'west', 'east'):
    print(f'\n### {view} (12d: mean vs stripped, bright max/peaks, dark max/pits)\n')
    print('| t′ | today | through-base | lerp | stripped today vs new |')
    print('|---|---|---|---|---|')
    for row in R[view]:
        x = row['12d']
        print(f"| {row['t']:.2f} | {d(x['today'])} | {d(x['tb'])} | {d(x['lerp'])} | {x['strip-new']['mean']:+.2f} (max px Δ {row['strip_max_luma_delta']:.1f}) |")
print('\n### floor (12c raw: max excess / peaks; crop mean luma in brackets)\n')
print('| t′ | today | through-base | lerp | stripped |')
print('|---|---|---|---|---|')
for row in R['floor']:
    x, m = row['12c'], row['crop_mean']
    print(f"| {row['t']:.2f} | {c(x['today'])} [{m['today']:.1f}] | {c(x['tb'])} [{m['tb']:.1f}] | {c(x['lerp'])} [{m['lerp']:.1f}] | {c(x['strip-new'])} [{m['strip-new']:.1f}] |")
print('\n### floor 12c dark side (dark max / pits)\n')
for row in R['floor']:
    x = row['12c']
    print(row['t'], {k: f"{v['dark']:.1f}/{v['pits']}" for k, v in x.items()})
print('\n### interior frame mean luma\n')
print('| t′ | today | through-base | lerp | stripped |')
print('|---|---|---|---|---|')
for row in R['interior']:
    m = row['frame_mean']
    print(f"| {row['t']:.2f} | {m['today']:.2f} | {m['tb']:.2f} | {m['lerp']:.2f} | {m['strip-new']:.2f} |")
print('\n### frame-level checks: tb vs lerp identical?')
for view in R:
    for row in R[view]:
        m = row['frame_mean']
        if abs(m['tb'] - m['lerp']) > 1e-9:
            print(view, row['t'], f"tb {m['tb']:.3f} lerp {m['lerp']:.3f}")
