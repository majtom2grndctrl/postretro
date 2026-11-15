# Task 5 capture driver: forced-travel captures of the setpiece roof, the
# rising room floor and interior, and em_sweeper's faces, for four columns
# (today | through-base | lerp | stripped), plus the metrics.
#
#   python3 -I cap.py capture   # run every missing capture
#   python3 -I cap.py metrics   # compute metrics -> results.json
#
# Metric rebuild of the predecessor's (probe-channel research §Task 12c, 12d):
# 8x8-pixel blocks wholly inside the surface crop (inset 8 u); a block's excess
# is its value minus the median of the crop blocks within 6 blocks of it
# (Chebyshev). Peaks / pits: local max / min blocks (8-neighbourhood) past
# +3 / -3. 12c reads each frame's own block mean luma; 12d reads each block's
# mean luma minus the stripped frame's (as built minus id 57 stripped) and
# also reports that difference's mean over the crop.
import json, math, os, subprocess, sys

S = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = '/Users/dhiester/Projects/Personal/postretro'
MAPS = os.path.join(S, 'ws/content/dev/maps')
CAP = os.path.join(S, 'cap')
BIN = os.path.join(S, 'bin')
W, H = 960, 540
UNIT = 0.0254
BLOCK = 8

# Columns: binary, PRL prefix, depth-blend env.
COLUMNS = {
    'today': ('postretro-83df', 'today', None),
    'tb': ('postretro-head', 'new', 'through-base'),
    'lerp': ('postretro-head', 'new', 'lerp'),
    'strip-today': ('postretro-83df', 'today-strip', None),
    'strip-new': ('postretro-head', 'new-strip', None),
}
# Diagnostic column, captured on demand only: id 57 with its depth block dropped.
EXTRA = {'nodepth': ('postretro-head', 'new-nodepth', None)}


def roof_view(t):
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((680.0, 320.0, 300.0), 0.0, -30.0, 90.0),
                plane=('z', 176.0 * t), rect=((840.0, 1144.0), (168.0, 472.0)))


def floor_view(t):
    floor = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((860.0, 320.0, floor + 64.0), 0.0, -28.0, 90.0),
                plane=('z', floor), rect=((856.0, 1128.0), (184.0, 456.0)))


def interior_view(t):
    floor = -176.0 + 176.0 * t
    return dict(map='mover-setpiece', mover='setpiece_room',
                pose=((860.0, 320.0, floor + 72.0), 0.0, -8.0, 100.0),
                plane=None, rect=None)


def west_view(t):
    return dict(map='enemy-movers', mover='em_sweeper',
                pose=((1100.0, 256.0, 140.0), 0.0, -20.0, 90.0),
                plane=('x', 1280.0 + 256.0 * t), rect=((72.0, 440.0), (8.0, 88.0)))


def east_view(t):
    return dict(map='enemy-movers', mover='em_sweeper',
                pose=((1700.0, 256.0, 140.0), 180.0, -20.0, 90.0),
                plane=('x', 1312.0 + 256.0 * t), rect=((72.0, 440.0), (8.0, 88.0)))


VIEWS = {
    'roof': (roof_view, [0.3, 0.4, 0.5, 0.6, 0.65, 0.7]),
    'west': (west_view, [0.20, 0.25, 0.28, 0.30, 0.32]),
    'east': (east_view, [0.00, 0.05, 0.10, 0.15, 0.70, 0.75, 0.80, 0.85]),
    'floor': (floor_view, [0.5, 0.6, 0.7, 0.8, 0.86, 0.9, 0.95, 1.0]),
    'interior': (interior_view, [round(0.05 * i, 2) for i in range(21)]),
}


def label(view, t, column):
    return f'{view}-t{t:.2f}-{column}'


