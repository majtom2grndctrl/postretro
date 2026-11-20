#!/usr/bin/env python3
"""Join a camera-route playback record with a Metal System Trace export.

A playback run (`--camera-route`) writes a per-frame record
(`postretro.camera_route_playback.v1`) and wraps each frame's drain and frame
encoders in a `pr-frame <k>` debug group, where k is the record row's `frame`.
This reducer joins the record with an `xctrace export` of the run's
`metal-application-intervals` and `metal-gpu-intervals` tables, and reports per
frame and per run. See `context/lib/rendering_pipeline.md` §12 (GPU Pass
Timing: Without timestamp support, Pin the GPU clock).

The join
--------
* In `metal-application-intervals`, a command buffer row (encoder id all ones)
  labelled "Streamed Lightmap Drain", "Streamed SH Upload Copies" or
  "Frame Encoder" has a child encoder row on the same command buffer whose
  label carries `pr-frame <k>`. A mark on the command buffer row's own label
  counts the same way. xctrace's own "Frame N" is never k.
* `metal-gpu-intervals` rows (any channel: SDMA0, GFX, ...) join to that
  marked encoder by encoder id.
* Attribution is by mark only, never by time overlap: a drain recorded on
  frame k is frame k's, however late the GPU runs it, including after frame
  k + 1's work begins (pin P6).
* A frame's GPU time per resource is the union of all its matched GPU
  intervals across channels and encoders, so nested and coalesced rows count
  once (§12). The drain total is the union of the lightmap and SH intervals.

Matched frames (pin P7)
-----------------------
Frame k's encoding is covered when k is the record's first row, or the
trace holds frame k - 1's "Frame Encoder" mark, or row k - 1 is a no-draw row
(which encodes no frame encoder) whose own encoding is covered. A frame's
drains are encoded after frame k - 1's frame encoder, so a covered frame's
drains are all in the trace; the first marked frame of a trace attached
mid-run, and any frame after a stretch of missing rows, is not covered. A
hold or route frame is matched when it is covered and the trace holds its own
"Frame Encoder" mark. A no-draw frame is matched when it is covered and the
trace marks some later frame, so it did not end before the frame's drains.
Unmatched frames leave every per-run figure and are counted and named; an
unmatched frame never counts as zero drain GPU time. Hold frames before the
trace attached are not an error. A record with no matched frame at all is an
error (exit 2).

Within a matched frame, a resource's drain GPU time reads `missing`, not
zero, when one of its marked encoders has no GPU row (the trace ended before
the GPU ran it) or when the record says the frame admitted items for that
resource but the trace holds no encoder for it. Such frames leave the drain
statistics and are counted.

Per-run figures (pins P8, P10)
------------------------------
Interval p50, p95, max (nearest rank) and frames over budget cover matched
route rows only, and leave out rows carrying a change mark or a gap. Hold,
no-draw and marked rows are reported apart with their own counts. Drain GPU
statistics cover matched, unmarked route frames; warm, cold and pool-growth
drains are counted and summarized apart. A minimal record (the Task 4 slice)
has no drain flags or marks: flags read `unknown`, never false, and the
output says the marks were not recorded.

Over-budget charge (both readings)
----------------------------------
For a measured route frame k whose interval exceeds the budget:

* own: the drain GPU time of the drains marked k.
* own + in flight: own, plus the whole drain GPU time of every earlier frame
  j < k whose drain GPU intervals (lightmap or SH) end after the start of
  frame k's interval window. That window, in trace time, is
  [B(k) - interval_k, B(k)], where B(k) is the start of frame k's earliest
  marked encoding row and interval_k the record's interval (time since the
  previous world frame began). The window start therefore stands for the
  previous frame's beginning; a drain still on the GPU after it was in
  flight while the over-budget interval ran.

GPU clock
---------
Every GPU figure names the clock it ran at. With `--ioreg <log>` (a poll
loop printing a timestamp line, then `"Core Clock(MHz)"=<n>` and friends, as
`mac-checkpoint.md` gives), polls join the trace by wall time: the export's
table of contents (`--toc`, or a `<trace-toc>` file in the trace dir) gives
the trace's start date, so trace time t maps to start + t. The clock is the
median, min and max of every non-zero `Core Clock(MHz)` reading in polls
inside the matched route frames' trace span (integer-second poll stamps,
from BSD `date`, widen the span by one second at its start). A zero or
missing reading is unrecorded. With no table of contents the time bases
cannot be aligned: the reducer says so and reports the clock over the whole
poll log, labelled as such. Without `--ioreg` the clock reads unrecorded:
the engine records none in-process (`gpu_clock: unrecorded`). An unrecorded
clock never reads as pinned.

Standard library only. Usage:

    python3 tools/playback_trace_reduce.py --record <playback.json> \\
        --trace-dir <dir of exported tables, .xml or .xml.xz> \\
        [--toc <toc.xml>] [--ioreg <ioreg log>] [--budget-ms 16.667] \\
        [--all-frames] [--json]
"""

