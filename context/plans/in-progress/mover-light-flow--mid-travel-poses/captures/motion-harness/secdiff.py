# List PRL sections whose bytes differ between two files.
import struct, sys, hashlib
def secs(p):
    d = open(p, 'rb').read(); v, n = struct.unpack_from('<HH', d, 4)
    out = {}
    for i in range(n):
        sid, off, size, ver = struct.unpack_from('<IQQH', d, 8 + 22 * i)
        out[sid] = (size, hashlib.sha1(d[off:off + size]).hexdigest())
    return out
a, b = secs(sys.argv[1]), secs(sys.argv[2])
for sid in sorted(set(a) | set(b)):
    if a.get(sid) != b.get(sid):
        print(f'  id {sid}: {a.get(sid, ("-",))[0]} vs {b.get(sid, ("-",))[0]} bytes')
