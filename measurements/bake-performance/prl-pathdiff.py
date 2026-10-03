# Compare two .prl bakes of the same map from different checkouts.
# The .prl embeds the absolute data-script path, so bytes differ by the
# checkout directory name. Prints the path in each file, whether everything
# after it is identical, and every differing byte run before it (expected:
# section-table offsets and the path's length prefix, each off by the path
# length difference).
# Usage: python3 prl-pathdiff.py <a.prl> <b.prl>
import sys

a = open(sys.argv[1], "rb").read()
b = open(sys.argv[2], "rb").read()
pa = a.index(b"\\\\?\\C:\\Users")
pb = b.index(b"\\\\?\\C:\\Users")
ea = a.index(b"stress-warren.ts", pa) + len(b"stress-warren.ts")
eb = b.index(b"stress-warren.ts", pb) + len(b"stress-warren.ts")
print("path a:", a[pa:ea].decode(), "\npath b:", b[pb:eb].decode())
print("path start equal:", pa == pb, "len diff:", (ea - pa) - (eb - pb), "file len diff:", len(a) - len(b))
print("tail after path identical:", a[ea:] == b[eb:], "tail bytes:", len(a) - ea)

# Bytes before the path: list differing runs.
runs = []
i = 0
n = pa
while i < n:
    if a[i] != b[i]:
        j = i
        while j < n and a[j] != b[j]:
            j += 1
        runs.append((i, j))
        i = j
    else:
        i += 1
print("differing runs before path:", len(runs))
for s, e in runs[:40]:
    print("  ", s, e - s, a[s:e].hex(), b[s:e].hex())
if len(runs) > 40:
    print("  ...", runs[-3:])
# Interpret each run as part of a little-endian integer: align to 8 bytes around it.
deltas = set()
for s, e in runs:
    for w in (4, 8):
        base = s - (s % w)
        if base + w <= n:
            va = int.from_bytes(a[base:base + w], "little")
            vb = int.from_bytes(b[base:base + w], "little")
            deltas.add((w, va - vb))
print("integer deltas (width, a-b):", sorted(deltas))