import argparse
import json
import lzma
import math
import re
import statistics
import sys
import xml.etree.ElementTree as ET
from datetime import datetime
from pathlib import Path

APP_SCHEMA = "metal-application-intervals"
GPU_SCHEMA = "metal-gpu-intervals"
ALL_ONES = 0xFFFFFFFFFFFFFFFF

LIGHTMAP = "lightmap"
SH = "sh"
FRAME = "frame"
OTHER = "other"
DRAINS = (LIGHTMAP, SH)
KIND_LABELS = (
    (LIGHTMAP, "Streamed Lightmap Drain"),
    (SH, "Streamed SH Upload Copies"),
    (FRAME, "Frame Encoder"),
)

MARK = re.compile(r"pr-frame (\d+)\b")
SCHEMA_NAME = re.compile(r'<schema name="([^"]+)"')
TIMESTAMP = re.compile(r"^\s*(\d+)(?:\.(\d+|N))?\s*$")
READING = re.compile(r'"(Core Clock\(MHz\)|Temperature\(C\)|Device Utilization %)"=(\d+)')

# Only these column tags are ever read, so only they are cached for refs.
CACHED_TAGS = frozenset({
    "start-time", "duration", "formatted-label", "metal-nesting-level",
    "metal-command-buffer-id", "gpu-channel-name", "metal-event-name",
})

DEFAULT_BUDGET_MS = 1000.0 / 60.0


class ReduceError(Exception):
    """A failure the CLI reports as an error and a non-zero exit."""


# ---------------------------------------------------------------- trace input


def open_table(path):
    path = Path(path)
    if path.name.endswith(".xz"):
        return lzma.open(path, "rb")
    return path.open("rb")


def sniff(path):
    """The schema a table export holds, `toc` for a table of contents, or None."""
    with open_table(path) as handle:
        head = handle.read(16384).decode("utf-8", errors="replace")
    if "<trace-toc" in head:
        return "toc"
    match = SCHEMA_NAME.search(head)
    return match.group(1) if match else None


def find_exports(trace_dir):
    """Map schema -> file for the exports in `trace_dir`, plus `toc` if present."""
    found = {}
    for path in sorted(Path(trace_dir).iterdir()):
        if not path.is_file() or not (path.name.endswith(".xml") or path.name.endswith(".xml.xz")):
            continue
        kind = sniff(path)
        if kind is not None and kind not in found:
            found[kind] = path
    return found


def iter_rows(handle):
    """Yield each table row as {mnemonic: (fmt, text) or None}.

    xctrace de-duplicates values: an element with `id` defines a value that
    later elements, top-level or nested, name with `ref`. Sentinel cells are
    empty and read None.
    """
    columns = []
    cache = {}
    node = None
    for event, el in ET.iterparse(handle, events=("start", "end")):
        if event == "start":
            if el.tag == "node":
                node = el
            continue
        if el.tag == "mnemonic":
            columns.append((el.text or "").strip())
            continue
        if el.tag != "row":
            continue
        values = {}
        for name, cell in zip(columns, el):
            values[name] = cell_value(cell, cache)
        yield values
        if node is not None:
            node.remove(el)
        el.clear()


def cell_value(cell, cache):
    ref = cell.get("ref")
    if ref is not None:
        return cache.get(ref)
    if cell.tag == "sentinel":
        return None
    for sub in cell.iter():
        ident = sub.get("id")
        if ident is not None and sub.tag in CACHED_TAGS:
            cache[ident] = (sub.get("fmt"), (sub.text or "").strip())
    return (cell.get("fmt"), (cell.text or "").strip())


def as_int(value):
    if value is None:
        return None
    try:
        return int(value[1])
    except (TypeError, ValueError):
        return None


def as_label(value):
    if value is None:
        return ""
    return value[0] if value[0] is not None else value[1]


def classify(*labels):
    for label in labels:
        for kind, text in KIND_LABELS:
            if text in label:
                return kind
    return OTHER


def short_label(label):
    return label.split("  (")[0].strip()


def read_marks(handle):
    """Marked encoder rows from a `metal-application-intervals` export."""
    parents = {}
    marked = []
    for row in iter_rows(handle):
        label = as_label(row.get("event-label"))
        cmdbuffer = as_int(row.get("cmdbuffer-id"))
        encoder = as_int(row.get("encoder-id"))
        start = as_int(row.get("start"))
        if encoder == ALL_ONES and cmdbuffer is not None:
            parents.setdefault(cmdbuffer, []).append((start or 0, label))
        match = MARK.search(label)
        if match is None or start is None:
            continue
        marked.append({
            "frame": int(match.group(1)),
            "start": start,
            "end": start + (as_int(row.get("duration")) or 0),
            "cmdbuffer": cmdbuffer,
            "encoder": encoder,
            "label": label,
        })
    for mark in marked:
        parent_label = ""
        for start, label in parents.get(mark["cmdbuffer"], ()):
            if start <= mark["start"]:
                parent_label = label
        mark["kind"] = classify(mark["label"], parent_label)
        mark["command_buffer"] = short_label(parent_label or mark["label"])
    return marked


