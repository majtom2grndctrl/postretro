"""Reduce a Metal System Trace `metal-gpu-intervals` export to per-pass GPU time.

usage: gpu_time.py <export.xml[.gz]> [summary.json] [--warmup N --samples N] [--cell-pass]
Copied from measurements/reflection-blend-cost-spike/gpu_time.py (branch
claude/probe-streaming-brief-364a5e) for the cell-rate spike; the one addition
is the Fluid Reflection Cell Pass (a compute pass) and a per-frame `cost`,
fluid surface plus cell pass, over complete frames. Before that it was adapted
from sh-compose-row-cost-spike/gpu_time.py, which came from shadow-fill-cost. The parsing is unchanged: one encoder per (command buffer,
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

Primary figure: the complete-frame reduction (`complete`). At low resolution
the trace drops some frames' Textured Pass row, and in those frames the
interval labelled Fluid Surface Pass is really forward's, so dividing a pass's
total by the frame count understates fluid (an owner 720p run read 4.2-4.7 ms
against a true ~6.0 ms). Frames are bounded by the BVH Cull Pass encoders (a
compute pass that never drops rows). A steady frame (index >= warmup) counts
only when it holds exactly one encoder each of Depth Pre-Pass, Textured Pass
and Fluid Surface Pass. Each pass's figure is its mean per-frame time over
those complete frames. Cross-check: the union of every labelled interval in a
frame does not depend on labels, so its mean over complete and over incomplete
frames should agree; when it does, the dropped rows lost labels, not time.
"""
import collections, gzip, json, re, sys
import xml.etree.ElementTree as E

