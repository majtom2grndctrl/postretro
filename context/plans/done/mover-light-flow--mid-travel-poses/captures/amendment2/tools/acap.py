# Amendment 2 capture driver (Windows port of fable-tools/ocap.py and
# opus-tools/cap.py views; scratch root from A2_ROOT or this file's parent).
#   python acap.py batch <jobs> <view> <bake,bake> <form,form> <t-spec> [mask] [attr]
#   t-spec: lo:hi:n (n samples inclusive) or t,t,t
# A column is (bake, form). Bakes name PRLs ws/content/dev/maps/<bake>-<map>.prl.
# Forms name POSTRETRO_PROTO_MOVER_SH specs (FORMS below).
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

A = os.environ.get('A2_ROOT', os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
REPO = os.environ.get('A2_REPO', r'C:\Users\danhi\Projects\Personal\postretro')
MAPS = os.path.join(A, 'ws', 'content', 'dev', 'maps')
CAP = os.path.join(A, 'cap')
BIN = os.environ.get('A2_BIN', os.path.join(A, 'bin', 'postretro.exe'))
W, H, U = 960, 540, 0.0254

FORMS = {
    'cur': '',                                   # today: cosine from the lookup point, 0.1 cell
    'r10': 'form=1;ramp=0.1;offset=0.1',         # surface-point ramp, 0.1 cell offset
    'r25': 'form=1;ramp=0.25;offset=0.25',       # + 0.25 cell offset
    'r40': 'form=1;ramp=0.4;offset=0.4',         # + 0.4 cell offset
    'o25': 'offset=0.25',                        # today's cosine, 0.25 cell offset alone
    'o40': 'offset=0.4',                         # today's cosine, 0.4 cell offset alone
}

def room_floor(t): return -176.0 + 176.0 * t

VIEWS = {
    # Opus door_ceiling (owner shot 1): rides the room, looks up/west at the ceiling over the door.
    'door_ceiling': lambda t: dict(map='mover-setpiece', mover='setpiece_room',
                                   pose=((984.0, 236.0, room_floor(t) + 56.0), 150.0, 35.0, 90.0)),
    # Fable sweepfix: rides the slab, square on to the east face.
    'sweepfix': lambda t: dict(map='enemy-movers', mover='em_sweeper',
                               pose=((1312.0 + 256.0 * t + 150.0, 256.0, 48.0), 180.0, 0.0, 100.0)),
    # Opus sweeper_east (owner shots 4/5).
    'sweeper_east': lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                   pose=((1654.0, 197.0, 79.0), 168.0, -5.0, 90.0)),
    # Fable walledge: rides the wall, in the opening, trailing end face + north face beside it.
    'walledge': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                               pose=((96.0 + 544.0 * t - 110.0, 1190.0, 112.0), -48.0, 0.0, 80.0)),
    # Fable wallfix: rides the wall, square on to the trailing two columns, north side.
    'wallfix': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                              pose=((200.0 + 544.0 * t, 1300.0, 112.0), -90.0, 0.0, 80.0)),
    # Opus wall_owner (owner shots 2/3).
    'wall_owner': lambda t: dict(map='mover-setpiece', mover='setpiece_wall',
                                 pose=((354.0, 1339.0, 79.0), -115.0, 12.0, 90.0)),
    # motion-harness floor view (task 12c's eighth-power metric).
    'floor': lambda t: dict(map='mover-setpiece', mover='setpiece_room',
                            pose=((860.0, 320.0, room_floor(t) + 64.0), 0.0, -28.0, 90.0)),
}


def label(view, t, bake, form, mask=None, attr=None):
    s = f'{view}-t{t:.5f}-{bake}-{form}'
    if mask: s += f'-m{mask}'
    if attr: s += f'-a{attr}'
    return s


ATTRS = {}  # name -> 'attr=..;min=..;max=..' (filled by callers via A2_ATTRS json)
if os.environ.get('A2_ATTRS'):
    ATTRS.update(json.load(open(os.environ['A2_ATTRS'], encoding='utf-8')))


def capture(view, t, bake, form, mask=None, attr=None):
    spec = VIEWS[view](t)
    name = label(view, t, bake, form, mask, attr)
    out = os.path.join(CAP, name + '.png')
    if os.path.exists(out):
        return out
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {'map': os.path.join(MAPS, f"{bake}-{spec['map']}.prl"),
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
    proto = FORMS[form]
    if attr:
        a = ATTRS[attr]
        if isinstance(a, dict):   # per-t specs (t-dependent AABBs), '*' as fallback
            a = a.get(f'{t:.5f}', a.get('*'))
        proto = ';'.join(p for p in (proto, a) if p)
    if proto:
        env['POSTRETRO_PROTO_MOVER_SH'] = proto
    r = subprocess.run([BIN, '--capture', sp], cwd=REPO, env=env, capture_output=True, text=True,
                       encoding='utf-8', errors='replace')
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {name} failed rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    if proto and 'AMEND2-PROTO' not in r.stderr:
        raise RuntimeError(f'capture {name}: prototype constants not confirmed in stderr')
    if os.path.exists(sp): os.remove(sp)
    return out


def parse_ts(s):
    if ':' in s:
        lo, hi, n = s.split(':')
        lo, hi, n = float(lo), float(hi), int(n)
        return [round(lo + (hi - lo) * i / (n - 1), 5) for i in range(n)]
    return [round(float(v), 5) for v in s.split(',')]


def batch(jobs, view, bakes, forms, ts, mask=None, attrs=(None,)):
    tasks = [(view, t, b, f, mask, a) for b in bakes for f in forms for a in attrs for t in ts
             if not os.path.exists(os.path.join(CAP, label(view, t, b, f, mask, a) + '.png'))]
    if os.environ.get('A2_REVERSE'):
        tasks = tasks[::-1]
    st = time.time()
    with ThreadPoolExecutor(jobs) as ex:
        list(ex.map(lambda a: capture(*a), tasks))
    print(f'{view}: {len(tasks)} captures in {time.time() - st:.0f}s', flush=True)


if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'batch':
        jobs, view = int(sys.argv[2]), sys.argv[3]
        bakes, forms, ts = sys.argv[4].split(','), sys.argv[5].split(','), parse_ts(sys.argv[6])
        mask = sys.argv[7] if len(sys.argv) > 7 and sys.argv[7] != '-' else None
        attrs = sys.argv[8].split(',') if len(sys.argv) > 8 else [None]
        batch(jobs, view, bakes, forms, ts, mask, attrs)