def read_gpu(handle, encoders):
    """GPU intervals (start, end, channel) per wanted encoder id."""
    out = {}
    for row in iter_rows(handle):
        encoder = as_int(row.get("encoder-id"))
        if encoder not in encoders:
            continue
        start = as_int(row.get("start"))
        duration = as_int(row.get("duration"))
        if start is None or duration is None:
            continue
        channel = as_label(row.get("channel-name"))
        out.setdefault(encoder, []).append((start, start + duration, channel))
    return out


def read_trace(app_path, gpu_path):
    with open_table(app_path) as handle:
        marks = read_marks(handle)
    # Encoder ids join GPU rows; the all-ones id names a command buffer, never
    # an encoder, so a mark on a command buffer row joins nothing.
    encoders = {m["encoder"] for m in marks if m["encoder"] not in (None, ALL_ONES)}
    with open_table(gpu_path) as handle:
        gpu = read_gpu(handle, encoders)
    return marks, gpu


def read_toc_start(path):
    """The trace's start date as epoch seconds, or None."""
    with open_table(path) as handle:
        text = handle.read().decode("utf-8", errors="replace")
    match = re.search(r"<start-date>([^<]+)</start-date>", text)
    if match is None:
        return None
    try:
        return datetime.fromisoformat(match.group(1).strip()).timestamp()
    except ValueError:
        return None


# ---------------------------------------------------------------- intervals


def union_ns(intervals):
    """Busy time of the union of (start, end) intervals."""
    total = 0
    cur_start = cur_end = None
    for start, end in sorted(intervals):
        if cur_end is None or start > cur_end:
            if cur_end is not None:
                total += cur_end - cur_start
            cur_start, cur_end = start, end
        else:
            cur_end = max(cur_end, end)
    if cur_end is not None:
        total += cur_end - cur_start
    return total


def nearest_rank(values, pct):
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, math.ceil(pct * len(ordered) / 100))
    return ordered[rank - 1]


def stats(values):
    """p50, p95, max (nearest rank) and total of a list of numbers."""
    if not values:
        return {"n": 0, "p50": None, "p95": None, "max": None, "total": None}
    return {
        "n": len(values),
        "p50": nearest_rank(values, 50),
        "p95": nearest_rank(values, 95),
        "max": max(values),
        "total": sum(values),
    }


def ranges(frames):
    """Compact `a-b, c` text for a sorted list of ints."""
    out = []
    frames = sorted(frames)
    i = 0
    while i < len(frames):
        j = i
        while j + 1 < len(frames) and frames[j + 1] == frames[j] + 1:
            j += 1
        out.append(str(frames[i]) if i == j else f"{frames[i]}-{frames[j]}")
        i = j + 1
    return ", ".join(out)


# ---------------------------------------------------------------- the join


def row_marks(row):
    """(changes, gap, recorded) — a minimal record carries no marks."""
    if "changes" not in row and "gap" not in row:
        return [], False, False
    return list(row.get("changes") or []), bool(row.get("gap")), True


def flag(drain, key):
    """A drain flag: True, False, or None when the record lacks it."""
    if not isinstance(drain, dict) or key not in drain:
        return None
    value = drain[key]
    return None if value is None else bool(value)


def admitted(drain, resource):
    if not isinstance(drain, dict):
        return None
    return drain.get(f"{resource}_admitted_items")


def frame_drains(frame, marks_of, gpu, drain):
    """Per-resource drain state for a matched frame."""
    out = {}
    intervals_all = []
    for resource in DRAINS:
        encs = [m for m in marks_of.get(frame, ()) if m["kind"] == resource]
        items = admitted(drain, resource)
        if not encs:
            if items:
                out[resource] = {"state": "missing", "encoders": 0, "gpu_us": None,
                                 "reason": "record admitted items, trace holds no encoder"}
            else:
                out[resource] = {"state": "none", "encoders": 0, "gpu_us": None}
            continue
        intervals = []
        missing = 0
        for m in encs:
            rows = gpu.get(m["encoder"])
            if not rows:
                missing += 1
            else:
                intervals += [(s, e) for s, e, _ in rows]
        if missing:
            out[resource] = {"state": "missing", "encoders": len(encs), "gpu_us": None,
                             "reason": f"{missing} encoder(s) with no GPU row"}
            continue
        out[resource] = {"state": "measured", "encoders": len(encs),
                         "gpu_us": union_ns(intervals) / 1000.0}
        intervals_all += intervals
    states = [out[r]["state"] for r in DRAINS]
    if "missing" in states:
        total = {"state": "missing", "gpu_us": None}
    elif "measured" in states:
        total = {"state": "measured", "gpu_us": union_ns(intervals_all) / 1000.0}
    else:
        total = {"state": "none", "gpu_us": None}
    out["total"] = total
    return out


