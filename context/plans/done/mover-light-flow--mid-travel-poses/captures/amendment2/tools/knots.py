# Print id-57 header facts per mover: epoch, pose count, knots (path t), ramp. Section size.
#   python knots.py <prl> [<prl> ...]
import struct, sys


def sections(path):
    data = open(path, 'rb').read()
    version, count = struct.unpack_from('<HH', data, 4)
    out = {}
    for i in range(count):
        sid, off, size, ver = struct.unpack_from('<IQQH', data, 8 + 22 * i)
        out[sid] = data[off:off + size]
    return out


def movers(s):
    epoch = s[0]
    at = 16
    cells = struct.unpack_from('<I', s, at)[0]
    at += 4 + cells * 12
    n = struct.unpack_from('<I', s, at)[0]; at += 4
    out = []
    for _ in range(n):
        mid, rs, re_, pc = struct.unpack_from('<IffI', s, at); at += 16
        knots = struct.unpack_from(f'<{pc}f', s, at); at += 4 * pc
        out.append((mid, rs, re_, list(knots)))
    return epoch, out


if __name__ == '__main__':
    for p in sys.argv[1:]:
        s = sections(p)
        sec = s.get(57)
        if sec is None:
            print(p, 'no id 57'); continue
        epoch, ms = movers(sec)
        print(f'{p}: id57 {len(sec)} B, epoch {epoch}')
        for mid, rs, re_, k in ms:
            print(f'  mover {mid}: ramp ({rs}, {re_}), {len(k)} poses')
            print('   knots', ' '.join(f'{x:.5f}' for x in k))
