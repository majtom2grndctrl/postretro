# M9 sightings (2026-11-15): HEAD (amend3/bin engine at dbacdfdf2) against same-rule dense truths.
#   python m9cap.py batch <jobs> <view> <col,col> <t-spec> [mask]
#   t-spec: lo:hi:n | t,t,t | step:lo:hi:dt   (parts joined by '+')
# Columns: head / truth -> amend3/ws/content/dev/maps/<col>-<map>.prl
#   enemy-movers truth: em_sweeper light_poses 113; mover-setpiece truth: room_big 98, room 76, wall 267.
# Frames: m9class/cap/<view>-t<t>-<col>[-m<mask>].png
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

TOOLS = r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools'
sys.path.insert(0, TOOLS)
import acap, blib  # noqa: E402  (acap: sweeper_east etc.; blib registers bw/bs/bfloor/broof/barch/...)

REPO = r'C:\Users\danhi\Projects\Personal\postretro'
SCR = blib.SCR
MAPS = os.path.join(SCR, 'amend3', 'ws', 'content', 'dev', 'maps')
EXE = os.path.join(SCR, 'amend3', 'bin', 'postretro.exe')
CAP = os.environ.get('M9_CAP', os.path.join(SCR, 'm9class', 'cap'))
U = acap.U

# Riding big-room view from the north half, square on to the door (south) wall: arch, jambs,
# south wall face, the ceiling and floor strips beside it. Local position at the lowered pose.
acap.VIEWS['m9door'] = lambda t: dict(map='mover-setpiece', mover='setpiece_room_big',
                                      pose=((1728.0, 1740.0, -196.0 + blib.TRAVEL * t), 270.0, 0.0, 100.0))
# Sweeper: rides the slab, close on the east face's top-south corner (owner's upper left).
acap.VIEWS['m9corner'] = lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                        pose=((1312.0 + 256.0 * t + 96.0, 150.0, 78.0), 200.0, 0.0, 80.0))

# Owner position read as feet rather than eye: eye about 0.6 m higher, above the slab's top (z 96).
acap.VIEWS['m9high'] = lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                      pose=((1654.0, 197.0, 102.0), 168.0, -8.0, 90.0))


def path(view, t, col, mask=None):
    return os.path.join(CAP, f'{view}-t{t:.5f}-{col}' + (f'-m{mask}' if mask else '') + '.png')


def capture(view, t, col, mask=None):
    out = path(view, t, col, mask)
    if os.path.exists(out):
        return out
    spec = acap.VIEWS[view](t)
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {'map': os.path.join(MAPS, f"{col}-{spec['map']}.prl"),
             'camera': {'position': [-y * U, z * U, -x * U], 'yaw_deg': yaw, 'pitch_deg': pitch, 'fov_deg': fov},
             'resolution': [acap.W, acap.H], 'output': out, 'force_full_resident_sh_compose': True,
             'reflection_quality': 'off', 'force_movers': [{'name': spec['mover'], 't': t}]}
    if mask:
        scene['light_term_mask'] = int(mask, 16)
    sp = out[:-4] + '.scene.json'
    with open(sp, 'w', encoding='utf-8') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    env.pop('POSTRETRO_PROTO_MOVER_SH', None)
    env.pop('POSTRETRO_PROTO_ENCLOSED_DARK', None)
    r = subprocess.run([EXE, '--capture', sp], cwd=REPO, env=env, capture_output=True, text=True,
                       encoding='utf-8', errors='replace')
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {out} failed rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    if 'AMEND2-PROTO' in r.stderr:
        raise RuntimeError(f'capture {out}: HEAD engine reported a prototype switch')
    os.remove(sp)
    return out


def parse_ts(s):
    out = []
    for part in s.split('+'):
        if part.startswith('step:'):
            _, lo, hi, dt = part.split(':')
            lo, hi, dt = float(lo), float(hi), float(dt)
            n = int(round((hi - lo) / dt))
            out += [round(min(hi, lo + i * dt), 5) for i in range(n + 1)] + [round(hi, 5)]
        else:
            out += acap.parse_ts(part)
    return sorted(set(out))


def batch(jobs, view, cols, ts, mask=None):
    tasks = [(view, t, c, mask) for c in cols for t in ts if not os.path.exists(path(view, t, c, mask))]
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