def drain_end_ns(marks_of, gpu):
    """Per frame: the latest end of its drain encoders' GPU rows, and their busy µs."""
    out = {}
    for frame, marks in marks_of.items():
        intervals = []
        for m in marks:
            if m["kind"] in DRAINS:
                intervals += [(s, e) for s, e, _ in gpu.get(m["encoder"], ())]
        if intervals:
            out[frame] = (max(e for _, e in intervals), union_ns(intervals) / 1000.0)
    return out


def reduce_run(record, marks, gpu, budget_ms=DEFAULT_BUDGET_MS, clock=None):
    header = record.get("header") or {}
    rows = record.get("rows") or []
    if not rows:
        raise ReduceError("the record holds no rows")
    marks_of = {}
    for m in marks:
        marks_of.setdefault(m["frame"], []).append(m)
    record_frames = {row["frame"] for row in rows}
    first_frame = min(record_frames)
    fe_frames = {f for f, ms in marks_of.items() if any(m["kind"] == FRAME for m in ms)}

    phase_of = {row["frame"]: row.get("phase") for row in rows}
    last_marked = max(marks_of) if marks_of else None
    covered = {}
    for frame in sorted(record_frames):
        previous = frame - 1
        covered[frame] = (frame == first_frame or previous in fe_frames
                          or (phase_of.get(previous) == "no_draw" and covered.get(previous, False)))

    def matched(frame):
        if not covered[frame]:
            return False
        if phase_of.get(frame) == "no_draw":
            return last_marked is not None and last_marked > frame
        return frame in fe_frames

    begin = {f: min(m["start"] for m in ms) for f, ms in marks_of.items()}
    drain_ends = drain_end_ns(marks_of, gpu)
    budget_us = budget_ms * 1000.0

    frames = []
    for row in rows:
        frame = row["frame"]
        changes, gap, marks_recorded = row_marks(row)
        drain = row.get("drain")
        entry = {
            "frame": frame,
            "phase": row.get("phase"),
            "pose": row.get("pose"),
            "route_time_s": row.get("route_time_s"),
            "interval_us": row.get("interval_us"),
            "changes": changes,
            "gap": gap,
            "marks_recorded": marks_recorded,
            "cold": flag(drain, "cold"),
            "cold_lightmap": flag(drain, "cold_lightmap"),
            "cold_sh": flag(drain, "cold_sh"),
            "pool_grew": flag(drain, "pool_grew"),
            "matched": matched(frame),
        }
        if entry["matched"]:
            entry["drains"] = frame_drains(frame, marks_of, gpu, drain)
            entry["begin_ns"] = begin.get(frame)
        else:
            entry["drains"] = None
        frames.append(entry)

    if not any(f["matched"] for f in frames):
        if not marks:
            raise ReduceError("the trace holds no `pr-frame` mark")
        raise ReduceError(
            f"no record frame matches a frame mark: the trace marks frames "
            f"{ranges(sorted(marks_of))}, the record holds frames {ranges(sorted(record_frames))}")

    def measured(f):
        return f["phase"] == "route" and not f["changes"] and not f["gap"]

    route = [f for f in frames if f["phase"] == "route"]
    route_matched = [f for f in route if f["matched"]]
    route_measured = [f for f in route_matched if measured(f)]

    def intervals_of(group):
        return [f["interval_us"] / 1000.0 for f in group if f["interval_us"] is not None]

    def interval_block(group):
        values = intervals_of(group)
        block = stats(values)
        block.pop("total")
        block["rows"] = len(group)
        block["over_budget"] = sum(1 for v in values if v > budget_ms) if values else None
        return block

    apart = {}
    for name, pick in (
        ("hold", lambda f: f["phase"] == "hold"),
        ("no_draw", lambda f: f["phase"] == "no_draw"),
        ("marked", lambda f: f["phase"] == "route" and (f["changes"] or f["gap"])),
    ):
        group = [f for f in frames if pick(f)]
        block = interval_block([f for f in group if f["matched"]])
        block["rows"] = len(group)
        block["matched"] = sum(1 for f in group if f["matched"])
        drained = [f["drains"]["total"] for f in group
                   if f["matched"] and f["drains"]["total"]["state"] != "none"]
        block["drain_gpu"] = stats([d["gpu_us"] / 1000.0 for d in drained
                                    if d["state"] == "measured"])
        block["drain_gpu"]["missing"] = sum(1 for d in drained if d["state"] == "missing")
        apart[name] = block
    marked_rows = [f for f in route if f["changes"] or f["gap"]]
    change_counts = {}
    for f in marked_rows:
        for kind in f["changes"]:
            change_counts[kind] = change_counts.get(kind, 0) + 1
    apart["marked"]["changes"] = change_counts
    apart["marked"]["gaps"] = sum(1 for f in marked_rows if f["gap"])

    drain_block = drain_summary(route_measured)
    charge = over_budget_charge(route_measured, budget_us, drain_ends)

    unmatched_route = [f["frame"] for f in route if not f["matched"]]
    unmatched_hold = [f["frame"] for f in frames if f["phase"] == "hold" and not f["matched"]]
    marks_unrecorded = sum(1 for f in frames if not f["marks_recorded"])
    trace_only = sorted(set(marks_of) - record_frames)

    return {
        "record": {
            "schema": header.get("schema"),
            "pacing": header.get("pacing"),
            "present_mode": header.get("present_mode"),
            "cap": header.get("cap"),
            "outcome": header.get("outcome"),
            "hold_frames": header.get("hold_frames"),
            "adapter": (header.get("machine") or {}).get("adapter") or header.get("adapter"),
            "gpu_clock": header.get("gpu_clock", "unrecorded"),
            "rows": len(rows),
            "phases": {p: sum(1 for f in frames if f["phase"] == p)
                       for p in ("hold", "route", "no_draw")},
            "marks_unrecorded_rows": marks_unrecorded,
        },
        "trace": {
            "marked_frames": len(marks_of),
            "first_marked": min(marks_of) if marks_of else None,
            "last_marked": max(marks_of) if marks_of else None,
            "marked_encoders": {k: sum(1 for m in marks if m["kind"] == k)
                                for k in (LIGHTMAP, SH, FRAME, OTHER)},
            "marked_frames_without_record_row": len(trace_only),
        },
        "budget_ms": budget_ms,
        "gpu_clock": clock or clock_unrecorded(header),
        "join": {
            "route_rows": len(route),
            "route_matched": len(route_matched),
            "route_unmatched": len(unmatched_route),
            "route_unmatched_frames": ranges(unmatched_route),
            "hold_unmatched": len(unmatched_hold),
        },
        "route": interval_block(route_measured),
        "route_record_wide": interval_block([f for f in route if measured(f)]),
        "apart": apart,
        "drain_gpu": drain_block,
        "over_budget_charge": charge,
        "frames": frames,
    }


