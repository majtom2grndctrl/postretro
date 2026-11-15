# Rewrite a PRL container without section id 57 (MoverProbeChannel), every
# other section's bytes and version kept: the test helper `without_channel`.
import struct, sys
src, dst = sys.argv[1], sys.argv[2]
data = open(src, 'rb').read()
assert data[:4] == b'PRL\0'
version, count = struct.unpack_from('<HH', data, 4)
entries = [struct.unpack_from('<IQQH', data, 8 + 22 * i) for i in range(count)]
kept = [(sid, data[off:off + size], ver) for sid, off, size, ver in entries if sid != 57]
assert len(kept) == count - 1, 'id 57 present exactly once'
out = bytearray(b'PRL\0' + struct.pack('<HH', version, len(kept)))
offset = 8 + 22 * len(kept)
for sid, blob, ver in kept:
    out += struct.pack('<IQQH', sid, offset, len(blob), ver)
    offset += len(blob)
for _, blob, _ in kept:
    out += blob
open(dst, 'wb').write(out)
