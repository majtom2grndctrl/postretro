# Rewrite a baseline PRL's id-57 epoch byte (5) to the current epoch (7), in place.
# Valid only when every mover record stores two poses (checked here), per the parity test's module doc.
#   python relabel.py <prl> [<prl> ...]
import struct, sys
sys.path.insert(0, __import__('os').path.dirname(__file__))
import knots

FROM, TO = 5, 7
for p in sys.argv[1:]:
    data = bytearray(open(p, 'rb').read())
    version, count = struct.unpack_from('<HH', data, 4)
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        if sid == 57:
            epoch, ms = knots.movers(bytes(data[off:off + size]))
            assert epoch == FROM, (p, epoch)
            assert all(len(k) == 2 for _, _, _, k in ms), (p, [len(k) for _, _, _, k in ms])
            data[off] = TO
            open(p, 'wb').write(data)
            print(p, f'id 57 epoch {FROM} -> {TO}, {len(ms)} two-pose mover(s)')
