# Task 25c capture driver: acap.py's views and settings, final binaries and PRLs.
#   python fcap.py batch <jobs> <view> <col,col> <t-spec> [mask]
# Columns:
#   head, truth            final engine (ramp is the only mover backface), final PRLs
#   ohead, odark, odtruth  prototype engine + POSTRETRO_PROTO_MOVER_SH r40, old PRLs
#                          (ramp-alone, dark+r40 prototype without edge copy, its dense truth)
# Labels: <view>-t<t>-<col>[-m<mask>].png in final/cap.
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
F = os.path.dirname(HERE)                 # .../amend2/final
A = os.path.dirname(F)                    # .../amend2
REPO_TOOLS = r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools'
os.environ['A2_ROOT'] = A
sys.path.insert(0, REPO_TOOLS)
import acap  # noqa: E402
import dcap  # noqa: E402,F401  (registers sweepwest/east/west)

REPO = r'C:\Users\danhi\Projects\Personal\postretro'
CAP = os.path.join(F, 'cap')
R40 = 'form=1;ramp=0.4;offset=0.4'
COLS = {
    'head':    (os.path.join(F, 'bin', 'postretro.exe'), os.path.join(F, 'ws', 'content', 'dev', 'maps'), 'head', None),
    'truth':   (os.path.join(F, 'bin', 'postretro.exe'), os.path.join(F, 'ws', 'content', 'dev', 'maps'), 'truth', None),
    'ohead':   (os.path.join(A, 'bin', 'postretro.exe'), os.path.join(A, 'ws', 'content', 'dev', 'maps'), 'head', R40),
    'odark':   (os.path.join(A, 'bin', 'postretro.exe'), os.path.join(A, 'ws', 'content', 'dev', 'maps'), 'dark', R40),
    'odtruth': (os.path.join(A, 'bin', 'postretro.exe'), os.path.join(A, 'ws', 'content', 'dev', 'maps'), 'dtruth', R40),
}
W, H, U = acap.W, acap.H, acap.U


def label(view, t, col, mask=None):
    return f'{view}-t{t:.5f}-{col}' + (f'-m{mask}' if mask else '')


def capture(view, t, col, mask=None):
    spec = acap.VIEWS[view](t)
    name = label(view, t, col, mask)
    out = os.path.join(CAP, name + '.png')
    if os.path.exists(out):
        return out
    binp, maps, bake, proto = COLS[col]
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {'map': os.path.join(maps, f"{bake}-{spec['map']}.prl"),
             'camera': {'position': [-y * U, z * U, -x * U], 'yaw_deg': yaw, 'pitch_deg': pitch, 'fov_deg': fov},
             'resolution': [W, H], 'output': out, 'force_full_resident_sh_compose': True,
             'reflection_quality': 'off', 'force_movers': [{'name': spec['mover'], 't': t}]}
    if mask:
        scene['light_term_mask'] = int(mask, 16)
    sp = os.path.join(CAP, name + '.scene.json')
    with open(sp, 'w', encoding='utf-8') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    env.pop('POSTRETRO_PROTO_MOVER_SH', None)
    env.pop('POSTRETRO_PROTO_ENCLOSED_DARK', None)
    if proto:
        env['POSTRETRO_PROTO_MOVER_SH'] = proto
    r = subprocess.run([binp, '--capture', sp], cwd=REPO, env=env, capture_output=True, text=True,
                       encoding='utf-8', errors='replace')
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {name} failed rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    if proto and 'AMEND2-PROTO' not in r.stderr:
        raise RuntimeError(f'capture {name}: prototype constants not confirmed in stderr')
    if not proto and 'AMEND2-PROTO' in r.stderr:
        raise RuntimeError(f'capture {name}: final engine reported a prototype switch')
    os.remove(sp)
    return out


def parse_ts(s):
    out = []
    for part in s.split('+'):
        out += acap.parse_ts(part)
    return sorted(set(out))


def batch(jobs, view, cols, ts, mask=None):
    tasks = [(view, t, c, mask) for c in cols for t in ts
             if not os.path.exists(os.path.join(CAP, label(view, t, c, mask) + '.png'))]
    st = time.time()
    with ThreadPoolExecutor(jobs) as ex:
        list(ex.map(lambda a: capture(*a), tasks))
    print(f'{view} {",".join(cols)} m{mask}: {len(tasks)} captures in {time.time() - st:.0f}s', flush=True)


if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'batch':
        jobs, view, cols, ts = int(sys.argv[2]), sys.argv[3], sys.argv[4].split(','), parse_ts(sys.argv[5])
        mask = sys.argv[6] if len(sys.argv) > 6 and sys.argv[6] != '-' else None
        batch(jobs, view, cols, ts, mask)
