"""Reduce a Metal System Trace `metal-gpu-intervals` export to per-pass GPU time.

usage: gpu_time.py <export.xml[.gz]> [summary.json] [--warmup N --samples N]
Adapted from sh-compose-row-cost-spike/gpu_time.py, which came from
shadow-fill-cost. The parsing is unchanged: one encoder per (command buffer,
encoder); its time is the union of its intervals (Metal nests rows) and a
"Coelasced N Encoders" row counts once. Only labelled passes ("Label:Label")
count; driver rows such as "GPU Execution" are excluded.

Per-frame figures, as the precedent: total pass time divided by the frame
count. The frame count is the indirect compose encoder count when the trace
has compose, else the largest once-per-frame render-pass count. Never take it
from a pass that can be absent (rendering_pipeline.md section 12).

Two additions, both layered on top of that rule (the precedent never parsed a
whole capture, so its traces held no warmup):
- `expected_frames`, from --warmup/--samples, is cross-checked against the
  precedent frame count and flagged when they disagree.
- A steady-state slice. The trace records the whole process, warmup included,
  while the GPU clock ramps. The slice starts at the start time of the
  (warmup+1)th encoder of a once-per-frame reference pass whose count matches
  the expected frame count, and divides each pass's encoder time inside the
  slice by the frames inside the slice. It is absent, never guessed, when no
  reference pass matches.
"""
import collections, gzip, json, re, sys
import xml.etree.ElementTree as E

ONCE_PER_FRAME = ("UI Pass", "Screen Effects Resolve Pass", "Textured Pass", "Depth Pre-Pass")
# Capture renders its tonemap through its own labelled pass (screen_effects.rs).
CAPTURE_ONCE_PER_FRAME = ONCE_PER_FRAME + ("Screen Effects Capture Tonemap Pass",)
COMPOSE_LABELS = ("Streamed SH Compose",)
FLUID_LABEL = "Fluid Surface Pass"
FORWARD_LABEL = "Textured Pass"
# Passes reported beside the fluid pass.
REPORTED = (FLUID_LABEL, FORWARD_LABEL, "Depth Pre-Pass", "Streamed SH Compose",
            "Fog Raymarch Pass", "Fog Composite Pass", "Skinned Mesh Pass",
            "Kinematic Brush Pass", "Screen Effects Capture Tonemap Pass")


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


def reduce_export(src, warmup=None, samples=None):
    root = E.parse(gzip.open(src) if src.endswith(".gz") else src).getroot()
    ids = {e.attrib["id"]: e for e in root.iter() if "id" in e.attrib}

    def res(e):
        return ids[e.attrib["ref"]] if "ref" in e.attrib else e

    # One encoder per (command buffer, encoder). Metal also emits nested rows for
    # an encoder (later start, same end), so an encoder's time is the union of its
    # intervals, never their sum. A "Coelasced N Encoders" row is one interval of
    # one encoder and counts once.
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

    count = collections.Counter()
    total_ns = collections.Counter()
    encoders = collections.defaultdict(list)  # label -> [(first start, union ns)]
    for (label, _, _), intervals in spans.items():
        ns = union_ns(intervals)
        count[label] += 1
        total_ns[label] += ns
        encoders[label].append((min(a for a, _ in intervals), ns))
    for label in encoders:
        encoders[label].sort()

    # Precedent frame count: the compose encoder count, else the largest
    # once-per-frame render-pass count.
    compose_frames = sum(count.get(k, 0) for k in COMPOSE_LABELS)
    frame_pass, frames = max(((p, count.get(p, 0)) for p in ONCE_PER_FRAME), key=lambda x: x[1])
    if compose_frames:
        frame_pass, frames = "Streamed SH Compose", compose_frames
    per_frame = lambda ns, n: round(ns / n / 1e6, 4) if n else None

    result = {
        "source": src,
        "camera_frames": frames,
        "frame_count_pass": frame_pass,
        "once_per_frame_counts": {p: count.get(p, 0) for p in CAPTURE_ONCE_PER_FRAME + COMPOSE_LABELS},
        "sum_labelled_passes_per_frame_ms": per_frame(sum(total_ns.values()), frames),
        "all_passes_per_frame_ms": {k: per_frame(v, frames) for k, v in total_ns.most_common()},
        "encoder_counts": {k: count[k] for k in sorted(count)},
        "reported": {},
        "expected_frames": None,
        "frame_count_matches_expected": None,
        "steady": None,
    }
    for label in REPORTED:
        encs = encoders.get(label, [])
        result["reported"][label] = {
            "encoders": count.get(label, 0),
            "total_ms": round(total_ns.get(label, 0) / 1e6, 3),
            "per_frame_ms": per_frame(total_ns.get(label, 0), frames),
            "per_encoder_ms": per_frame(total_ns.get(label, 0), count.get(label, 0)),
            "per_encoder_median_ms": round(sorted(n for _, n in encs)[len(encs) // 2] / 1e6, 4) if encs else None,
        }

    if warmup is not None and samples is not None:
        expected = warmup + samples
        # The measured frames, plus the final output frame the capture renders.
        tolerance = max(3, expected // 50)
        result["expected_frames"] = expected
        result["frame_count_matches_expected"] = abs(frames - expected) <= tolerance
        reference = next((p for p in COMPOSE_LABELS + CAPTURE_ONCE_PER_FRAME
                          if abs(count.get(p, 0) - expected) <= tolerance), None)
        if reference is None:
            result["steady"] = {"available": False,
                                "reason": f"no once-per-frame pass has {expected}+-{tolerance} encoders",
                                "reference_counts": {p: count.get(p, 0) for p in COMPOSE_LABELS + CAPTURE_ONCE_PER_FRAME}}
        else:
            ref_encoders = encoders[reference]
            t0 = ref_encoders[warmup][0]
            frames_after = sum(1 for start, _ in ref_encoders if start >= t0)
            steady = {"available": True, "reference_pass": reference, "frames": frames_after,
                      "passes": {}}
            for label in REPORTED:
                inside = [n for start, n in encoders.get(label, []) if start >= t0]
                steady["passes"][label] = {
                    "encoders": len(inside),
                    "per_frame_ms": per_frame(sum(inside), frames_after),
                    "per_encoder_ms": per_frame(sum(inside), len(inside)),
                    "per_encoder_median_ms": round(sorted(inside)[len(inside) // 2] / 1e6, 4) if inside else None,
                }
            result["steady"] = steady
    return result


if __name__ == "__main__":
    args = sys.argv[1:]
    warmup = samples = None
    if "--warmup" in args:
        i = args.index("--warmup"); warmup = int(args[i + 1]); del args[i:i + 2]
    if "--samples" in args:
        i = args.index("--samples"); samples = int(args[i + 1]); del args[i:i + 2]
    out = reduce_export(args[0], warmup, samples)
    text = json.dumps(out, indent=2)
    if len(args) > 1:
        open(args[1], "w").write(text + "\n")
    print(text)
