# Amendment 2 helpers: .map movers (axis-aligned brush AABBs, linear paths),
# PRL lattice (id 34 header), and band scans.
import re, struct, math

U = 0.0254          # metres per map unit
CELL_U = 1.0 / U    # one 1 m lattice step in map units (39.3700787)


def parse_map(path):
    text = open(path, encoding='utf-8').read()
    ents, cur, brush, depth = [], None, None, 0
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith('//'):
            continue
        if s == '{':
            depth += 1
            if depth == 1:
                cur = {'kv': {}, 'brushes': []}
            else:
                brush = []
            continue
        if s == '}':
            if depth == 2:
                cur['brushes'].append(brush); brush = None
            else:
                ents.append(cur); cur = None
            depth -= 1
            continue
        if depth == 1:
            m = re.match(r'"([^"]*)"\s+"([^"]*)"', s)
            if m:
                cur['kv'][m.group(1)] = m.group(2)
        elif depth == 2:
            pts = re.findall(r'\(\s*([-\d.eE]+)\s+([-\d.eE]+)\s+([-\d.eE]+)\s*\)', s)
            brush.append([tuple(map(float, p)) for p in pts[:3]])
    return ents


def brush_aabb(brush):
    """AABB of an axis-aligned brush from its planes (each plane's 3 points
    share one coordinate)."""
    lo, hi = [math.inf] * 3, [-math.inf] * 3
    for pts in brush:
        for a in range(3):
            vals = {p[a] for p in pts}
            if len(vals) == 1:
                v = vals.pop()
                lo[a] = min(lo[a], v); hi[a] = max(hi[a], v)
    return lo, hi


def movers(path):
    ents = parse_map(path)
    by_name = {e['kv'].get('name'): e for e in ents}
    out = []
    for e in ents:
        if e['kv'].get('classname') != 'kinematic_mover':
            continue
        wps, name = [], e['kv'].get('path')
        seen = set()
        while name and name in by_name and name not in seen:
            seen.add(name)
            w = by_name[name]
            wps.append(tuple(map(float, w['kv']['origin'].split())))
            name = w['kv'].get('next')
        out.append(dict(name=e['kv'].get('name'), kv=e['kv'], waypoints=wps,
                        brushes=[brush_aabb(b) for b in e['brushes']]))
    return out


def sections(path):
    data = open(path, 'rb').read()
    version, count = struct.unpack_from('<HH', data, 4)
    out = {}
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        out[sid] = data[off:off + size]
    return out


def lattice(path):
    """(origin_map, cell_u, dims_map) — map-space layer origins per axis.
    Engine (ex, ey, ez) = (-y*U, z*U, -x*U)."""
    s = sections(path)[34]
    o = 4
    eo = struct.unpack_from('<3f', s, o); ec = struct.unpack_from('<3f', s, o + 12)
    dims = struct.unpack_from('<3I', s, o + 24)
    # map x = -ez/U, map y = -ex/U, map z = ey/U; layers run along each engine axis.
    return dict(engine_origin=eo, engine_cell=ec, dims=dims)


def layers_map(lat, axis):
    """Map-space coordinates of every lattice layer along map axis 0/1/2."""
    eo, ec, d = lat['engine_origin'], lat['engine_cell'], lat['dims']
    if axis == 0:   # map x = -ez/U
        return [-(eo[2] + k * ec[2]) / U for k in range(d[2])]
    if axis == 1:   # map y = -ex/U
        return [-(eo[0] + k * ec[0]) / U for k in range(d[0])]
    return [(eo[1] + k * ec[1]) / U for k in range(d[1])]


def pose_ts(n):
    return [i / (n - 1) for i in range(n)]
