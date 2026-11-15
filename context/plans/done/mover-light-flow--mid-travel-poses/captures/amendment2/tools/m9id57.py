# Read id 57 (epoch 7) far enough to say, per (mover, probe, pose): bounce stored / base / unmarked,
# the stored tile's mean RGB, and the depth mark and the stored depth tile's mean E[d] (m).
#   import m9id57; ch = m9id57.load(prl)
import struct, sys
import numpy as np
sys.path.insert(0, r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools')
import knots as _kn

TILE_HALVES = 6 * 6 * 3
DEPTH_TEXELS = 10 * 10


class R:
    def __init__(s, b): s.b, s.at = b, 0
    def take(s, n): v = s.b[s.at:s.at + n]; s.at += n; return v
    def u8(s): return s.take(1)[0]
    def u32(s): return struct.unpack('<I', s.take(4))[0]
    def u32s(s, n): return np.frombuffer(s.take(4 * n), '<u4').copy()


def load(prl):
    secs = _kn.sections(prl)
    s = secs[57]
    o34 = struct.unpack_from('<3f', secs[34], 4); d34 = struct.unpack_from('<3I', secs[34], 28)
    r = R(s)
    epoch, af = r.u8(), r.u8()
    adims = struct.unpack('<3I', r.take(12)); dpl, bpl = r.u8(), r.u8()
    ncell = r.u32()
    cells = [struct.unpack('<IQ', r.take(12)) for _ in range(ncell)]
    nm = r.u32(); movers = {}
    for _ in range(nm):
        mid, rs, re_, pc = struct.unpack('<IffI', r.take(16))
        movers[mid] = dict(ramp=(rs, re_), knots=list(struct.unpack(f'<{pc}f', r.take(4 * pc))))
    N = lambda m: len(movers[m]['knots'])
    # direct
    doff = r.u32s(ncell + 1); nrec = int(doff[-1])
    recs = [struct.unpack('<BBHIQ', r.take(16)) for _ in range(nrec)]
    moff = r.u32s(nrec + 1); dmov = r.u32s(int(moff[-1]))
    vis_len = 0; own = 0
    for i, rec in enumerate(recs):
        probes = bin(rec[4]).count('1')
        for m in dmov[moff[i]:moff[i + 1]]:
            vis_len += probes * N(int(m))
        if rec[1] & 1:
            own += probes * TILE_HALVES
    direct = None
    if dpl == 0:
        vis = np.frombuffer(r.take(vis_len), np.uint8); r.take(2 * own)
        direct = (recs, moff, dmov, vis, doff)
    # bounce
    boff = r.u32s(ncell + 1); nb = int(boff[-1])
    eoff = r.u32s(nb + 1); eb = r.take(int(eoff[-1]))
    entries = []
    for i in range(nb):
        sp = eb[eoff[i]:eoff[i + 1]]
        mid = struct.unpack_from('<I', sp, 0)[0]
        pm = [struct.unpack_from('<QQ', sp, 8 + 16 * k) for k in range(N(mid))]
        entries.append((mid, pm))
    tiles = None
    if bpl == 0:
        nt = sum(bin(st).count('1') for _, pm in entries for st, _ in pm)
        tiles = np.frombuffer(r.take(2 * TILE_HALVES * nt), '<f2').astype(np.float32).reshape(nt, 36, 3)
    # depth
    nd = r.u32(); drecs = []
    for _ in range(nd):
        mid, pi = struct.unpack('<II', r.take(8)); n = N(mid)
        words = struct.unpack(f'<{(n + 15) // 16}I', r.take(4 * ((n + 15) // 16)))
        marks = [(words[k // 16] >> (2 * (k % 16))) & 3 for k in range(n)]
        drecs.append((mid, pi, marks))
    ndt = sum(sum(1 for m in mk if m == 1) for _, _, mk in drecs)
    dt = np.frombuffer(r.take(ndt * DEPTH_TEXELS * 4), '<f2').astype(np.float32).reshape(ndt, DEPTH_TEXELS, 2)
    assert r.at == len(s), (r.at, len(s))
    # probe index of (cell, local bit)
    ax, ay, az = adims; nx, ny, nz = d34

    def probe_of(cell, bit):
        cx, cy, cz = cell % ax, (cell // ax) % ay, cell // (ax * ay)
        lx, ly, lz = bit % 4, (bit // 4) % 4, bit // 16
        x, y, zz = cx * 4 + lx, cy * 4 + ly, cz * 4 + lz
        return x + y * nx + zz * nx * ny
    # bounce per (mover, probe): list over poses of ('S', meanrgb) / ('B', None) / ('-', None)
    bounce = {}
    ti = 0
    for pos in range(ncell):
        cell = cells[pos][0]
        for e in range(boff[pos], boff[pos + 1]):
            mid, pm = entries[e]
            n = N(mid)
            per = {}
            for k, (st, ba) in enumerate(pm):
                bits = [b for b in range(64) if st >> b & 1]
                for b in bits:
                    val = None
                    if tiles is not None:
                        val = float(tiles[ti].mean()); ti += 1
                    per.setdefault(b, ['-'] * n)[k] = ('S', val)
                for b in range(64):
                    if ba >> b & 1:
                        per.setdefault(b, ['-'] * n)[k] = ('B', None)
            for b, lst in per.items():
                bounce[(mid, probe_of(cell, b))] = lst
    depth = {}
    di = 0
    for mid, pi, marks in drecs:
        out = []
        for m in marks:
            if m == 1:
                out.append(('S', float(dt[di][:, 0].mean()))); di += 1
            else:
                out.append(('B' if m == 2 else '-', None))
        depth[(mid, pi)] = out

    def pos(pi):
        x, y, zz = pi % nx, (pi // nx) % ny, pi // (nx * ny)
        e = np.array([o34[0] + x, o34[1] + y, o34[2] + zz])   # engine m (cell 1.0 assumed; checked below)
        return np.array([-e[2] / 0.0254, -e[0] / 0.0254, e[1] / 0.0254])
    cs = struct.unpack_from('<3f', secs[34], 16)
    assert cs == (1.0, 1.0, 1.0), cs
    return dict(probe_of=probe_of, cells=cells, epoch=epoch, movers=movers, bounce=bounce, depth=depth, pos=pos, dims=d34,
                direct=direct, bplace=bpl, dplace=dpl)


if __name__ == '__main__':
    ch = load(sys.argv[1])
    print('epoch', ch['epoch'], 'bounce placement', ch['bplace'], 'probes with bounce', len(ch['bounce']),
          'depth records', len(ch['depth']))


def direct_q(ch, mover, probe):
    """Per light record naming `mover` at `probe`: (namespace, light, [q per pose])."""
    recs, moff, dmov, vis, doff = ch['direct']
    out = []
    at = 0
    for pos in range(len(doff) - 1):
        for i in range(doff[pos], doff[pos + 1]):
            ns, fl, _, light, mask = recs[i]
            bits = [b for b in range(64) if mask >> b & 1]
            for m in dmov[moff[i]:moff[i + 1]]:
                n = len(ch['movers'][int(m)]['knots'])
                if int(m) == mover:
                    for slot, b in enumerate(bits):
                        if ch['probe_of'](ch['cells'][pos][0], b) == probe:
                            out.append((ns, light, list(vis[at + slot * n: at + slot * n + n])))
                at += len(bits) * n
    return out
