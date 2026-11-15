# Owner-view capture driver (diagnosis). Views are in Quake map units.
#   python -I ocap.py one <view> <t> <col>
#   python -I ocap.py batch <jobs> <col,col> <view> <t0> <t1> <n>
import json, math, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
F = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = '/Users/dhiester/Projects/Personal/postretro'
MAPS, CAP, BIN = F + '/ws/content/dev/maps', F + '/cap', F + '/bin/postretro'
W, H, UNIT = 960, 540, 0.0254

def lintel(t):   # inside the rising room, looking up at the ceiling above the west door
    fl = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room', pose=((984.0, 236.0, fl + 64.0), 148.0, 38.0, 90.0))
def lintel2(t):  # nearer, steeper
    fl = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room', pose=((930.0, 320.0, fl + 56.0), 180.0, 50.0, 100.0))
def wall(t):     # owner pose north of the sliding wall, looking at its trailing edge
    return dict(map='mover-setpiece', mover='setpiece_wall', pose=((354.0, 1339.0, 79.0), -100.0, 8.0, 90.0))
def wallfix(t):  # camera riding the wall, square on to the trailing two columns, north side
    return dict(map='mover-setpiece', mover='setpiece_wall', pose=((200.0 + 544.0 * t, 1300.0, 112.0), -90.0, 0.0, 80.0))
def wallfixs(t): # same, south side
    return dict(map='mover-setpiece', mover='setpiece_wall', pose=((200.0 + 544.0 * t, 820.0, 112.0), 90.0, 0.0, 80.0))
def sweep(t):    # owner pose east of the sweeper
    return dict(map='enemy-movers', mover='em_sweeper', pose=((1654.0, 197.0, 79.0), 170.0, -5.0, 90.0))
def sweepfix(t): # camera riding the slab, square on to the east face
    return dict(map='enemy-movers', mover='em_sweeper', pose=((1312.0 + 256.0 * t + 150.0, 256.0, 48.0), 180.0, 0.0, 100.0))
VIEWS = dict(lintel=lintel, lintel2=lintel2, wall=wall, wallfix=wallfix, wallfixs=wallfixs, sweep=sweep, sweepfix=sweepfix)

def walledge(t):  # camera riding the wall, in the opening, looking at the trailing end face and the north face beside it
    e = 96.0 + 544.0 * t
    return dict(map='mover-setpiece', mover='setpiece_wall', pose=((e - 110.0, 1190.0, 112.0), -48.0, 0.0, 80.0))
VIEWS['walledge'] = walledge

def label(view, t, col): return f'{view}-t{t:.5f}-{col}'

def capture(view, t, col, extra=None):
    spec = VIEWS[view](t)
    name = label(view, t, col)
    out = os.path.join(CAP, name + '.png')
    if os.path.exists(out): return out
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {'map': os.path.join(MAPS, f"{col}-{spec['map']}.prl"),
             'camera': {'position': [-y * UNIT, z * UNIT, -x * UNIT], 'yaw_deg': yaw, 'pitch_deg': pitch, 'fov_deg': fov},
             'resolution': [W, H], 'output': out, 'force_full_resident_sh_compose': True,
             'reflection_quality': 'off', 'force_movers': [{'name': spec['mover'], 't': t}]}
    if extra: scene.update(extra)
    import re
    mm = re.search(r'M(\d+)$', col)
    if mm:
        scene['light_term_mask'] = int(mm.group(1)); scene['map'] = os.path.join(MAPS, f"{col[:mm.start()]}-{spec['map']}.prl")
    sp = os.path.join(CAP, name + '.scene.json')
    json.dump(scene, open(sp, 'w'), indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    r = subprocess.run(['nice', BIN, '--capture', sp], cwd=REPO, env=env, capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {name} failed rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    os.remove(sp)
    return out

if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'one':
        print(capture(sys.argv[2], float(sys.argv[3]), sys.argv[4]))
    else:
        jobs, cols, view = int(sys.argv[2]), sys.argv[3].split(','), sys.argv[4]
        t0, t1, n = float(sys.argv[5]), float(sys.argv[6]), int(sys.argv[7])
        ts = [round(t0 + (t1 - t0) * i / (n - 1), 5) for i in range(n)]
        tasks = [(view, t, c) for t in ts for c in cols]
        st = time.time()
        with ThreadPoolExecutor(jobs) as ex:
            for i, _ in enumerate(ex.map(lambda a: capture(*a), tasks)):
                pass
        print(f'{len(tasks)} done in {time.time() - st:.0f}s')
