# Print id 57's mover records (id, ramp, pose count, knots) for PRLs.
import struct, sys


def section57(path):
    data = open(path, 'rb').read()
    version, count = struct.unpack_from('<HH', data, 4)
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        if sid == 57:
            return data[off:off + size]


for p in sys.argv[1:]:
    s = section57(p)
    listed = struct.unpack_from('<I', s, 16)[0]
    at = 20 + 12 * listed
    n = struct.unpack_from('<I', s, at)[0]; at += 4
    print(p.split('/')[-1], 'epoch', s[0], 'listed cells', listed)
    for _ in range(n):
        mid, rs, re_, k = struct.unpack_from('<IffI', s, at); at += 16
        knots = struct.unpack_from(f'<{k}f', s, at); at += 4 * k
        print(f'  mover {mid}: ramp ({rs}, {re_}), {k} poses, knots {[round(x, 5) for x in knots]}')
