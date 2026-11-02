#!/usr/bin/env python3
"""Sum the build-cache entry bytes written during the SH Bake stage window.

The SH base stage writes per-light patch layers as each finishes and the id-34
volume entry at its end, so entries whose mtime falls inside the stage window
(from the timestamped stderr log) are that stage's. The largest entry in the
window is the id-34 volume entry; the rest are patch layers (and any small
SH-stage keys). Entries are hash-named; this is the only attribution available.
usage: cache-window.py <cache-dir> <epoch0> <t_sh_start> <t_sh_end>
  epoch0 = unix time the prl-build process started (run-bake-ts.sh / warm-edit.sh record it)"""
import os, sys, json
d, e0, a, b = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4])
inw, total = [], 0
for n in os.listdir(d):
    p = os.path.join(d, n)
    if not os.path.isfile(p): continue
    st = os.stat(p); total += st.st_size
    if e0 + a - 1 <= st.st_mtime <= e0 + b + 1.5: inw.append(st.st_size)
inw.sort(reverse=True)
# Other stages write into the same window (campaign-test: ~1400 entries of one identical
# size). Entries are hash-named, so attribute by size: a size shared by more than 50
# entries is not a patch layer. The remaining entries are the id-34 entry (largest) and layers.
from collections import Counter
cnt = Counter(inw)
cand = [x for x in inw if cnt[x] <= 50]
print(json.dumps({"cache_total_bytes": total, "entries_total": len(os.listdir(d)),
  "sh_window_entries": len(inw), "sh_window_bytes": sum(inw),
  "heuristic_largest_entry_bytes (id-34 volume entry)": cand[0] if cand else None,
  "heuristic_layer_entries": max(len(cand) - 1, 0),
  "heuristic_layer_bytes": sum(cand[1:])}, indent=1))
