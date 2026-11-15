# Value side of id 57: direct q (+ own tiles, across marks) and bounce tiles/marks, per global probe.
import struct, numpy as np
from prl57 import u32, u64, sections, grid
def bits(m): return [i for i in range(64) if (m >> i) & 1]
def values57(s, gdims, resident=0):
    assert s[0] == 5
    adims = struct.unpack_from('<3I', s, 2); pl = (s[14], s[15]); at = 16
    L = u32(s, at); at += 4
    cells = [(u32(s, at+12*i), u64(s, at+12*i+4)) for i in range(L)]; at += 12*L
    nmov = u32(s, at); at += 4; poses = {}; knots = {}
    for _ in range(nmov):
        mid = u32(s, at); n = u32(s, at+12); poses[mid] = n; knots[mid] = struct.unpack_from(f'<{n}f', s, at+16); at += 16+4*n
    def gprobe(cell, local):
        cx = cell % adims[0]; cy = (cell // adims[0]) % adims[1]; cz = cell // (adims[0]*adims[1])
        x = cx*4 + local % 4; y = cy*4 + (local // 4) % 4; z = cz*4 + local // 16
        return x + gdims[0]*(y + gdims[1]*z)
    doff = struct.unpack_from(f'<{L+1}I', s, at); at += 4*(L+1); R = doff[-1]
    recs = []
    for i in range(R):
        recs.append((s[at], s[at+1] & 1, u32(s, at+4), u64(s, at+8))); at += 16
    moff = struct.unpack_from(f'<{R+1}I', s, at); at += 4*(R+1)
    dmov = struct.unpack_from(f'<{moff[-1]}I', s, at); at += 4*moff[-1]
    across = {}
    if any(n > 2 for n in poses.values()):
        na = u32(s, at); at += 4
        for _ in range(na):
            pos, mid, k = struct.unpack_from('<3I', s, at); masks = struct.unpack_from(f'<{k}Q', s, at+12); at += 12+8*k
            across[(pos, mid)] = masks
    assert pl[0] == resident and pl[1] == resident, pl
    vis_at = at
    vis_len = sum(bin(recs[i][3]).count('1')*poses[m] for i in range(R) for m in dmov[moff[i]:moff[i+1]])
    own_at = vis_at + vis_len
    # direct[(mover)][probe] -> list of (ns, light, q[N] float 0..1 with nan = across, owntile or None)
    direct = {m: {} for m in poses}
    va = vis_at; oa = own_at
    for pos in range(L):
        cell = cells[pos][0]
        for i in range(doff[pos], doff[pos+1]):
            ns, own, light, mask = recs[i]; bl = bits(mask)
            tiles = None
            if own:
                tiles = np.frombuffer(s, dtype='<f2', count=len(bl)*108, offset=oa).astype(np.float64).reshape(len(bl), 36, 3); oa += len(bl)*216
            for m in dmov[moff[i]:moff[i+1]]:
                N = poses[m]; q = np.frombuffer(s, dtype=np.uint8, count=len(bl)*N, offset=va).reshape(len(bl), N).astype(np.float64)/255; va += len(bl)*N
                am = across.get((pos, m))
                for slot, b in enumerate(bl):
                    qq = q[slot].copy()
                    if am:
                        for k in range(1, N-1):
                            if (am[k-1] >> b) & 1: qq[k] = np.nan
                    direct[m].setdefault(gprobe(cell, b), []).append((ns, light, qq, tiles[slot] if own else None))
    at = oa
    boff = struct.unpack_from(f'<{L+1}I', s, at); at += 4*(L+1); E = boff[-1]
    eoff = struct.unpack_from(f'<{E+1}I', s, at); at += 4*(E+1); eb = at; at = eb + eoff[-1]
    bounce = {m: {} for m in poses}   # mover -> probe -> (marks[N] 0/1/2, {pose: tile(36,3)})
    for pos in range(L):
        cell = cells[pos][0]
        for e in range(boff[pos], boff[pos+1]):
            a = eb + eoff[e]; m = u32(s, a); N = poses[m]
            for k in range(N):
                st, ba = u64(s, a+8+16*k), u64(s, a+16+16*k)
                for b in bits(st):
                    t = np.frombuffer(s, dtype='<f2', count=108, offset=at).astype(np.float64).reshape(36, 3); at += 216
                    g = gprobe(cell, b); r = bounce[m].setdefault(g, ([0]*N, {})); r[0][k] = 1; r[1][k] = t
                for b in bits(ba):
                    g = gprobe(cell, b); r = bounce[m].setdefault(g, ([0]*N, {})); r[0][k] = 2
    assert u32(s, at) < 100000
    return poses, knots, direct, bounce, at
def loadv(path):
    sec = sections(path); g = grid(sec[34])
    fmt = struct.unpack_from('<I', sec[34], 76)[0]
    return g, values57(sec[57], g[2]), fmt
