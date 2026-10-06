"""Compare two spike atlas dumps (dirs holding indirect.bin, direct.bin, dims.json).

usage: compare_atlas.py <baseline-dir> <arm-dir>
Prints, per atlas: byte-identical or not, differing texels, and the maximum
absolute deviation over RGB as rgba16float values (dims.json is not read).
A size mismatch reports identical=false with no texel diff. Exit 0 when every
atlas is identical, 1 otherwise; capture.sh relies on it.
"""
import json, struct, sys
from pathlib import Path

a, b = Path(sys.argv[1]), Path(sys.argv[2])
result = {}
for name in ("indirect", "direct"):
    pa, pb = a / f"{name}.bin", b / f"{name}.bin"
    if not pa.exists() and not pb.exists():
        continue
    da, db = pa.read_bytes(), pb.read_bytes()
    entry = {"bytes": len(da), "identical": da == db}
    if da != db and len(da) == len(db):
        diff_texels, max_dev = 0, 0.0
        for off in range(0, len(da), 8):
            ta, tb = da[off:off + 8], db[off:off + 8]
            if ta != tb:
                diff_texels += 1
                va, vb = struct.unpack("<4e", ta), struct.unpack("<4e", tb)
                max_dev = max(max_dev, max(abs(x - y) for x, y in zip(va[:3], vb[:3])))
        entry.update(diff_texels=diff_texels, texels=len(da) // 8, max_abs_dev_rgb=max_dev)
    result[name] = entry
print(json.dumps(result, indent=2))
sys.exit(0 if result and all(e["identical"] for e in result.values()) else 1)