def drain_category(frame, resource=None):
    """warm, cold, growth or unknown for a drain frame (cold and growth may both hold)."""
    cold = frame["cold"] if resource is None else frame[f"cold_{resource}"]
    growth = frame["pool_grew"] if resource in (None, LIGHTMAP) else False
    if cold is None or growth is None:
        return ["unknown"]
    cats = []
    if cold:
        cats.append("cold")
    if growth:
        cats.append("growth")
    return cats or ["warm"]


def drain_summary(route_measured):
    out = {"missing": {}, "resources": {}, "categories": {}}
    for resource in DRAINS + ("total",):
        drained = [f for f in route_measured if f["drains"][resource]["state"] != "none"]
        known = [f for f in drained if f["drains"][resource]["state"] == "measured"]
        out["missing"][resource] = len(drained) - len(known)
        block = stats([f["drains"][resource]["gpu_us"] / 1000.0 for f in known])
        block["encoders"] = sum(f["drains"][resource].get("encoders", 0) for f in known) \
            if resource != "total" else None
        out["resources"][resource] = block
        cats = {}
        for f in known:
            for cat in drain_category(f, None if resource == "total" else resource):
                cats.setdefault(cat, []).append(f["drains"][resource]["gpu_us"] / 1000.0)
        out["categories"][resource] = {
            cat: stats(cats.get(cat, []))
            for cat in ("warm", "cold", "growth", "unknown")
            if not (resource == SH and cat == "growth")
        }
    return out


def over_budget_charge(route_measured, budget_us, drain_ends):
    frames_out = []
    for f in route_measured:
        interval = f["interval_us"]
        if interval is None or interval <= budget_us:
            continue
        own = f["drains"]["total"]
        own_ms = None if own["state"] == "missing" else (own["gpu_us"] or 0.0) / 1000.0
        window_start = None if f["begin_ns"] is None else f["begin_ns"] - interval * 1000
        in_flight = []
        if window_start is not None:
            for j, (end, busy_us) in drain_ends.items():
                if j < f["frame"] and end > window_start:
                    in_flight.append((j, busy_us / 1000.0))
        in_flight.sort()
        frames_out.append({
            "frame": f["frame"],
            "interval_ms": interval / 1000.0,
            "own_ms": own_ms,
            "in_flight_frames": [j for j, _ in in_flight],
            "own_plus_in_flight_ms": None if own_ms is None
            else own_ms + sum(ms for _, ms in in_flight),
        })
    with_own = sum(1 for f in frames_out if f["own_ms"])
    with_any = sum(1 for f in frames_out if f["own_ms"] or f["in_flight_frames"])
    return {
        "over_budget": len(frames_out),
        "with_own_drain": with_own,
        "with_own_or_in_flight_drain": with_any,
        "with_no_drain": len(frames_out) - with_any,
        "own_missing": sum(1 for f in frames_out if f["own_ms"] is None),
        "frames": frames_out,
    }