def capture(view, t, column):
    make, _ = VIEWS[view]
    spec = make(t)
    binary, prefix, blend = {**COLUMNS, **EXTRA}[column]
    name = label(view, t, column)
    out = os.path.join(CAP, name + '.png')
    if os.path.exists(out):
        return out
    (x, y, z), yaw, pitch, fov = spec['pose']
    scene = {
        'map': os.path.join(MAPS, f"{prefix}-{spec['map']}.prl"),
        'camera': {'position': [-y * UNIT, z * UNIT, -x * UNIT], 'yaw_deg': yaw,
                   'pitch_deg': pitch, 'fov_deg': fov},
        'resolution': [W, H],
        'output': out,
        'force_full_resident_sh_compose': True,
        'reflection_quality': 'off',
        'force_movers': [{'name': spec['mover'], 't': t}],
    }
    scene_path = os.path.join(CAP, name + '.scene.json')
    with open(scene_path, 'w') as f:
        json.dump(scene, f, indent=1)
    env = dict(os.environ, POSTRETRO_SH_STREAMING='sync-proof', RUST_LOG='warn')
    if blend:
        env['POSTRETRO_MOVER_DEPTH_BLEND'] = blend
    else:
        env.pop('POSTRETRO_MOVER_DEPTH_BLEND', None)
    result = subprocess.run([os.path.join(BIN, binary), '--capture', scene_path], cwd=REPO,
                            env=env, capture_output=True, text=True)
    if result.returncode != 0 or not os.path.exists(out):
        sys.exit(f'capture {name} failed\n{result.stdout}\n{result.stderr}')
    if result.stderr.strip():
        with open(os.path.join(CAP, name + '.stderr.txt'), 'w') as f:
            f.write(result.stderr)
    return out


def read_luma(path):
    raw = subprocess.run(['magick', path, '-depth', '8', 'RGB:-'], capture_output=True,
                         check=True).stdout
    assert len(raw) == W * H * 3, path
    return [0.2126 * raw[i] + 0.7152 * raw[i + 1] + 0.0722 * raw[i + 2]
            for i in range(0, len(raw), 3)]


def ray(pose, px, py):
    (cx, cy, cz), yaw, pitch, fov = pose
    sy, cyw = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    sp, cp = math.sin(math.radians(pitch)), math.cos(math.radians(pitch))
    forward = (cyw * cp, sy * cp, sp)
    right = (sy, -cyw, 0.0)
    up = (-cyw * sp, -sy * sp, cp)
    half_x = math.tan(math.radians(fov) / 2.0)
    half_y = half_x * H / W
    nx = 2.0 * px / W - 1.0
    ny = 1.0 - 2.0 * py / H
    return tuple(forward[i] + right[i] * nx * half_x + up[i] * ny * half_y for i in range(3))


