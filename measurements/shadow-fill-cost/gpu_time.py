"""Sum Metal GPU time per postretro pass label from a metal-gpu-intervals export.

usage: gpu_time.py <export.xml[.gz]> [summary.json]
Rows are deduplicated by (command buffer, encoder). Coalesced-encoder rows fold
into their base label. Per-frame figures divide by the Textured Pass count, an
approximate camera-frame count at trace boundaries.
"""
import collections, gzip, json, sys
import xml.etree.ElementTree as E

src = sys.argv[1]
root = E.parse(gzip.open(src) if src.endswith(".gz") else src).getroot()
ids = {e.attrib["id"]: e for e in root.iter() if "id" in e.attrib}


def res(e):
    return ids[e.attrib["ref"]] if "ref" in e.attrib else e


seen = set()
count = collections.Counter()
total_ns = collections.Counter()
for row in root.iter("row"):
    cols = list(row)
    if len(cols) < 17 or cols[10].tag == "sentinel":
        continue
    if "postretro (" not in res(cols[10]).attrib.get("fmt", ""):
        continue
    label = res(cols[6]).attrib.get("fmt", "").split(":")[0].split("  (")[0]
    key = (res(cols[15]).text, res(cols[16]).text, label)
    if key in seen:
        continue
    seen.add(key)
    count[label] += 1
    total_ns[label] += int(res(cols[1]).text)

frames = count.get("Textured Pass", 0)
shadow = {k for k in count if "Shadow" in k or "Depth Cache" in k}
per_frame = lambda ns: round(ns / frames / 1e6, 4) if frames else None
result = {
    "source": src,
    "camera_frames": frames,
    "shadow_passes": {k: {"count": count[k], "total_ms": round(total_ns[k] / 1e6, 3),
                          "per_frame_ms": per_frame(total_ns[k])} for k in sorted(shadow)},
    "shadow_total_per_frame_ms": per_frame(sum(total_ns[k] for k in shadow)),
    "all_passes_per_frame_ms": {k: per_frame(v) for k, v in total_ns.most_common()},
}
text = json.dumps(result, indent=2)
if len(sys.argv) > 2:
    open(sys.argv[2], "w").write(text + "\n")
print(text)
