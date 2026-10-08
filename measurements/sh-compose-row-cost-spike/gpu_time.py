"""Sum Metal GPU time per postretro pass label from a metal-gpu-intervals export.

usage: gpu_time.py <export.xml[.gz]> [summary.json]
Copied from shadow-fill-cost, then corrected in review. One encoder per
(command buffer, encoder): its time is the union of its intervals (Metal nests
rows), and a "Coelasced N Encoders" row counts once. Compose labels report
time per encoder (the spike's metric). Per-frame figures divide by the
indirect compose encoder count, falling back to the largest once-per-frame
render-pass count only for traces without compose. Only labelled passes
("Label:Label") count; driver rows such as "GPU Execution" are excluded.
"""
import collections, gzip, json, re, sys
import xml.etree.ElementTree as E

ONCE_PER_FRAME = ("UI Pass", "Screen Effects Resolve Pass", "Textured Pass", "Depth Pre-Pass")

src = sys.argv[1]
root = E.parse(gzip.open(src) if src.endswith(".gz") else src).getroot()
ids = {e.attrib["id"]: e for e in root.iter() if "id" in e.attrib}


def res(e):
    return ids[e.attrib["ref"]] if "ref" in e.attrib else e


# One encoder per (command buffer, encoder). Metal also emits nested rows for
# an encoder (later start, same end), so an encoder's time is the union of its
# intervals, never their sum. A "Coelasced N Encoders" row is one interval of
# one encoder and counts once (spike review: the old code added nested rows and
# multiplied coalesced ones).
spans = collections.defaultdict(list)  # (label, cb, encoder) -> [(start, end)]
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
    begin = int(res(cols[0]).text)
    spans[(label, res(cols[15]).text, res(cols[16]).text)].append((begin, begin + int(res(cols[1]).text)))


def union_ns(intervals):
    total, cur_start, cur_end = 0, None, None
    for a, b in sorted(intervals):
        if cur_end is None or a > cur_end:
            if cur_end is not None:
                total += cur_end - cur_start
            cur_start, cur_end = a, b
        else:
            cur_end = max(cur_end, b)
    return total + (cur_end - cur_start if cur_end is not None else 0)


count = collections.Counter()
total_ns = collections.Counter()
per_encoder = collections.defaultdict(list)
for (label, _, _), intervals in spans.items():
    ns = union_ns(intervals)
    count[label] += 1
    total_ns[label] += ns
    per_encoder[label].append(ns)

# Frames: every frame dispatches the indirect compose pass exactly once (A or
# [B] in a paired run), so its encoder count is the frame count. The old
# heuristic (largest render-pass encoder count) undercounted whenever render
# encoders were missing from part of a trace, inflating every per-frame value
# together. It is kept only as a fallback for traces without compose.
compose_frames = count.get("Streamed SH Compose", 0) + count.get("Streamed SH Compose [B]", 0)
frame_pass, frames = max(((p, count.get(p, 0)) for p in ONCE_PER_FRAME), key=lambda x: x[1])
if compose_frames:
    frame_pass, frames = "Streamed SH Compose (+ [B])", compose_frames
shadow = {k for k in count if "Shadow" in k or "Depth Cache" in k}
per_frame = lambda ns: round(ns / frames / 1e6, 4) if frames else None
compose = sorted(k for k in count if "SH Compose" in k or "Animated Direct SH" in k)
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
    # Each compose label's time per encoder is the arm's per-frame pass time:
    # the metric every spike delta uses (no frame denominator involved).
    "compose_per_encoder_ms": {k: round(total_ns[k] / count[k] / 1e6, 4) for k in compose},
    "compose_per_encoder_median_ms": {k: round(sorted(per_encoder[k])[len(per_encoder[k]) // 2] / 1e6, 4)
                                      for k in compose},
    "compose_encoder_counts": {k: count[k] for k in compose},
}
text = json.dumps(result, indent=2)
if len(sys.argv) > 2:
    open(sys.argv[2], "w").write(text + "\n")
print(text)
