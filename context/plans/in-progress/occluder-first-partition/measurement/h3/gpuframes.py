"""Per-frame GPU busy time from a Metal System Trace of one H3 capture run.

usage: gpuframes.py <metal-gpu-intervals export .xml> [skip_frames]
Frames are delimited by the capture's once-per-frame `Capture Measurement Encoder`
interval (the capture waits for the GPU after every submit, so frames never overlap).
Per frame: busy = union of every postretro GPU interval (all channels, nested and
coalesced rows counted once) inside (previous delimiter end, this delimiter end];
span = first interval start to delimiter end. Prints median / p95 of both over the
frames after skip_frames, the frame count, and the per-frame mean of the top passes.
"""
import collections, re, statistics as st, sys
import xml.etree.ElementTree as ET

tree = ET.parse(sys.argv[1]); skip = int(sys.argv[2]) if len(sys.argv) > 2 else 60
ids = {}
for e in tree.iter():
    if "id" in e.attrib:
        ids[e.attrib["id"]] = e
val = lambda e: ids[e.attrib["ref"]] if "ref" in e.attrib else e
rows = []
for r in tree.iter("row"):
    ch = [val(c) for c in r]
    proc = "".join(c.attrib.get("fmt", "") for c in ch if c.tag == "process")
    if "postretro" not in proc:
        continue
    start = int(ch[0].text); dur = int(ch[1].text); chan = ch[2].attrib.get("fmt")
    name = re.sub(r"\s+0x[0-9a-f]+$", "", ch[6].attrib.get("fmt", "").split(":")[0].strip())
    rows.append((start, start + dur, chan, name))
rows.sort()
delims = sorted(e for s, e, c, n in rows if n.startswith("Capture Measurement Encoder"))
busy, span, per = [], [], collections.defaultdict(float)
j = 0
for i in range(1, len(delims)):
    lo, hi = delims[i - 1], delims[i]
    iv = [(max(s, lo), min(e, hi), n) for s, e, c, n in rows if e > lo and s < hi]
    iv.sort()
    tot, cur_s, cur_e = 0, None, None
    for s, e, n in iv:
        if cur_e is None or s > cur_e:
            if cur_e is not None:
                tot += cur_e - cur_s
            cur_s, cur_e = s, e
        else:
            cur_e = max(cur_e, e)
    if cur_e is not None:
        tot += cur_e - cur_s
    if i > skip:
        busy.append(tot / 1e6); span.append((hi - iv[0][0]) / 1e6 if iv else 0)
        for s, e, n in iv:
            per[n] += (e - s) / 1e6
n = len(busy)
q = lambda v, p: sorted(v)[min(len(v) - 1, int(p * len(v)))]
print(f"frames {n} busy median {st.median(busy):.2f} p95 {q(busy, 0.95):.2f} ms | span median {st.median(span):.2f} p95 {q(span, 0.95):.2f} ms")
for k, v in sorted(per.items(), key=lambda x: -x[1])[:12]:
    print(f"  {k[:48]:48s} {v / n:7.3f} ms/frame")