def crop_blocks(spec):
    """8x8 blocks whose four corner rays meet the plane inside the inset rect."""
    axis, value = spec['plane']
    a = {'x': 0, 'y': 1, 'z': 2}[axis]
    others = [i for i in range(3) if i != a]
    (u0, u1), (v0, v1) = spec['rect']
    origin = spec['pose'][0]

    def inside(px, py):
        d = ray(spec['pose'], px, py)
        if abs(d[a]) < 1e-9:
            return False
        s = (value - origin[a]) / d[a]
        if s <= 0:
            return False
        p = [origin[i] + s * d[i] for i in range(3)]
        return u0 <= p[others[0]] <= u1 and v0 <= p[others[1]] <= v1

    out = []
    for by in range(H // BLOCK):
        for bx in range(W // BLOCK):
            x0, y0 = bx * BLOCK, by * BLOCK
            if all(inside(x, y) for x in (x0, x0 + BLOCK) for y in (y0, y0 + BLOCK)):
                out.append((bx, by))
    return out


def block_means(luma, blocks):
    means = {}
    for bx, by in blocks:
        total = 0.0
        for y in range(by * BLOCK, by * BLOCK + BLOCK):
            row = y * W
            total += sum(luma[row + bx * BLOCK: row + bx * BLOCK + BLOCK])
        means[(bx, by)] = total / (BLOCK * BLOCK)
    return means


def median(values):
    s = sorted(values)
    n = len(s)
    return s[n // 2] if n % 2 else 0.5 * (s[n // 2 - 1] + s[n // 2])


def excess_metric(values):
    ex = {}
    for (bx, by), v in values.items():
        neigh = [values[(x, y)] for x in range(bx - 6, bx + 7) for y in range(by - 6, by + 7)
                 if (x, y) in values]
        ex[(bx, by)] = v - median(neigh)

    # Peaks / pits: blocks past +3 / -3 excess that are a local max / min of
    # the block values over their 8 neighbours (reproduces the predecessor's
    # roof counts exactly; its floor counts run 2-5 lower).
    def nbrs(b):
        return [values[(b[0] + dx, b[1] + dy)] for dx in (-1, 0, 1) for dy in (-1, 0, 1)
                if (dx or dy) and (b[0] + dx, b[1] + dy) in values]

    peaks = sum(1 for b, e in ex.items() if e > 3 and all(values[b] >= n for n in nbrs(b)))
    pits = sum(1 for b, e in ex.items() if e < -3 and all(values[b] <= n for n in nbrs(b)))
    return dict(bright=max(ex.values()), peaks=peaks, dark=-min(ex.values()), pits=pits,
                excess=ex)


def frame_mean(luma):
    return sum(luma) / len(luma)


def metrics():
    results = {}
    for view, (make, ts) in VIEWS.items():
        for t in ts:
            spec = make(t)
            lumas = {c: read_luma(os.path.join(CAP, label(view, t, c) + '.png')) for c in COLUMNS}
            row = {'t': t, 'frame_mean': {c: frame_mean(l) for c, l in lumas.items()}}
            if spec['plane'] is not None:
                blocks = crop_blocks(spec)
                means = {c: block_means(l, blocks) for c, l in lumas.items()}
                row['blocks'] = len(blocks)
                row['crop_mean'] = {c: sum(m.values()) / len(m) for c, m in means.items()}
                # 12c: each frame's own block means.
                row['12c'] = {}
                for c, m in means.items():
                    e = excess_metric(m)
                    e.pop('excess')
                    row['12c'][c] = e
                # 12d: as built minus its own build's stripped frame.
                row['12d'] = {}
                for c, strip in (('today', 'strip-today'), ('tb', 'strip-new'),
                                 ('lerp', 'strip-new'), ('strip-new', 'strip-today')):
                    diff = {b: means[c][b] - means[strip][b] for b in blocks}
                    e = excess_metric(diff)
                    e.pop('excess')
                    e['mean'] = sum(diff.values()) / len(diff)
                    e['worst_block'] = min(diff.values())
                    row['12d'][c] = e
            # The two stripped frames should agree: other sections unchanged.
            a, b = lumas['strip-today'], lumas['strip-new']
            row['strip_max_luma_delta'] = max(abs(x - y) for x, y in zip(a, b))
            results.setdefault(view, []).append(row)
            print(view, t, json.dumps({k: v for k, v in row.items() if k in ('12d', 'frame_mean')},
                                      default=str)[:300], flush=True)
    with open(os.path.join(S, 'results.json'), 'w') as f:
        json.dump(results, f, indent=1, default=str)


if __name__ == '__main__':
    os.makedirs(CAP, exist_ok=True)
    if sys.argv[1] == 'capture':
        only = sys.argv[2:] or list(VIEWS)
        for view in only:
            _, ts = VIEWS[view]
            for t in ts:
                for column in COLUMNS:
                    capture(view, t, column)
                print('captured', view, t, flush=True)
    elif sys.argv[1] == 'one':
        print(capture(sys.argv[2], float(sys.argv[3]), sys.argv[4]))
    elif sys.argv[1] == 'metrics':
        metrics()
