# Big-room re-capture driver (2026-11-13): fcap.py's settings on the new mover-setpiece.
#   python bcap.py batch <jobs> <view> <col,col> <t-spec> [mask]
#   t-spec: lo:hi:n | t,t,t | trip[:step] (whole trip at 60 fps frame spacing) | parts joined by '+'
# Columns (PRLs in amend3/ws/content/dev/maps/<col>-mover-setpiece.prl):
#   head   HEAD engine (amend3/bin), HEAD bake
#   truth  HEAD engine, dense bake (wall 267, room 76, room_big 98)
#   ramp   prototype engine (amend2/bin) + POSTRETRO_PROTO_MOVER_SH r40, f712cfc14 pre-dark bake
# Labels: <view>-t<t>-<col>[-m<mask>].png in amend3/cap.
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acap, blib  # noqa: E402

REPO = r'C:\Users\danhi\Projects\Personal\postretro'
MAPS = os.path.join(blib.A3, 'ws', 'content', 'dev', 'maps')
R40 = 'form=1;ramp=0.4;offset=0.4'
COLS = {
    'head': (os.path.join(blib.A3, 'bin', 'postretro.exe'), 'head', None),
    'truth': (os.path.join(blib.A3, 'bin', 'postretro.exe'), 'truth', None),
    'ramp': (os.path.join(blib.SCR, 'amend2', 'bin', 'postretro.exe'), 'ramp', R40),
}
CAP = blib.CAP


def capture(view, t, col, mask=None):
    spec = acap.VIEWS[view](t)
    out = blib.path(view, t, col, mask)
    if os.path.exists(out):
        return out
    binp, bake, proto = COLS[col]
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {'map': os.path.join(MAPS, f"{bake}-{spec['map']}.prl"),
             'camera': {'position': [-y * acap.U, z * acap.U, -x * acap.U], 'yaw_deg': yaw, 'pitch_deg': pitch,
                        'fov_deg': fov},
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
    if proto:
        env['POSTRETRO_PROTO_MOVER_SH'] = proto
    r = subprocess.run([binp, '--capture', sp], cwd=REPO, env=env, capture_output=True, text=True,
                       encoding='utf-8', errors='replace')
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f'capture {out} failed rc={r.returncode}\n{r.stdout[-1500:]}\n{r.stderr[-3000:]}')
    if proto and 'AMEND2-PROTO' not in r.stderr:
        raise RuntimeError(f'capture {out}: prototype constants not confirmed in stderr')
    if not proto and 'AMEND2-PROTO' in r.stderr:
        raise RuntimeError(f'capture {out}: HEAD engine reported a prototype switch')
    os.remove(sp)
    return out


def parse_ts(s):
    out = []
    for part in s.split('+'):
        if part.startswith('trip'):
            step = int(part.split(':')[1]) if ':' in part else 1
            out += blib.trip_ts(step)
        else:
            out += acap.parse_ts(part)
    return sorted(set(out))


def batch(jobs, view, cols, ts, mask=None):
    tasks = [(view, t, c, mask) for c in cols for t in ts if not os.path.exists(blib.path(view, t, c, mask))]
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
