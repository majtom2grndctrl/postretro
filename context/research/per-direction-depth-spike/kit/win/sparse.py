#!/usr/bin/env python3
"""Keep real tiles only on Q3-flagged probes: sparse.py <tile.bin> <bits> <bit index> <out.bin>.

Unflagged probes get their mean over texels in every texel, which reads as today's
isotropic moments (the isotropic control reproduced today's frame exactly).
"""
import struct, sys

src, bits_path, bit, out_path = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
b = open(src, "rb").read()
bits = open(bits_path, "rb").read()
t = struct.unpack_from("<I", b, 4)[0]
n = t * t
tile = n * 4
probes = (len(b) - 24) // tile
assert probes == len(bits), (probes, len(bits))
out = bytearray(b[:24])
kept = 0
for p in range(probes):
    raw = b[24 + p * tile:24 + (p + 1) * tile]
    if bits[p] >> bit & 1:
        out += raw
        kept += 1
    else:
        v = struct.unpack(f"<{n * 2}e", raw)
        out += struct.pack("<ee", sum(v[0::2]) / n, sum(v[1::2]) / n) * n
open(out_path, "wb").write(out)
print(f"{out_path}: kept {kept} of {probes} tiles")