# ---------------------------------------------------------------- the clock


def parse_ioreg(text):
    """Polls: [{'t': seconds, 'whole_seconds': bool, 'clock': [...], ...}]."""
    polls = []
    current = None
    for line in text.splitlines():
        stamp = TIMESTAMP.match(line)
        if stamp:
            whole = stamp.group(2) in (None, "N")
            t = float(stamp.group(1)) if whole else float(f"{stamp.group(1)}.{stamp.group(2)}")
            current = {"t": t, "whole_seconds": whole, "clock": [], "temp": [], "util": []}
            polls.append(current)
            continue
        if current is None:
            continue
        for name, value in READING.findall(line):
            key = {"Core Clock(MHz)": "clock", "Temperature(C)": "temp",
                   "Device Utilization %": "util"}[name]
            current[key].append(int(value))
    return polls


def clock_unrecorded(header, reason=None):
    source = header.get("gpu_clock", "unrecorded") if header else "unrecorded"
    why = reason or ("no --ioreg poll given; the record names the clock "
                     f"`{source}`" if source == "unrecorded" else f"record names `{source}`")
    return {"state": "unrecorded", "reason": why, "text": f"GPU clock: unrecorded ({why})"}


def clock_summary(polls, window=None, scope=None):
    """The clock over polls inside `window` (epoch seconds), or every poll."""
    if window is not None:
        lo, hi = window
        chosen = [p for p in polls
                  if (lo - (1.0 if p["whole_seconds"] else 0.0)) <= p["t"] <= hi]
    else:
        chosen = list(polls)
    readings = [c for p in chosen for c in p["clock"] if c > 0]
    recorded_polls = sum(1 for p in chosen if any(c > 0 for c in p["clock"]))
    scope = scope or "the trace's matched route span"
    if not readings:
        why = f"no non-zero Core Clock reading in {len(chosen)} poll(s) over {scope}"
        return {"state": "unrecorded", "reason": why, "polls": len(chosen),
                "text": f"GPU clock: unrecorded ({why})"}
    median = statistics.median(readings)
    temps = [t for p in chosen for t in p["temp"]]
    utils = [u for p in chosen for u in p["util"]]
    out = {
        "state": "recorded",
        "scope": scope,
        "median_mhz": median,
        "min_mhz": min(readings),
        "max_mhz": max(readings),
        "readings": len(readings),
        "polls": len(chosen),
        "unrecorded_polls": len(chosen) - recorded_polls,
        "temperature_max_c": max(temps) if temps else None,
        "utilization_median_pct": statistics.median(utils) if utils else None,
    }
    out["text"] = (f"GPU clock: median {median:g} MHz (min {out['min_mhz']}, max {out['max_mhz']}; "
                   f"{len(readings)} readings from {recorded_polls} polls"
                   f"{', ' + str(out['unrecorded_polls']) + ' unrecorded' if out['unrecorded_polls'] else ''}"
                   f") over {scope}")
    return out


def route_span_ns(summary, gpu, marks):
    """Trace-time span of the matched, measured route frames' marked work."""
    frames = {f["frame"] for f in summary["frames"]
              if f["matched"] and f["phase"] == "route" and not f["changes"] and not f["gap"]}
    starts, ends = [], []
    for m in marks:
        if m["frame"] in frames:
            starts.append(m["start"])
            ends.append(m["end"])
            ends += [e for _, e, _ in gpu.get(m["encoder"], ())]
    if not starts:
        return None
    return min(starts), max(ends)


# ---------------------------------------------------------------- driver


def reduce_paths(record_path, trace_dir, ioreg_path=None, toc_path=None,
                 budget_ms=DEFAULT_BUDGET_MS):
    with Path(record_path).open(encoding="utf-8") as handle:
        record = json.load(handle)
    exports = find_exports(trace_dir)
    for schema in (APP_SCHEMA, GPU_SCHEMA):
        if schema not in exports:
            raise ReduceError(f"{trace_dir} holds no `{schema}` export")
    marks, gpu = read_trace(exports[APP_SCHEMA], exports[GPU_SCHEMA])
    summary = reduce_run(record, marks, gpu, budget_ms)
    summary["record"]["path"] = str(record_path)
    summary["trace"]["dir"] = str(trace_dir)
    if ioreg_path is not None:
        with Path(ioreg_path).open(encoding="utf-8", errors="replace") as handle:
            polls = parse_ioreg(handle.read())
        toc = toc_path or exports.get("toc")
        start = read_toc_start(toc) if toc is not None else None
        span = route_span_ns(summary, gpu, marks)
        if start is None or span is None:
            why = ("no trace table of contents with a start date" if start is None
                   else "no matched route frame to place")
            clock = clock_summary(polls, None,
                                  scope=f"the whole poll log (time bases not aligned: {why})")
        else:
            clock = clock_summary(polls, (start + span[0] / 1e9, start + span[1] / 1e9))
        summary["gpu_clock"] = clock
    return summary


