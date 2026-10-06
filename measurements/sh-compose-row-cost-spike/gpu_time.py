"""Sum Metal GPU time per postretro pass label from a metal-gpu-intervals export.

Copied from shadow-fill-cost; adds per-encoder compose times for paired A/B runs.

usage: gpu_time.py <export.xml[.gz]> [summary.json]
Rows are deduplicated by (command buffer, encoder, start time); distinct
intervals of one encoder all count. A row Metal reports as "Coelasced N
Encoders" folds into its base label and counts as N encoders. Per-frame
figures divide by the frame count: the largest encoder count among passes
recorded once per frame, since coalescing can only hide encoders, never add
them. Only labelled passes ("Label:Label") count; driver rows such as
"GPU Execution", "GL/CL" and paging are excluded.
"""
import collections, gzip, json, re, sys
import xml.etree.ElementTree as E

ONCE_PER_FRAME = ("UI Pass", "Screen Effects Resolve Pass", "Textured Pass", "Depth Pre-Pass")

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
    raw = res(cols[6]).attrib.get("fmt", "").split("  (")[0]
    if ":" not in raw:
        continue
    label = raw.split(":")[0]
    key = (res(cols[15]).text, res(cols[16]).text, res(cols[0]).text)
    if key in seen:
        continue
    seen.add(key)
    coalesced = re.search(r"Coelasced (\d+) Encoders", raw)
    count[label] += int(coalesced[1]) if coalesced else 1
    total_ns[label] += int(res(cols[1]).text)

frame_pass, frames = max(((p, count.get(p, 0)) for p in ONCE_PER_FRAME), key=lambda x: x[1])
shadow = {k for k in count if "Shadow" in k or "Depth Cache" in k}
per_frame = lambda ns: round(ns / frames / 1e6, 4) if frames else None
result = {
    "source": src,
    "camera_frames": frames,
    "frame_count_pass": frame_pass,
    "once_per_frame_counts": {p: count.get(p, 0) for p in ONCE_PER_FRAME},
    "shadow_passes": {k: {"count": count[k], "total_ms": round(total_ns[k] / 1e6, 3),
                          "per_frame_ms": per_frame(total_ns[k])} for k in sorted(shadow)},
    "shadow_total_per_frame_ms": per_frame(sum(total_ns[k] for k in shadow)),
    "sum_labelled_passes_per_frame_ms": per_frame(sum(total_ns.values())),
    "all_passes_per_frame_ms": {k: per_frame(v) for k, v in total_ns.most_common()},
    # Paired A/B runs alternate compose pipelines per frame, so each compose
    # label runs in about half the frames: its time per encoder is the arm's
    # per-frame pass time.
    "compose_per_encoder_ms": {k: round(total_ns[k] / count[k] / 1e6, 4)
                               for k in sorted(count) if "SH Compose" in k or "Animated Direct SH" in k},
    "compose_encoder_counts": {k: count[k] for k in sorted(count) if "SH Compose" in k or "Animated Direct SH" in k},
}
text = json.dumps(result, indent=2)
if len(sys.argv) > 2:
    open(sys.argv[2], "w").write(text + "\n")
print(text)
