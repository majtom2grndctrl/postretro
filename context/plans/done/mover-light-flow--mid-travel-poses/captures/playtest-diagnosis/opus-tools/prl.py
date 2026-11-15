# Minimal PRL readers: section table, id-34 grid + probe metadata, id-57 depth records.
import struct, sys
U = 0.0254
def sections(path):
    data = open(path, 'rb').read()
    version, count = struct.unpack_from('<HH', data, 4)
    out = {}
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        out[sid] = data[off:off + size]
    return out
def grid(path):
    s = sections(path)[34]
    o = 4
    origin = struct.unpack_from('<3f', s, o); cell = struct.unpack_from('<3f', s, o + 12)
    dims = struct.unpack_from('<3I', s, o + 24)
    stride = struct.unpack_from('<I', s, o + 36)[0]
    hdr = 84
    n = dims[0] * dims[1] * dims[2]
    recs = []
    for i in range(n):
        b = s[hdr + i * stride: hdr + (i + 1) * stride]
        recs.append(b[0])
    return origin, cell, dims, recs
def probe_world_map(origin, cell, dims, idx):
    x = idx % dims[0]; y = (idx // dims[0]) % dims[1]; z = idx // (dims[0] * dims[1])
    e = [origin[0] + x * cell[0], origin[1] + y * cell[1], origin[2] + z * cell[2]]
    # engine (ex, ey, ez) = (-y_map*U, z_map*U, -x_map*U)
    return (-e[2] / U, -e[0] / U, e[1] / U)
def map_to_index(origin, cell, dims, p):
    ex, ey, ez = -p[1] * U, p[2] * U, -p[0] * U
    ix = round((ex - origin[0]) / cell[0]); iy = round((ey - origin[1]) / cell[1]); iz = round((ez - origin[2]) / cell[2])
    return ix + iy * dims[0] + iz * dims[0] * dims[1]
def u32(b, a): return struct.unpack_from('<I', b, a)[0]
def depth_records(path):
    """{(mover, probe): marks list (0 unmarked, 1 stored, 2 base)}"""
    sys.path.insert(0, '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus/tools')
    import importlib.util
    spec = importlib.util.spec_from_file_location('s57', '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus/tools/strip57lib.py')
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    return m.depth_records(sections(path)[57])