ONCE_PER_FRAME = ("UI Pass", "Screen Effects Resolve Pass", "Textured Pass", "Depth Pre-Pass")
# Capture renders its tonemap through its own labelled pass (screen_effects.rs).
CAPTURE_ONCE_PER_FRAME = ONCE_PER_FRAME + ("Screen Effects Capture Tonemap Pass",)
COMPOSE_LABELS = ("Streamed SH Compose",)
FLUID_LABEL = "Fluid Surface Pass"
# The cell-rate spike's compute pass, recorded before the fluid pass in arms
# with the cell buffer on.
CELL_LABEL = "Fluid Reflection Cell Pass"
# Frame marker for the complete-frame reduction: a compute pass whose rows the
# trace never drops. Mac run (owner's 5300M): e24 and reflection-blend-spike
# record no BVH Cull Pass; Candidate Cull Pass is once per frame on both. The
# first label with at least two encoders is used.
FRAME_MARKERS = ("BVH Cull Pass", "Candidate Cull Pass")
COMPLETE_REQUIRED = ("Depth Pre-Pass", "Textured Pass", "Fluid Surface Pass")
# A complete-frame figure needs at least this many complete steady frames.
MIN_COMPLETE_FRAMES = 30
FORWARD_LABEL = "Textured Pass"
# Passes reported beside the fluid pass.
REPORTED = (FLUID_LABEL, CELL_LABEL, FORWARD_LABEL, "Depth Pre-Pass", "Streamed SH Compose",
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


def reduce_export(src, warmup=None, samples=None, cell_pass=False):
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
    encoder_rows = []  # (first start, label, intervals, union ns)
    for (label, _, _), intervals in spans.items():
        ns = union_ns(intervals)
        count[label] += 1
        total_ns[label] += ns
        encoders[label].append((min(a for a, _ in intervals), ns))
        encoder_rows.append((min(a for a, _ in intervals), label, intervals, ns))
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
        # Mac run: under sync-proof e24 composes SH 5 times, so the compose count
        # is no frame count. Check the frame marker instead: the capture renders a
        # few frames before its warmup counter (7-13 seen), plus the output frame.
        marker = next((m for m in FRAME_MARKERS if count.get(m, 0) >= 2), None)
        if marker:
            result["frame_count_marker"] = {"pass": marker, "count": count[marker]}
            result["frame_count_matches_expected"] = expected <= count[marker] <= expected + max(20, expected // 20)
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
    result["complete"] = complete_frames(encoder_rows, warmup, samples, cell_pass)
    return result


def complete_frames(encoder_rows, warmup, samples=None, cell_pass=False):
    """The complete-frame reduction (module docstring). With `cell_pass`, a
    complete frame also holds exactly one Fluid Reflection Cell Pass encoder,
    and `cost` is fluid surface plus cell pass per frame; without it, a frame
    holding any cell pass encoder is incomplete (the arm must not run one)."""
    present = collections.Counter(label for _, label, _, _ in encoder_rows)
    FRAME_MARKER = next((m for m in FRAME_MARKERS if present[m] >= 2), FRAME_MARKERS[0])
    marks = sorted(start for start, label, _, _ in encoder_rows if label == FRAME_MARKER)
    if len(marks) < 2:
        return {"available": False, "reason": f"fewer than two encoders of any of {FRAME_MARKERS}"}
    import bisect
    # Frame i spans [marks[i], marks[i+1]); the last mark opens the capture's
    # output frame, which has no successor and is dropped.
    frames = [{"labels": collections.Counter(), "ns": collections.Counter(), "intervals": []}
              for _ in range(len(marks) - 1)]
    for start, label, intervals, ns in encoder_rows:
        i = bisect.bisect_right(marks, start) - 1
        if 0 <= i < len(frames):
            f = frames[i]
            f["labels"][label] += 1
            f["ns"][label] += ns
            f["intervals"].extend(intervals)
    # Mac run: the capture renders a few frames before its warmup counter, so
    # the timed span is the last `samples` framed frames, not frames[warmup:].
    first = max(0, len(frames) - samples) if samples else (warmup or 0)
    steady = frames[first:]
    if not steady:
        return {"available": False, "reason": f"no frames after warmup {first} ({len(frames)} framed)"}
    want_cells = 1 if cell_pass else 0
    flags = [all(f["labels"][k] == 1 for k in COMPLETE_REQUIRED) and f["labels"][CELL_LABEL] == want_cells
             for f in steady]
    complete = [f for f, ok in zip(steady, flags) if ok]
    incomplete = [f for f, ok in zip(steady, flags) if not ok]
    mean = lambda xs: round(sum(xs) / len(xs) / 1e6, 4) if xs else None
    gpu_frame = lambda fs: mean([union_ns(f["intervals"]) for f in fs])
    out = {
        "available": len(complete) >= MIN_COMPLETE_FRAMES,
        "marker": FRAME_MARKER,
        "required": list(COMPLETE_REQUIRED) + ([CELL_LABEL] if cell_pass else []),
        "cell_pass": cell_pass,
        "framed_frames": len(frames),
        "steady_frames": len(steady),
        "complete_frames": len(complete),
        "gpu_frame_ms_complete": gpu_frame(complete),
        "gpu_frame_ms_incomplete": gpu_frame(incomplete),
        "passes": {},
    }
    if not out["available"]:
        out["reason"] = f"{len(complete)} complete steady frames (< {MIN_COMPLETE_FRAMES})"
    if complete:
        per = sorted(f["ns"][FLUID_LABEL] + f["ns"][CELL_LABEL] for f in complete)
        out["cost"] = {
            "per_frame_ms": mean(per),
            "median_ms": round(per[len(per) // 2] / 1e6, 4),
            "p10_ms": round(per[len(per) // 10] / 1e6, 4),
            "p90_ms": round(per[(len(per) * 9) // 10] / 1e6, 4),
        }
    labels = sorted({k for f in complete for k in f["ns"]})
    for label in labels:
        per = sorted(f["ns"][label] for f in complete)
        out["passes"][label] = {
            "per_frame_ms": mean(per),
            "median_ms": round(per[len(per) // 2] / 1e6, 4),
            "p10_ms": round(per[len(per) // 10] / 1e6, 4),
            "p90_ms": round(per[(len(per) * 9) // 10] / 1e6, 4),
        }
    return out


if __name__ == "__main__":
    args = sys.argv[1:]
    warmup = samples = None
    if "--warmup" in args:
        i = args.index("--warmup"); warmup = int(args[i + 1]); del args[i:i + 2]
    if "--samples" in args:
        i = args.index("--samples"); samples = int(args[i + 1]); del args[i:i + 2]
    cells = "--cell-pass" in args
    if cells:
        args.remove("--cell-pass")
    out = reduce_export(args[0], warmup, samples, cells)
    text = json.dumps(out, indent=2)
    if len(args) > 1:
        open(args[1], "w").write(text + "\n")
    print(text)
