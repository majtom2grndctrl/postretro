# Opus diagnosis capture driver.
#   python -I cap.py <jobs> <view> <col> <t,t,...|lo:hi:step>
# Columns name PRLs maps/<col>-<map>.prl; engine bin/postretro.
import json, math, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

O = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus'
REPO = '/Users/dhiester/Projects/Personal/postretro'
BIN = os.environ.get('CAPBIN', O + '/bin/postretro')
W, H = 960, 540
U = 0.0254


def room_floor(t):
    return -176.0 + 176.0 * t


# pose: map-space (x, y, z), yaw (deg, 0 = +x, 90 = +y), pitch (deg up), fov
VIEWS = {
    # Owner shot 1: inside the rising room, looking up/west at the ceiling over the door.
    'door_ceiling': lambda t: dict(map='mover-setpiece', mover='setpiece_room',
                                   pose=((984.0, 236.0, room_floor(t) + 56.0), 150.0, 35.0, 90.0)),
    # Same, closer and steeper.
    'door_ceiling2': lambda t: dict(map='mover-setpiece', mover='setpiece_room',
                                    pose=((930.0, 300.0, room_floor(t) + 40.0), 180.0, 45.0, 90.0)),
    # Owner shots 2/3: back room, looking at the wall door's north face.
    'wall_back': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                                pose=((354.0, 1339.0, 79.0), -60.0, 5.0, 90.0)),
    'wall_owner': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                                 pose=((354.0, 1339.0, 79.0), -115.0, 12.0, 90.0)),
    'wall_back_wide': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                                     pose=((500.0, 1400.0, 112.0), -90.0, -5.0, 100.0)),
    # Owner shots 4/5: east of the sweeper, looking west at its east face.
    'sweeper_east': lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                   pose=((1654.0, 197.0, 79.0), 168.0, -5.0, 90.0)),
}


MASK = os.environ.get('MASK')


def label(view, t, col):
    return f'{view}-t{t:.5f}-{col}' + (f'-m{MASK}' if MASK else '')


def capture(view, t, col):
    spec = VIEWS[view](t)
    out = os.path.join(O, 'cap', label(view, t, col) + '.png')
    if os.path.exists(out):
        return out
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {
        'map': os.path.join(O, 'ws/content/dev/maps', f"{col}-{spec['map']}.prl"),
        'camera': {'position': [-y * U, z * U, -x * U], 'yaw_deg': yaw,
                   'pitch_deg': pitch, 'fov_deg': fov},
        'resolution': [W, H],
        'output': out,
        'force_full_resident_sh_compose': True,
        'reflection_quality': 'off',
        'force_movers': [{'name': spec['mover'], 't': t}],
    }
    if MASK:
        scene['light_term_mask'] = int(MASK, 16)
    scene_path = out[:-4] + '.scene.json'
    with open(scene_path, 'w') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    r = subprocess.run(['nice', BIN, '--capture', scene_path], cwd=REPO, env=env,
                       capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'{out} rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    os.remove(scene_path)
    return out


def parse_ts(s):
    if ':' in s:
        lo, hi, step = map(float, s.split(':'))
        n = int(round((hi - lo) / step))
        return [round(lo + step * i, 6) for i in range(n + 1)]
    return [float(v) for v in s.split(',')]


if __name__ == '__main__':
    jobs = int(sys.argv[1]); view = sys.argv[2]; cols = sys.argv[3].split(',')
    ts = parse_ts(sys.argv[4])
    tasks = [(view, t, c) for c in cols for t in ts]
    t0 = time.time()
    with ThreadPoolExecutor(jobs) as ex:
        for i, r in enumerate(ex.map(lambda a: capture(*a), tasks)):
            pass
    print(f'{len(tasks)} captures in {time.time() - t0:.0f}s')
