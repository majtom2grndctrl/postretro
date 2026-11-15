# Task 15 motion recapture: motion/tools/mcap.py's five views, with per-column
# (engine, PRL) so the HEAD engine (epoch 6) and the pre-amendment engine
# (epoch 5) render side by side.
#
#   venv/bin/python -I tcap.py one <view> <t> <col>
#   venv/bin/python -I tcap.py batch <jobs> <col> <grid> [view ...]
#     grid: std (mcap's 0.02 / 1/70 grids) | truth (the truth bake's knots:
#           k/40 on the room, k/56 on the sweeper) | flash (the amend room's
#           knots, each +-0.001; ceiling only)
# Columns:
#   amend  HEAD engine + HEAD PRLs (t15/ws)
#   truth  pre-amendment engine + light_poses 41/57 PRLs (advice/ws, epoch 5)
#   head   pre-amendment engine + pre-amendment PRLs (motion/ws); std-grid
#          frames already sit in motion/cap and are read from there
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

SP = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad'
sys.path.insert(0, SP + '/motion/tools')
import mcap  # noqa: E402

T = SP + '/t15'
CAP = T + '/cap'
OLD_CAP = SP + '/motion/cap'
COLS = {
    'amend': (T + '/bin/postretro', T + '/ws/content/dev/maps/amend-{map}.prl'),
    'truth': (SP + '/motion/bin/postretro', SP + '/advice/ws/content/dev/maps/xfine-{map}.prl'),
    'head': (SP + '/motion/bin/postretro', SP + '/motion/ws/content/dev/maps/head-{map}.prl'),
}
ROOM_VIEWS = ('floor', 'ceiling', 'interior')
# The amend room's knots (id 57 mover 1, mover-setpiece), from knots.py.
AMEND_ROOM_KNOTS = [0.0, 0.03002, 0.07413, 0.10696, 0.11601, 0.13917, 0.2, 0.22324, 0.2517, 0.29782,
                    0.33065, 0.3397, 0.36287, 0.4, 0.44694, 0.47539, 0.52152, 0.55435, 0.56339, 0.58656,
                    0.6, 0.67063, 0.69909, 0.74521, 0.77804, 0.78709, 0.8, 0.81026, 0.89433, 0.92278,
                    0.96891, 1.0]


def truth_ts(view):
    n = 40 if view in ROOM_VIEWS else 56
    return [round(k / n, 6) for k in range(n + 1)]


def flash_ts():
    out = set()
    for k in AMEND_ROOM_KNOTS[1:-1]:
        for d in (-0.001, 0.0, 0.001):
            out.add(round(k + d, 6))
    return sorted(out)


def ts_for(view, grid):
    if grid == 'std':
        return mcap.ts_for(view)
    if grid == 'truth':
        return truth_ts(view)
    if grid == 'flash':
        return flash_ts()
    raise SystemExit(f'unknown grid {grid}')


def label(view, t, col):
    return f'{view}-t{t:.6f}-{col}'


def path_for(view, t, col):
    """Existing frame (old 3-decimal label in motion/cap, or ours)."""
    ours = os.path.join(CAP, label(view, t, col) + '.png')
    if os.path.exists(ours):
        return ours
    old = os.path.join(OLD_CAP, mcap.label(view, t, col) + '.png')
    if os.path.exists(old) and round(t, 6) in mcap.ts_for(view):
        return old
    return ours


def capture(view, t, col):
    spec = mcap.VIEWS[view](t)
    out = os.path.join(CAP, label(view, t, col) + '.png')
    if os.path.exists(path_for(view, t, col)):
        return path_for(view, t, col), 0.0
    binary, prl = COLS[col]
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {
        'map': prl.format(map=spec['map']),
        'camera': {'position': [-y * mcap.UNIT, z * mcap.UNIT, -x * mcap.UNIT], 'yaw_deg': yaw,
                   'pitch_deg': pitch, 'fov_deg': fov},
        'resolution': [mcap.W, mcap.H],
        'output': out,
        'force_full_resident_sh_compose': True,
        'reflection_quality': 'off',
        'force_movers': [{'name': spec['mover'], 't': t}],
    }
    name = label(view, t, col)
    scene_path = os.path.join(CAP, name + '.scene.json')
    with open(scene_path, 'w') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    env.pop('POSTRETRO_MOVER_DEPTH_BLEND', None)
    t0 = time.time()
    result = subprocess.run(['nice', binary, '--capture', scene_path], cwd=mcap.REPO, env=env,
                            capture_output=True, text=True)
    dt = time.time() - t0
    if result.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {name} failed rc={result.returncode}\n{result.stdout[-2000:]}\n{result.stderr[-4000:]}')
    if result.stderr.strip():
        with open(os.path.join(CAP, name + '.stderr.txt'), 'w') as f:
            f.write(result.stderr)
    return out, dt


def run_batch(jobs, col, grid, views):
    tasks = [(v, t, col) for v in views for t in ts_for(v, grid)
             if not os.path.exists(path_for(v, t, col))]
    print(f'{len(tasks)} captures to run, {jobs} at a time', flush=True)
    start = time.time()
    done = 0
    with ThreadPoolExecutor(jobs) as ex:
        futs = [ex.submit(capture, *task) for task in tasks]
        try:
            for fut in futs:
                fut.result()
                done += 1
                if done % 50 == 0:
                    el = time.time() - start
                    print(f'{done}/{len(tasks)} {el:.0f}s, eta {el / done * (len(tasks) - done):.0f}s', flush=True)
        except Exception as e:
            for f in futs:
                f.cancel()
            print('ABORT', e, flush=True)
            sys.exit(1)
    print(f'batch done in {time.time() - start:.0f}s', flush=True)


if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'one':
        print(capture(sys.argv[2], float(sys.argv[3]), sys.argv[4]))
    elif sys.argv[1] == 'batch':
        views = sys.argv[5:] or list(mcap.VIEWS)
        if sys.argv[4] == 'flash':
            views = ['ceiling']
        run_batch(int(sys.argv[2]), sys.argv[3], sys.argv[4], views)