def ms(value, digits=3):
    return "-" if value is None else f"{value:.{digits}f}"


def table(headers, rows, indent="  "):
    cells = [[str(c) for c in row] for row in rows]
    widths = [len(h) for h in headers]
    for row in cells:
        widths = [max(w, len(c)) for w, c in zip(widths, row)]
    out = [indent + "  ".join(h.ljust(w) for h, w in zip(headers, widths)).rstrip()]
    out.append(indent + "  ".join("-" * w for w in widths))
    for row in cells:
        out.append(indent + "  ".join(c.ljust(w) for c, w in zip(row, widths)).rstrip())
    return out


def tri(value):
    return "unknown" if value is None else ("yes" if value else "no")


def drain_cell(drains, resource):
    entry = drains[resource]
    if entry["state"] == "none":
        return "-"
    if entry["state"] == "missing":
        return "missing"
    return ms(entry["gpu_us"] / 1000.0)


def render_text(s, all_frames=False):
    rec, trace, join = s["record"], s["trace"], s["join"]
    clock = s["gpu_clock"]["text"]
    out = []
    phases = rec["phases"]
    out.append(f"Record: {rec.get('path', '')}  ({rec['schema']})")
    cap = rec["cap"]
    cap_text = "-" if not cap else f"{cap.get('source')} {cap.get('decoded_bytes_per_drain')} B"
    out.append(f"  pacing {rec['pacing'] or '-'}  present {rec['present_mode'] or '-'}  "
               f"cap {cap_text}  outcome {rec['outcome'] or '-'}")
    out.append(f"  rows {rec['rows']}: hold {phases['hold']}, route {phases['route']}, "
               f"no-draw {phases['no_draw']}")
    if rec["marks_unrecorded_rows"]:
        out.append(f"  change and gap marks: not in the record ({rec['marks_unrecorded_rows']} rows,"
                   " minimal schema); no row is excluded as marked")
    out.append(f"Trace: {trace.get('dir', '')}")
    enc = trace["marked_encoders"]
    out.append(f"  marked frames {trace['marked_frames']} ({trace['first_marked']}-"
               f"{trace['last_marked']}); marked encoders: lightmap {enc[LIGHTMAP]}, "
               f"SH {enc[SH]}, frame {enc[FRAME]}, other {enc[OTHER]}")
    if trace["marked_frames_without_record_row"]:
        out.append(f"  marked frames with no record row: {trace['marked_frames_without_record_row']}")
    out.append("")
    out.append("Join")
    out.append(f"  route frames: {join['route_rows']}; matched {join['route_matched']}; "
               f"unmatched {join['route_unmatched']}"
               + (f" (frames {join['route_unmatched_frames']})" if join["route_unmatched"] else ""))
    out.append(f"  hold frames unmatched: {join['hold_unmatched']} (not an error)")
    out.append("")
    budget = s["budget_ms"]
    out.append(f"Route intervals, ms (matched route frames, change- and gap-marked left out; "
               f"budget {budget:.3f} ms)")
    rows = []
    for name, b in (("route (matched)", s["route"]),
                    ("route (record-wide, trace-independent)", s["route_record_wide"])):
        rows.append([name, b["rows"], b["n"], ms(b["p50"]), ms(b["p95"]), ms(b["max"]),
                     "-" if b["over_budget"] is None else b["over_budget"]])
    out += table(["rows", "count", "timed", "p50", "p95", "max", "over budget"], rows)
    out.append("")
    out.append("Reported apart (matched rows' intervals; drain GPU ms)")
    out.append(f"  {clock}")
    rows = []
    for name, b in s["apart"].items():
        d = b["drain_gpu"]
        rows.append([name, b["rows"], b["matched"], ms(b["p50"]), ms(b["p95"]), ms(b["max"]),
                     "-" if b["over_budget"] is None else b["over_budget"], d["n"], ms(d["max"]),
                     d["missing"]])
    out += table(["rows", "count", "matched", "p50", "p95", "max", "over budget", "drains",
                  "drain max", "missing"], rows)
    marked = s["apart"]["marked"]
    if marked["changes"] or marked["gaps"]:
        kinds = ", ".join(f"{k} {n}" for k, n in sorted(marked["changes"].items()))
        out.append(f"  marked: changes {kinds or 'none'}; gaps {marked['gaps']}")
    out.append("")
    d = s["drain_gpu"]
    out.append("Drain GPU time, ms (matched, unmarked route frames; union per frame)")
    out.append(f"  {clock}")
    rows = []
    for resource in (LIGHTMAP, SH, "total"):
        b = d["resources"][resource]
        rows.append([resource, b["n"], ms(b["p50"]), ms(b["p95"]), ms(b["max"]), ms(b["total"]),
                     d["missing"][resource]])
    out += table(["drain", "frames", "p50", "p95", "max", "sum", "missing"], rows)
    rows = []
    for resource in (LIGHTMAP, SH, "total"):
        for cat, b in d["categories"][resource].items():
            rows.append([resource, cat, b["n"], ms(b["p50"]), ms(b["max"])])
    out.append("  by kind (a frame both cold and growing counts in both):")
    out += table(["drain", "kind", "frames", "p50", "max"], rows, "    ")
    out.append("")
    c = s["over_budget_charge"]
    out.append(f"Over-budget charge ({c['over_budget']} measured route frames over {budget:.3f} ms)")
    out.append(f"  {clock}")
    out.append(f"  own drain: {c['with_own_drain']}; own or in-flight drain: "
               f"{c['with_own_or_in_flight_drain']}; no drain: {c['with_no_drain']}"
               + (f"; own drain missing: {c['own_missing']}" if c["own_missing"] else ""))
    listed = [f for f in c["frames"]
              if all_frames or f["own_ms"] or f["own_ms"] is None or f["in_flight_frames"]]
    if listed:
        if not all_frames and len(listed) < len(c["frames"]):
            out.append("  frames with a drain charged (--all-frames lists every one):")
        rows = [[f["frame"], ms(f["interval_ms"]), ms(f["own_ms"]),
                 ms(f["own_plus_in_flight_ms"]),
                 ",".join(str(j) for j in f["in_flight_frames"]) or "-"]
                for f in listed]
        out += table(["frame", "interval", "own", "own+in flight", "in-flight frames"], rows, "    ")
    out.append("")
    charged = {f["frame"] for f in c["frames"] if f["in_flight_frames"]}
    shown = [f for f in s["frames"] if f["matched"] and (
        all_frames or f["drains"]["total"]["state"] != "none" or f["frame"] in charged)]
    title = ("every matched frame" if all_frames
             else "matched frames with a drain, or over budget with one in flight; "
                  "--all-frames lists every one")
    out.append(f"Per frame ({title}; drain GPU ms)")
    out.append(f"  {clock}")
    rows = []
    for f in shown:
        marks = ",".join(f["changes"] + (["gap"] if f["gap"] else [])) or "-"
        rows.append([f["frame"], f["phase"], f["pose"], ms(None if f["interval_us"] is None
                                                            else f["interval_us"] / 1000.0),
                     drain_cell(f["drains"], LIGHTMAP), drain_cell(f["drains"], SH),
                     drain_cell(f["drains"], "total"), tri(f["cold"]), tri(f["pool_grew"]),
                     marks])
    out += table(["frame", "phase", "pose", "interval", "lightmap", "SH", "total", "cold",
                  "growth", "marks"], rows)
    return "\n".join(out) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Join a camera-route playback record with a Metal System Trace export.")
    parser.add_argument("--record", type=Path, required=True, help="playback record JSON")
    parser.add_argument("--trace-dir", type=Path, required=True,
                        help="directory of `xctrace export` tables (.xml or .xml.xz)")
    parser.add_argument("--toc", type=Path, help="the export's table of contents "
                        "(default: a <trace-toc> file in --trace-dir)")
    parser.add_argument("--ioreg", type=Path, help="ioreg PerformanceStatistics poll log")
    parser.add_argument("--budget-ms", type=float, default=DEFAULT_BUDGET_MS,
                        help="frame interval budget in ms (default 16.667)")
    parser.add_argument("--all-frames", action="store_true",
                        help="list every matched frame, not only drain and over-budget ones")
    parser.add_argument("--json", action="store_true", help="print the summary as JSON")
    args = parser.parse_args(argv)
    try:
        summary = reduce_paths(args.record, args.trace_dir, args.ioreg, args.toc, args.budget_ms)
    except (OSError, ValueError, ET.ParseError, lzma.LZMAError) as err:
        print(f"playback_trace_reduce: error: {err}", file=sys.stderr)
        return 2
    except ReduceError as err:
        print(f"playback_trace_reduce: error: {err}", file=sys.stderr)
        return 2
    if args.json:
        json.dump(summary, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        sys.stdout.write(render_text(summary, args.all_frames))
    return 0


if __name__ == "__main__":
    sys.exit(main())
