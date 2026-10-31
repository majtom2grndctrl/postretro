"""Reduce a Metal System Trace `metal-gpu-intervals` export to per-pass GPU time per frame.

usage: gpu_time.py <export.xml[.gz]> [summary.json] [--warmup N --samples N]

Adapted from measurements/fluid-base-pass-cost/metal/gpu_time.py. The parsing is
unchanged: one encoder per (command buffer, encoder); its time is the union of its
intervals (Metal nests rows) and a "Coelasced N Encoders" row counts once. Only
labelled passes ("Label:Label") of the postretro process count; driver rows such as
"GPU Execution" are excluded.

What differs: the passes. This kit times the mover depth pre-pass change, so the
figures are Depth Pre-Pass, Textured Pass (the main forward pass), Kinematic Brush
Pass (the mover colour pass), SDF Shadow Pass (reads the pre-pass depth), their sum,
and the whole-frame GPU time (the union of every labelled interval in a frame).

Frame count: never from a render-pass encoder count (rendering_pipeline.md section 12;
context/plans/drafts/gpu-pass-paired-ab/index.md). Frames are bounded by a compute
frame marker whose rows the trace never drops (BVH Cull Pass, else Candidate Cull
Pass). The timed span is the last `samples` framed frames (the capture renders a few
frames before its warmup counter, plus the output frame, which has no successor and
is dropped).

A steady frame counts as complete when each tracked pass (TRACKED) appears in it
exactly as often as that pass's modal per-frame count over the steady frames, and
Depth Pre-Pass and Textured Pass each appear exactly once. The modal counts are
reported, so an arm whose mover pass is absent (mode 0) or present (mode 1) is
visible in the output. Each pass's figure is its mean per-frame time over complete
frames. Cross-check: the label-independent GPU frame time over complete and over
incomplete frames should agree; when it does, dropped rows lost labels, not time.
"""
import bisect, collections, gzip, json, sys
import xml.etree.ElementTree as E

DEPTH = "Depth Pre-Pass"
FORWARD = "Textured Pass"
MOVER = "Kinematic Brush Pass"
SDF = "SDF Shadow Pass"
TRACKED = (DEPTH, FORWARD, MOVER, SDF)
# Passes summed into `sum_ms`: the ones the change can move.
SUMMED = TRACKED
ALWAYS_ONCE = (DEPTH, FORWARD)
FRAME_MARKERS = ("BVH Cull Pass", "Candidate Cull Pass")
MIN_COMPLETE_FRAMES = 30


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


def encoder_rows(src):
    root = E.parse(gzip.open(src) if str(src).endswith(".gz") else src).getroot()
    ids = {e.attrib["id"]: e for e in root.iter() if "id" in e.attrib}

    def res(e):
        return ids[e.attrib["ref"]] if "ref" in e.attrib else e

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
    rows = []
    for (label, _, _), intervals in spans.items():
        rows.append((min(a for a, _ in intervals), label, intervals, union_ns(intervals)))
    rows.sort()
    return rows


def stats(values_ns):
    v = sorted(values_ns)
    if not v:
        return None
    ms = lambda x: round(x / 1e6, 4)
    return {"per_frame_ms": ms(sum(v) / len(v)), "median_ms": ms(v[len(v) // 2]),
            "p10_ms": ms(v[len(v) // 10]), "p90_ms": ms(v[(len(v) * 9) // 10])}


def reduce_export(src, warmup=None, samples=None):
    rows = encoder_rows(src)
    count = collections.Counter(label for _, label, _, _ in rows)
    marker = next((m for m in FRAME_MARKERS if count[m] >= 2), None)
    out = {"source": str(src), "encoder_counts": dict(sorted(count.items())), "marker": marker,
           "expected_frames": None, "frame_count_ok": None}
    if marker is None:
        out.update(available=False, reason=f"no frame marker among {FRAME_MARKERS}")
        return out
    marks = sorted(start for start, label, _, _ in rows if label == marker)
    out["marker_count"] = len(marks)
    if warmup is not None and samples is not None:
        expected = warmup + samples
        out["expected_frames"] = expected
        # A few frames render before the warmup counter starts (7-13 seen), plus
        # the output frame: never fewer than expected, never many more.
        out["frame_count_ok"] = expected <= len(marks) <= expected + max(20, expected // 20)
    frames = [{"labels": collections.Counter(), "ns": collections.Counter(), "intervals": []}
              for _ in range(len(marks) - 1)]
    for start, label, intervals, ns in rows:
        i = bisect.bisect_right(marks, start) - 1
        if 0 <= i < len(frames):
            f = frames[i]
            f["labels"][label] += 1
            f["ns"][label] += ns
            f["intervals"].extend(intervals)
    first = max(0, len(frames) - samples) if samples else (warmup or 0)
    steady = frames[first:]
    out["framed_frames"] = len(frames)
    out["steady_frames"] = len(steady)
    if not steady:
        out.update(available=False, reason="no steady frames")
        return out
    modes = {}
    for label in TRACKED:
        c = collections.Counter(f["labels"][label] for f in steady)
        modes[label] = c.most_common(1)[0][0]
    out["modal_counts"] = modes
    out["per_frame_count_histogram"] = {
        label: dict(sorted(collections.Counter(f["labels"][label] for f in steady).items())) for label in TRACKED}
    ok = lambda f: (all(f["labels"][k] == modes[k] for k in TRACKED)
                    and all(f["labels"][k] == 1 for k in ALWAYS_ONCE))
    complete = [f for f in steady if ok(f)]
    incomplete = [f for f in steady if not ok(f)]
    out["complete_frames"] = len(complete)
    out["available"] = len(complete) >= MIN_COMPLETE_FRAMES
    if not out["available"]:
        out["reason"] = f"{len(complete)} complete steady frames (< {MIN_COMPLETE_FRAMES})"
    frame_ns = lambda fs: [union_ns(f["intervals"]) for f in fs]
    out["gpu_frame_complete"] = stats(frame_ns(complete))
    out["gpu_frame_incomplete"] = stats(frame_ns(incomplete))
    out["passes"] = {}
    if complete:
        for label in sorted({k for f in complete for k in f["ns"]}):
            out["passes"][label] = stats([f["ns"][label] for f in complete])
        for label in TRACKED:
            out["passes"].setdefault(label, stats([0 for _ in complete]))
        out["sum"] = stats([sum(f["ns"][k] for k in SUMMED) for f in complete])
    return out


if __name__ == "__main__":
    args = sys.argv[1:]
    warmup = samples = None
    if "--warmup" in args:
        i = args.index("--warmup"); warmup = int(args[i + 1]); del args[i:i + 2]
    if "--samples" in args:
        i = args.index("--samples"); samples = int(args[i + 1]); del args[i:i + 2]
    result = reduce_export(args[0], warmup, samples)
    text = json.dumps(result, indent=2)
    if len(args) > 1:
        open(args[1], "w").write(text + "\n")
    print(text)
