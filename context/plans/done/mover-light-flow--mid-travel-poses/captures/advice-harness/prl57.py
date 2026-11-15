# Read-only parser: id 34 header+moments, id 59 tiles, id 57 depth records.
import struct, numpy as np
TILE_HALVES = 108; DT = 400
def u32(b, at): return struct.unpack_from('<I', b, at)[0]
def u64(b, at): return struct.unpack_from('<Q', b, at)[0]
def sections(path):
    data = open(path, 'rb').read()
    version, count = struct.unpack_from('<HH', data, 4)
    out = {}
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        out[sid] = data[off:off + size]
    return out
def grid(s34):
    o = struct.unpack_from('<3f', s34, 4); c = struct.unpack_from('<3f', s34, 16); d = struct.unpack_from('<3I', s34, 28)
    n = d[0]*d[1]*d[2]
    rec = np.frombuffer(s34, dtype=np.uint8, count=n*8, offset=84).reshape(n, 8)
    mean = np.frombuffer(rec[:,1:3].tobytes(), dtype='<f2').astype(np.float64)
    msq = np.frombuffer(rec[:,3:5].tobytes(), dtype='<f2').astype(np.float64)
    return o, c, d, rec[:,0].copy(), mean, msq
def tiles59(s59):
    if s59 is None: return {}
    count = u32(s59, 12)
    idx = struct.unpack_from(f'<{count}I', s59, 16)
    t = np.frombuffer(s59, dtype='<f2', count=count*200, offset=16+4*count).astype(np.float64).reshape(count, 100, 2)
    return {p: t[i] for i, p in enumerate(idx)}
def depth57(s, resident):
    assert s[0] == 5
    placements = (s[14], s[15])
    at = 16
    listed = u32(s, at); at += 4 + 12 * listed
    nmov = u32(s, at); at += 4
    poses = {}; knots = {}
    for _ in range(nmov):
        mid = u32(s, at); n = u32(s, at + 12)
        poses[mid] = n; knots[mid] = struct.unpack_from(f'<{n}f', s, at+16)
        at += 16 + 4 * n
    doff = struct.unpack_from(f'<{listed + 1}I', s, at); at += 4 * (listed + 1)
    nrec = doff[-1]; recs = []
    for i in range(nrec):
        recs.append((s[at + 1] & 1, bin(u64(s, at + 8)).count('1'))); at += 16
    moff = struct.unpack_from(f'<{nrec + 1}I', s, at); at += 4 * (nrec + 1)
    movers = struct.unpack_from(f'<{moff[-1]}I', s, at); at += 4 * moff[-1]
    if any(n > 2 for n in poses.values()):
        na = u32(s, at); at += 4
        for _ in range(na):
            mid = u32(s, at + 4); k = u32(s, at + 8); at += 12 + 8 * k
    vis = sum(recs[i][1] * poses[m] for i in range(nrec) for m in movers[moff[i]:moff[i + 1]])
    own = sum(p * TILE_HALVES for f, p in recs if f)
    if placements[0] == resident: at += vis + 2 * own
    boff = struct.unpack_from(f'<{listed + 1}I', s, at); at += 4 * (listed + 1)
    ne = boff[-1]
    eoff = struct.unpack_from(f'<{ne + 1}I', s, at); at += 4 * (ne + 1)
    ebase = at; tl = 0
    for i in range(ne):
        a, b = ebase + eoff[i], ebase + eoff[i + 1]
        n = (b - a - 8) // 16
        for k in range(n): tl += bin(u64(s, a + 8 + 16 * k)).count('1')
    at = ebase + eoff[-1]
    if placements[1] == resident: at += 2 * tl * TILE_HALVES
    nd = u32(s, at); p = at + 4
    records = []; stored = 0
    for _ in range(nd):
        mid = u32(s, p); probe = u32(s, p+4); n = poses[mid]
        words = -(-n // 16); marks = []
        for k in range(n):
            marks.append((u32(s, p + 8 + 4 * (k // 16)) >> (2 * (k % 16))) & 3)
        stored += marks.count(1)
        records.append([mid, probe, marks]); p += 8 + 4 * words
    assert len(s) - p == stored * DT, (len(s) - p, stored * DT)
    t = np.frombuffer(s, dtype='<f2', count=stored*200, offset=p).astype(np.float64).reshape(stored, 100, 2)
    i = 0; out = {}
    for mid, probe, marks in records:
        tl = {}
        for k, m in enumerate(marks):
            if m == 1: tl[k] = t[i]; i += 1
        out[(mid, probe)] = (marks, tl)
    return poses, knots, out
def load(path):
    sec = sections(path)
    g = grid(sec[34]); t59 = tiles59(sec.get(59))
    for r in (0, 1):
        try:
            d = depth57(sec[57], r); break
        except AssertionError as e: err = e
    else: raise err
    return g, t59, d
