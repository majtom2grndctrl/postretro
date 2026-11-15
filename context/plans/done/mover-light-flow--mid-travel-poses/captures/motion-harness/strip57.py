# Rewrite a PRL container with id 57 (MoverProbeChannel) changed, every other
# section's bytes and version kept.
#   strip57.py all   src dst   # drop id 57 (mover-absent frame)
#   strip57.py depth src dst   # keep id 57, empty its depth block (u32 0, no tiles)
# The depth block is the tail of the epoch-5 section (wire.rs to_bytes); the
# walk below follows crates/level-format/src/mover_probe_channel/wire*.rs.
import struct, sys

TILE_HALVES = 6 * 6 * 3          # MOVER_IRRADIANCE_TILE_F16_COUNT
DEPTH_TILE_BYTES = 10 * 10 * 4   # bordered 10x10 texels, Rg16Float


def u32(b, at):
    return struct.unpack_from('<I', b, at)[0]


def u64(b, at):
    return struct.unpack_from('<Q', b, at)[0]


def depth_block_start(s):
    assert s[0] == 5, f'epoch {s[0]}'
    direct_res = s[14] == 0  # placement: 0 resident? check below
    placements = (s[14], s[15])
    at = 16
    listed = u32(s, at); at += 4
    at += 12 * listed
    nmov = u32(s, at); at += 4
    poses = {}
    for _ in range(nmov):
        mid = u32(s, at); n = u32(s, at + 12)
        poses[mid] = n
        at += 16 + 4 * n
    # direct
    doff = struct.unpack_from(f'<{listed + 1}I', s, at); at += 4 * (listed + 1)
    nrec = doff[-1]
    recs = []
    for i in range(nrec):
        flags = s[at + 1]; mask = u64(s, at + 8)
        recs.append((flags & 1, bin(mask).count('1')))
        at += 16
    moff = struct.unpack_from(f'<{nrec + 1}I', s, at); at += 4 * (nrec + 1)
    movers = struct.unpack_from(f'<{moff[-1]}I', s, at); at += 4 * moff[-1]
    if any(n > 2 for n in poses.values()):
        na = u32(s, at); at += 4
        for _ in range(na):
            mid = u32(s, at + 4); k = u32(s, at + 8)
            assert k == poses[mid] - 2
            at += 12 + 8 * k
    vis = sum(recs[i][1] * poses[m] for i in range(nrec) for m in movers[moff[i]:moff[i + 1]])
    own = sum(p * TILE_HALVES for f, p in recs if f)
    return placements, poses, at, vis, own


def strip_depth(s):
    placements, poses, at, vis, own = depth_block_start(s)
    PLACE = {0: 'resident', 1: 'streamed'}
    # Placement byte values: confirm from the enum (Resident/Streamed).
    if placements[0] == RESIDENT:
        at += vis + 2 * own
    listed = u32(s, 16)
    boff = struct.unpack_from(f'<{listed + 1}I', s, at); at += 4 * (listed + 1)
    ne = boff[-1]
    eoff = struct.unpack_from(f'<{ne + 1}I', s, at); at += 4 * (ne + 1)
    ebase = at
    tiles = 0
    for i in range(ne):
        a, b = ebase + eoff[i], ebase + eoff[i + 1]
        mid = u32(s, a)
        n = (b - a - 8) // 16
        assert n == poses[mid], (n, poses[mid])
        for k in range(n):
            tiles += bin(u64(s, a + 8 + 16 * k)).count('1')
    at = ebase + eoff[-1]
    if placements[1] == RESIDENT:
        at += 2 * tiles * TILE_HALVES
    # depth block
    nd = u32(s, at)
    p = at + 4
    stored = 0
    dmovers = set()
    for _ in range(nd):
        mid = u32(s, p); n = poses[mid]
        dmovers.add(mid)
        words = -(-n // 16)
        for w in range(words):
            word = u32(s, p + 8 + 4 * w)
            for k in range(16):
                if w * 16 + k < n and (word >> (2 * k)) & 3 == 1:
                    stored += 1
        p += 8 + 4 * words
    assert len(s) - p == stored * DEPTH_TILE_BYTES, (len(s) - p, stored)
    print(f'  depth block: {nd} records, {stored} tiles, {len(s) - at} bytes; movers {sorted(dmovers)}'
          f'; poses {poses}; placements {placements}')
    return s[:at] + struct.pack('<I', 0)


RESIDENT = None


def main():
    global RESIDENT
    mode, src, dst = sys.argv[1:4]
    RESIDENT = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    data = open(src, 'rb').read()
    assert data[:4] == b'PRL\0'
    version, count = struct.unpack_from('<HH', data, 4)
    entries = [struct.unpack_from('<IQQH', data, 8 + 22 * i) for i in range(count)]
    kept = []
    for sid, off, size, ver in entries:
        blob = data[off:off + size]
        if sid == 57:
            if mode == 'all':
                continue
            blob = strip_depth(blob)
        kept.append((sid, blob, ver))
    assert len([e for e in entries if e[0] == 57]) == 1
    out = bytearray(b'PRL\0' + struct.pack('<HH', version, len(kept)))
    offset = 8 + 22 * len(kept)
    for sid, blob, ver in kept:
        out += struct.pack('<IQQH', sid, offset, len(blob), ver)
        offset += len(blob)
    for _, blob, _ in kept:
        out += blob
    open(dst, 'wb').write(out)


main()
