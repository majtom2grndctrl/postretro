#!/usr/bin/env python3
"""Join a camera-route playback record with a Metal System Trace export.

A playback run (`--camera-route`) writes a per-frame record
(`postretro.camera_route_playback.v1`) and wraps each frame's drain and frame
encoders in a `pr-frame <k> <site>` debug group, where k is the record row's
`frame` and the site is `lightmap`, `sh` or `frame`. Marks from before the
site was added read `pr-frame <k>`. This reducer joins the record with an
`xctrace export` of the run's `metal-application-intervals` and
`metal-gpu-intervals` tables, and reports per frame and per run. See
`context/lib/rendering_pipeline.md` §12 (GPU Pass Timing: Without timestamp
support, Pin the GPU clock).

The join
--------
* In `metal-application-intervals`, a marked encoder row is a child of a
  command buffer row (encoder id all ones) with the same command buffer id,
  and its label carries the mark. A mark on the command buffer row's own
  label names no encoder, so it joins no GPU row: it counts toward the
  frame's coverage and its frame encoder, never toward its drains. xctrace's
  own "Frame N" is never k.
* A mark's site says what it marks. A mark with no site is classified by its
  command buffer's label: "Streamed Lightmap Drain", "Streamed SH Upload
  Copies" or "Frame Encoder". wgpu's Metal backend relabels the last command
  buffer of every queue submit "(wgpu internal) Signal", so an older mark on
  such a buffer cannot be classified. The site wins over any label.
* `metal-gpu-intervals` rows (any channel: SDMA0, GFX, ...) join to that
  marked encoder by encoder id.
* Attribution is by mark only, never by time overlap: a drain recorded on
  frame k is frame k's, however late the GPU runs it, including after frame
  k + 1's work begins.
* A frame's GPU time per resource is the union of all its matched GPU
  intervals across channels and encoders, so nested and coalesced rows count
  once (§12). The drain total is the union of the lightmap and SH intervals.

Matched frames
--------------
Frame k's encoding is covered when k is the record's first row, or the
trace holds frame k - 1's frame encoder mark, or row k - 1 is a no-draw row
(which encodes no frame encoder) whose own encoding is covered. A frame's
drains are encoded after frame k - 1's frame encoder, so a covered frame's
drains are all in the trace; the first marked frame of a trace attached
mid-run, and any frame after a stretch of missing rows, is not covered. A
hold or route frame is matched when it is covered and the trace holds its own
frame encoder mark. A no-draw frame is matched when it is covered and the
trace marks some later frame, so it did not end before the frame's drains.
Unmatched frames leave every per-run figure and are counted and named; an
unmatched frame never counts as zero drain GPU time. Hold frames before the
trace attached are not an error. A record with no matched frame at all is an
error (exit 2), and so is a malformed record.

Within a matched frame, a resource's drain GPU time reads `missing`, not
zero, when one of its marked encoders has no GPU row (the trace ended before
the GPU ran it) or when the record says the frame admitted items for that
resource but the trace holds no encoder for it. Such frames leave the drain
statistics and are counted.

Unattributed work. An older mark that no label classifies is marked GPU work
of frame k that may be either drain, so it makes the frame's lightmap, SH and
total drain read `missing` with the reason "marked GPU work on a relabelled
command buffer could not be attributed". Adding it to one resource, or
leaving it out, would report a partial number as the whole drain. The work
itself, the union of its GPU intervals, is reported per frame
(`unattributed`) and per run. Current marks name their site, so a current
trace has none.

Per-run figures
---------------
Interval p50, p95, max (nearest rank) and frames over budget cover matched
route rows only, and leave out rows carrying a change mark or a gap. The
`route_record_wide` row gives the same figures over every unmarked route row
in the record, matched or not: it reads only the record, never the trace.
Hold, no-draw and marked rows are reported apart with their own counts. Drain
GPU statistics cover matched, unmarked route frames; warm, cold and
pool-growth drains are counted and summarized apart. A record from before the
drain, cold and growth columns existed has no drain flags or marks: flags read
`unknown`, never false, and the output says the marks were not recorded.

Over-budget charge (both readings)
----------------------------------
A row's `interval_us` runs from the previous world frame's start to its own
frame's start, so row k's interval is the time frame k - 1 took. Frame k's own
drains are encoded after frame k starts, so they cannot have stretched it.
For a measured route frame k whose interval exceeds the budget:

* own: the drain GPU time of the drains marked k - 1, the frame whose start
  opens the interval, whatever its phase. It reads `missing` when frame
  k - 1's drain is missing, or frame k - 1 has no record row or is unmatched.
  A frame with no drain reads 0.
* own + in flight: own, plus the whole drain GPU time of every earlier frame
  j < k - 1 whose drain was still on the GPU when frame k - 1 began.

The trace holds no frame-start event, and no mark sits at a fixed lag from
one: the CPU work before the first mark (ticks, the streaming step, the SH
residency drain, eye and visibility work, and before the frame encoder, the
surface acquire, which blocks under vsync) varies frame to frame by several
ms. So S(k - 1), frame k - 1's start, is bounded rather than estimated. Every
marked row of frame j, and its command buffer row (encoding, creation to
commit, on the main thread), falls after S(j) and before S(j + 1). With
earliest(j) and latest(j) the first start and last end of those rows:

    U = min(earliest(k - 1), earliest(k) - interval_k)
    L = max(latest(k - 2), latest(k - 1) - interval_k - 1 µs)

(the record truncates the interval to the microsecond). An earlier frame's
drain whose GPU end is at or before L was done when frame k - 1 began; one
that ends after U was in flight; one that ends in (L, U] may have been.

The in-flight reading never adds a partial number. It reads `missing`, and
names each cause, when an earlier drain:

* may have been in flight (its end lies in (L, U]);
* was in flight but its GPU time is unknown: its frame is unmatched, has no
  record row, or holds unattributed work;
* has an unknown end: an encoder with no GPU row, or admitted items with no
  encoder. Such a drain may be in flight for every later window, so one
  dropped row can blank a long run; the per-run block counts the rows each
  cause blanked.

It also reads `missing` when S(k - 1) may precede the first marked row of the
covered stretch that ends at frame k - 1, since a frame before that stretch
may hold drains the trace never saw. A drain encoded before the stretch and
still running after S(k - 1) is the one case it cannot see. The per-run
counts take known drains only: a possible in-flight drain is counted apart.

GPU clock
---------
Every GPU figure names the clock it ran at. With `--ioreg <log>` (a poll
loop printing a timestamp line, then `"Core Clock(MHz)"=<n>` and friends, as
`docs/camera-routes.md` §"GPU time on a Mac" gives), polls join the trace by
wall time: the export's table of contents (`--toc`, or a `<trace-toc>` file in
the trace dir) gives the trace's start date, so trace time t maps to start +
t. The clock is the median, min and max of every non-zero `Core Clock(MHz)`
reading in polls inside the matched route frames' trace span. A poll log with
whole-second stamps (e.g. `date +%s`) widens the span by one second at its
start. A zero or missing reading is unrecorded. With no table of contents, or
a start date that does not parse, the time bases cannot be aligned: the
reducer says why and reports the clock over the whole poll log, labelled as
such. Without `--ioreg` the clock reads unrecorded: the engine records none
in-process (`gpu_clock: unrecorded`). An unrecorded clock never reads as
pinned.

Output
------
Text by default, or the whole summary with `--json`. `--budget-ms` sets the
frame-interval budget (default 16.667 ms, one 60 Hz frame). By default the
over-budget table lists only rows with a drain charged or a reading missing,
and the per-frame table only frames with a drain, or over budget with one in
flight; `--all-frames` lists every one.

Standard library only. Usage:

    python3 tools/playback_trace_reduce.py --record <playback.json> \
        --trace-dir <dir of exported tables, .xml or .xml.xz> \
        [--toc <toc.xml>] [--ioreg <ioreg log>] [--budget-ms 16.667] \
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
# A mark with no site on a command buffer no label names: marked GPU work the
# reducer cannot attribute to a drain or the frame encoder.
OTHER = "unattributed"
DRAINS = (LIGHTMAP, SH)
KIND_LABELS = (
    (LIGHTMAP, "Streamed Lightmap Drain"),
    (SH, "Streamed SH Upload Copies"),
    (FRAME, "Frame Encoder"),
)
# wgpu-hal's Metal backend relabels the last command buffer of every submit.
SIGNAL_LABEL = "(wgpu internal) Signal"
UNATTRIBUTED_REASON = "marked GPU work on a relabelled command buffer could not be attributed"
UNCLASSIFIED_REASON = "marked GPU work on an unrecognised command buffer could not be attributed"

# `pr-frame <k> <site>`; a mark from before the site was added has no site.
MARK = re.compile(r"pr-frame (\d+)(?: (lightmap|sh|frame))?\b")
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
        # The schema element sits in the export's first lines; 16 KB clears a
        # long header without inflating a whole multi-GB table.
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
    """Marked encoder rows from a `metal-application-intervals` export.

    A mark's kind is its site when the mark names one, and otherwise the kind
    its command buffer's label (or its own) names. Each mark keeps its command
    buffer row's span, the row's encoding from creation to commit."""
    parents = {}
    marked = []
    for row in iter_rows(handle):
        label = as_label(row.get("event-label"))
        cmdbuffer = as_int(row.get("cmdbuffer-id"))
        encoder = as_int(row.get("encoder-id"))
        start = as_int(row.get("start"))
        end = None if start is None else start + (as_int(row.get("duration")) or 0)
        if encoder == ALL_ONES and cmdbuffer is not None and start is not None:
            parents.setdefault(cmdbuffer, []).append((start, end, label))
        match = MARK.search(label)
        if match is None or start is None:
            continue
        marked.append({
            "frame": int(match.group(1)),
            "site": match.group(2),
            "start": start,
            "end": end,
            "cmdbuffer": cmdbuffer,
            "encoder": encoder,
            "label": label,
        })
    for mark in marked:
        parent = None
        for candidate in parents.get(mark["cmdbuffer"], ()):
            if candidate[0] <= mark["start"]:
                parent = candidate
        parent_label = parent[2] if parent else ""
        mark["buffer_start"] = parent[0] if parent else None
        mark["buffer_end"] = parent[1] if parent else None
        # The site wins over any label: wgpu may have relabelled the buffer.
        mark["kind"] = mark["site"] or classify(mark["label"], parent_label)
        mark["relabelled"] = SIGNAL_LABEL in parent_label
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


def names_encoder(mark):
    """False for a mark on a command buffer row: its id names no encoder."""
    return mark["encoder"] not in (None, ALL_ONES)


def read_trace(app_path, gpu_path):
    with open_table(app_path) as handle:
        marks = read_marks(handle)
    # Encoder ids join GPU rows; the all-ones id names a command buffer, never
    # an encoder, so a mark on a command buffer row joins nothing.
    encoders = {m["encoder"] for m in marks if names_encoder(m)}
    with open_table(gpu_path) as handle:
        gpu = read_gpu(handle, encoders)
    return marks, gpu


ISO_PARTS = re.compile(
    r"^(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2})(?:\.(\d+))?(Z|[+-]\d{2}:?\d{2})?$")


def parse_start_date(text):
    """Epoch seconds of an ISO 8601 date with a UTC offset, or None.

    Python 3.8's `fromisoformat` takes only 3- or 6-digit fractions and a
    `+HH:MM` offset, so a `Z`, a `+HHMM` offset and any other fraction length
    are normalized first. A date with no offset names no instant: None."""
    match = ISO_PARTS.match(text.strip())
    if match is None:
        return None
    base, fraction, offset = match.groups()
    if offset is None:
        return None
    if offset == "Z":
        offset = "+00:00"
    elif ":" not in offset:
        offset = f"{offset[:3]}:{offset[3:]}"
    fraction = "" if fraction is None else "." + (fraction + "000000")[:6]
    try:
        return datetime.fromisoformat(f"{base}{fraction}{offset}").timestamp()
    except ValueError:
        return None


def read_toc_start(path):
    """(epoch seconds, None) for the trace's start date, or (None, why not)."""
    with open_table(path) as handle:
        text = handle.read().decode("utf-8", errors="replace")
    match = re.search(r"<start-date>([^<]+)</start-date>", text)
    if match is None:
        return None, f"the table of contents {path} holds no start date"
    raw = match.group(1).strip()
    start = parse_start_date(raw)
    if start is None:
        return None, (f"the table of contents' start date `{raw}` did not parse "
                      "as an ISO 8601 date with a UTC offset")
    return start, None


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
    """p50, p95 and max (nearest rank) of a list of numbers."""
    if not values:
        return {"n": 0, "p50": None, "p95": None, "max": None}
    return {
        "n": len(values),
        "p50": nearest_rank(values, 50),
        "p95": nearest_rank(values, 95),
        "max": max(values),
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


# ---------------------------------------------------------------- the record


def is_int(value):
    # bool is an int subclass; it is never a frame index or a count.
    return isinstance(value, int) and not isinstance(value, bool)


def is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def validate_record(record):
    """(header, rows) of a record whose rows carry what the join reads.

    Raises ReduceError naming the row index and field, so a malformed record
    exits 2 rather than failing midway through the join.
    """
    if not isinstance(record, dict):
        raise ReduceError("the record's top level is not a JSON object")
    header = record.get("header")
    if header is not None and not isinstance(header, dict):
        raise ReduceError("the record's `header` is not an object")
    # Read with `.get`, so each must be an object when present.
    for field in ("cap", "machine"):
        value = (header or {}).get(field)
        if value is not None and not isinstance(value, dict):
            raise ReduceError(f"the record header's `{field}` is not an object ({value!r})")
    rows = record.get("rows")
    if rows is None:
        rows = []
    if not isinstance(rows, list):
        raise ReduceError("the record's `rows` is not a list")
    seen = set()
    for index, row in enumerate(rows):
        where = f"record row {index}"
        if not isinstance(row, dict):
            raise ReduceError(f"{where} is not an object")
        frame = row.get("frame")
        if not is_int(frame) or frame < 0:
            raise ReduceError(f"{where}: `frame` is not a non-negative integer ({frame!r})")
        if frame in seen:
            raise ReduceError(f"{where}: `frame` {frame} repeats an earlier row's")
        seen.add(frame)
        where = f"{where} (frame {frame})"
        interval = row.get("interval_us")
        if interval is not None and not is_number(interval):
            raise ReduceError(f"{where}: `interval_us` is not a number ({interval!r})")
        if row.get("phase") is not None and not isinstance(row["phase"], str):
            raise ReduceError(f"{where}: `phase` is not a string")
        changes = row.get("changes")
        if changes is not None and not (isinstance(changes, list)
                                        and all(isinstance(c, str) for c in changes)):
            raise ReduceError(f"{where}: `changes` is not a list of strings")
        drain = row.get("drain")
        if drain is not None:
            if not isinstance(drain, dict):
                raise ReduceError(f"{where}: `drain` is not an object")
            for resource in DRAINS:
                items = drain.get(f"{resource}_admitted_items")
                if items is not None and not is_int(items):
                    raise ReduceError(f"{where}: `drain.{resource}_admitted_items` "
                                      "is not an integer")
    return header or {}, rows


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


def drain_encoders(marks, resource=None):
    """A frame's marked drain encoders, of one resource or both. A mark on a
    command buffer row names no encoder, so it is never one of them."""
    kinds = DRAINS if resource is None else (resource,)
    return [m for m in marks if m["kind"] in kinds and names_encoder(m)]


def unattributed_encoders(marks):
    """A frame's marked encoders no site or label attributes."""
    return [m for m in marks if m["kind"] == OTHER and names_encoder(m)]


def unattributed_work(marks, gpu):
    """A frame's marked-but-unattributed GPU work, or None when it has none.
    `gpu_us` is the union of its GPU rows, None when an encoder has none."""
    encs = unattributed_encoders(marks)
    if not encs:
        return None
    intervals = []
    no_row = 0
    for m in encs:
        rows = gpu.get(m["encoder"])
        if rows:
            intervals += [(s, e) for s, e, _ in rows]
        else:
            no_row += 1
    relabelled = sum(1 for m in encs if m["relabelled"])
    return {
        "encoders": len(encs),
        "relabelled": relabelled,
        "no_gpu_row": no_row,
        "gpu_us": None if no_row else union_ns(intervals) / 1000.0,
        "reason": UNATTRIBUTED_REASON if relabelled == len(encs) else UNCLASSIFIED_REASON,
    }


def frame_drains(frame, marks_of, gpu, drain):
    """Per-resource drain state for a matched frame.

    Marked work the reducer cannot attribute may belong to either drain, so
    it makes both resources and the total read `missing`, never a partial
    number. Each missing entry carries `causes`: `no_encoder` (admitted
    items, no encoder) and `no_gpu_row` leave the drain's GPU end unknown;
    `unattributed` leaves only its split unknown."""
    marks = marks_of.get(frame, ())
    unattributed = unattributed_work(marks, gpu)
    out = {}
    own_reasons = []
    intervals_all = []
    for resource in DRAINS:
        encs = drain_encoders(marks, resource)
        items = admitted(drain, resource)
        reasons, causes = [], []
        if not encs and items:
            reasons.append("record admitted items, trace holds no encoder")
            causes.append("no_encoder")
            own_reasons.append(f"{resource}: {reasons[-1]}")
        intervals = []
        missing = 0
        for m in encs:
            rows = gpu.get(m["encoder"])
            if not rows:
                missing += 1
            else:
                intervals += [(s, e) for s, e, _ in rows]
        if missing:
            reasons.append(f"{missing} encoder(s) with no GPU row")
            causes.append("no_gpu_row")
            own_reasons.append(f"{resource}: {reasons[-1]}")
        if unattributed is not None:
            reasons.append(unattributed["reason"])
            causes.append("unattributed")
        if reasons:
            out[resource] = {"state": "missing", "encoders": len(encs), "gpu_us": None,
                             "reason": "; ".join(reasons), "causes": causes}
        elif encs:
            out[resource] = {"state": "measured", "encoders": len(encs),
                             "gpu_us": union_ns(intervals) / 1000.0}
            intervals_all += intervals
        else:
            out[resource] = {"state": "none", "encoders": 0, "gpu_us": None}
    missing = [r for r in DRAINS if out[r]["state"] == "missing"]
    if missing:
        if unattributed is not None:
            own_reasons.append(unattributed["reason"])
        causes = sorted({c for r in missing for c in out[r]["causes"]})
        total = {"state": "missing", "gpu_us": None, "reason": "; ".join(own_reasons),
                 "causes": causes}
    elif any(out[r]["state"] == "measured" for r in DRAINS):
        total = {"state": "measured", "gpu_us": union_ns(intervals_all) / 1000.0}
    else:
        total = {"state": "none", "gpu_us": None}
    out["total"] = total
    return out, unattributed


def drain_gpu_by_frame(marks_of, gpu):
    """Per trace frame with a marked drain or unattributed encoder: the latest
    end of their GPU rows (None when they have none), the drains' busy µs,
    and how many of the encoders have no GPU row, so their end is unknown."""
    out = {}
    for frame, marks in marks_of.items():
        drains = drain_encoders(marks)
        encs = [(m, True) for m in drains] + [(m, False) for m in unattributed_encoders(marks)]
        if not encs:
            continue
        intervals, busy = [], []
        no_row = 0
        for m, is_drain in encs:
            rows = gpu.get(m["encoder"])
            if rows:
                intervals += [(s, e) for s, e, _ in rows]
                if is_drain:
                    busy += [(s, e) for s, e, _ in rows]
            else:
                no_row += 1
        out[frame] = {
            "end": max(e for _, e in intervals) if intervals else None,
            "busy_us": union_ns(busy) / 1000.0,
            "no_gpu_row": no_row,
        }
    return out


def frame_spans(marks):
    """Per trace frame: the earliest start and latest end, in trace ns, of its
    marked app rows and their command buffer rows. Frame k encodes them all
    after its own start and before frame k + 1's."""
    spans = {}
    for m in marks:
        lo, hi = m["start"], m["end"]
        if m["buffer_start"] is not None:
            lo = min(lo, m["buffer_start"])
            hi = max(hi, m["buffer_end"])
        current = spans.get(m["frame"])
        spans[m["frame"]] = (lo, hi) if current is None else (min(current[0], lo),
                                                              max(current[1], hi))
    return spans


def start_bounds(k, interval_us, spans):
    """(L, U): trace-ns bounds on S(k - 1), frame k - 1's start, for row k.

    With S(j) <= the earliest of frame j's rows and S(j + 1) >= the latest
    end of frame j's rows, and S(k) - S(k - 1) = interval_k:
    U = min(earliest(k - 1), earliest(k) - interval_k) and
    L = max(latest(k - 2), latest(k - 1) - interval_k). The record stores the
    interval truncated to the microsecond, so L takes one microsecond more.
    Either bound is None when the trace gives none."""
    interval_ns = interval_us * 1000
    lower, upper = [], []
    if k - 2 in spans:
        lower.append(spans[k - 2][1])
    if k - 1 in spans:
        lower.append(spans[k - 1][1] - interval_ns - 1000)
        upper.append(spans[k - 1][0])
    if k in spans:
        upper.append(spans[k][0] - interval_ns)
    return (max(lower) if lower else None, min(upper) if upper else None)


def reduce_run(record, marks, gpu, budget_ms=DEFAULT_BUDGET_MS, clock=None):
    header, rows = validate_record(record)
    if not rows:
        raise ReduceError("the record holds no rows")
    marks_of = {}
    for m in marks:
        marks_of.setdefault(m["frame"], []).append(m)
    record_frames = {row["frame"] for row in rows}
    first_frame = min(record_frames)
    # A mark on a frame encoder's command buffer row counts here: it places
    # the frame encoder in the trace even though it joins no GPU row.
    fe_start = {}
    for f, ms in marks_of.items():
        starts = [m["start"] for m in ms if m["kind"] == FRAME]
        if starts:
            fe_start[f] = min(starts)
    fe_frames = set(fe_start)

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

    def is_covered(frame):
        # A trace frame with no record row is covered when the trace holds
        # its predecessor's frame encoder.
        if frame in covered:
            return covered[frame]
        return frame - 1 in fe_frames

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
            entry["drains"], entry["unattributed"] = frame_drains(frame, marks_of, gpu, drain)
            entry["frame_encoder_ns"] = fe_start.get(frame)
        else:
            entry["drains"] = entry["unattributed"] = None
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
    charge = over_budget_charge(frames, route_measured, budget_ms * 1000.0, marks_of,
                                frame_spans(marks), drain_gpu_by_frame(marks_of, gpu),
                                is_covered)

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
            # null when the record names no clock (it predates the field).
            "gpu_clock": header.get("gpu_clock"),
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
            "marks_with_site": sum(1 for m in marks if m["site"]),
            "marks": len(marks),
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
        "unattributed": unattributed_summary(frames),
        "over_budget_charge": charge,
        "frames": frames,
    }


def unattributed_summary(frames):
    """Matched frames' marked-but-unattributed GPU work, every phase."""
    hit = [f for f in frames if f["matched"] and f["unattributed"] is not None]
    known = [f["unattributed"]["gpu_us"] / 1000.0 for f in hit
             if f["unattributed"]["gpu_us"] is not None]
    return {
        "frames": len(hit),
        "frame_list": ranges(f["frame"] for f in hit),
        "encoders": sum(f["unattributed"]["encoders"] for f in hit),
        "relabelled": sum(f["unattributed"]["relabelled"] for f in hit),
        "gpu_ms_sum": sum(known) if known else None,
        "gpu_ms_max": max(known) if known else None,
        # Frames with an unattributed encoder that has no GPU row.
        "gpu_missing": len(hit) - len(known),
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
        values = [f["drains"][resource]["gpu_us"] / 1000.0 for f in known]
        block = stats(values)
        block["total"] = sum(values) if values else None
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


def stretch_starts(frames, marks_of, is_covered):
    """Per frame, the first frame of the covered stretch that ends at it, or
    None when it is not covered. Every frame in a stretch has all its drains
    in the trace."""
    known = set(marks_of) | {f["frame"] for f in frames}
    out = {}
    start = None
    for frame in range(min(known), max(known) + 1):
        if is_covered(frame):
            start = frame if start is None else start
        else:
            start = None
        out[frame] = start
    return out


def own_drain(by_frame, k):
    """Row k's own charge, frame k - 1's drain: (state, ms, reason)."""
    previous = by_frame.get(k - 1)
    if previous is None:
        return "missing", None, f"frame {k - 1} has no record row"
    if not previous["matched"]:
        return "missing", None, f"frame {k - 1} is not matched in the trace"
    total = previous["drains"]["total"]
    if total["state"] == "missing":
        return "missing", None, f"frame {k - 1}'s drain is missing ({total['reason']})"
    if total["state"] == "none":
        return "none", 0.0, None
    return "measured", total["gpu_us"] / 1000.0, None


# Why an in-flight reading is blank, per earlier frame j.
CAUSE_TEXT = {
    "end_unknown": "frame {j}'s drain end is unknown",
    "time_unknown": "frame {j}'s drain was in flight, its GPU time unknown",
    "possible": "frame {j}'s drain may have been in flight",
    "uncovered": "frame {j} opens the trace's covered stretch, so an earlier drain may be "
                 "missing from the trace",
    "not_covered": "frame {j} is not covered, so an earlier drain may be missing from the trace",
}


def over_budget_charge(frames, route_measured, budget_us, marks_of, spans, drain_gpu,
                       is_covered):
    by_frame = {f["frame"]: f for f in frames}
    stretch = stretch_starts(frames, marks_of, is_covered)
    # A drain's GPU end is unknown when an encoder has no GPU row, or when
    # the record admitted items the trace holds no encoder for.
    end_unknown = {j for j, d in drain_gpu.items() if d["no_gpu_row"]}
    end_unknown |= {f["frame"] for f in frames if f["matched"] for r in DRAINS
                    if "no_encoder" in f["drains"][r].get("causes", ())}
    candidates = sorted(set(drain_gpu) | end_unknown)
    frames_out = []
    for f in route_measured:
        interval = f["interval_us"]
        if interval is None or interval <= budget_us:
            continue
        k = f["frame"]
        own_state, own_ms, own_reason = own_drain(by_frame, k)
        lower, upper = start_bounds(k, interval, spans)
        in_flight, possible, partial, causes = [], [], [], []
        for j in candidates:
            if j >= k - 1:
                break
            entry = by_frame.get(j)
            if j in end_unknown:
                partial.append(j)
                causes.append(("end_unknown", j))
                continue
            end = drain_gpu[j]["end"]
            if lower is not None and end <= lower:
                continue
            if upper is None or end <= upper:
                possible.append(j)
                causes.append(("possible", j))
            elif (entry is not None and entry["matched"]
                  and entry["drains"]["total"]["state"] == "measured"):
                in_flight.append((j, drain_gpu[j]["busy_us"] / 1000.0))
            else:
                partial.append(j)
                causes.append(("time_unknown", j))
        first = stretch.get(k - 1)
        if first is None:
            causes.append(("not_covered", k - 1))
        else:
            seen_from = min((spans[g][0] for g in range(first, k + 1) if g in spans),
                            default=None)
            # S(k - 1) may precede the stretch's first marked row, so a drain
            # of a frame before the stretch may be in flight unseen.
            if lower is None or seen_from is None or lower < seen_from:
                causes.append(("uncovered", first))
        in_flight_reason = "; ".join(CAUSE_TEXT[c].format(j=j) for c, j in causes) or None
        in_flight_ms = None if causes else sum(ms for _, ms in in_flight)
        frames_out.append({
            "frame": k,
            "interval_ms": interval / 1000.0,
            "own_frame": k - 1,
            "own_state": own_state,
            "own_ms": own_ms,
            "own_reason": own_reason,
            # Bounds on frame k - 1's start, in trace ns.
            "start_lower_ns": lower,
            "start_upper_ns": upper,
            "in_flight_frames": [j for j, _ in in_flight],
            "in_flight_possible_frames": possible,
            "in_flight_partial_frames": partial,
            "in_flight_causes": [{"cause": c, "frame": j} for c, j in causes],
            "in_flight_reason": in_flight_reason,
            "own_plus_in_flight_ms": None if own_ms is None or in_flight_ms is None
            else own_ms + in_flight_ms,
        })

    blanked = {}
    for f in frames_out:
        for cause in f["in_flight_causes"]:
            blanked.setdefault((cause["cause"], cause["frame"]), []).append(f["frame"])
    blanked_by = [{"cause": c, "frame": j, "text": CAUSE_TEXT[c].format(j=j), "rows": len(rows),
                   "row_frames": ranges(rows)}
                  for (c, j), rows in sorted(blanked.items(), key=lambda kv: (-len(kv[1]),
                                                                             kv[0][1]))]
    return {
        "over_budget": len(frames_out),
        "with_own_drain": sum(1 for f in frames_out if f["own_ms"]),
        # Known drains only: own, or one known to be in flight.
        "with_own_or_in_flight_drain": sum(1 for f in frames_out
                                           if f["own_ms"] or f["in_flight_frames"]),
        # A drain that may have been in flight, or was with its time unknown.
        "with_possible_in_flight_drain": sum(
            1 for f in frames_out
            if f["in_flight_possible_frames"] or f["in_flight_partial_frames"]),
        # Known to have none: own reads 0 and the in-flight reading is whole.
        "with_no_drain": sum(1 for f in frames_out if f["own_state"] == "none"
                             and f["in_flight_reason"] is None and not f["in_flight_frames"]),
        "own_missing": sum(1 for f in frames_out if f["own_ms"] is None),
        "in_flight_missing": sum(1 for f in frames_out if f["in_flight_reason"] is not None),
        "in_flight_blanked_by": blanked_by,
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


def clock_unrecorded(header):
    source = (header or {}).get("gpu_clock")
    named = ("the record names no clock" if source is None
             else f"the record names the clock `{source}`")
    why = f"no --ioreg poll given; {named}"
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
        if toc is None:
            start, why = None, "no trace table of contents (--toc, or a <trace-toc> file)"
        else:
            start, why = read_toc_start(toc)
        span = route_span_ns(summary, gpu, marks)
        if start is not None and span is None:
            why = "no matched route frame to place"
        if why is not None:
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


def unattributed_cell(work):
    if work is None:
        return "-"
    return "missing" if work["gpu_us"] is None else ms(work["gpu_us"] / 1000.0)


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
               f"SH {enc[SH]}, frame {enc[FRAME]}, unattributed {enc[OTHER]}")
    if trace["marks_with_site"] < trace["marks"]:
        out.append(f"  marks naming their site: {trace['marks_with_site']} of {trace['marks']}; "
                   "a mark with none is attributed by its command buffer's label")
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
    u = s["unattributed"]
    if u["frames"]:
        out.append(f"  unattributed marked GPU work (matched frames, any phase): {u['frames']} "
                   f"frame(s) ({u['frame_list']}), {u['encoders']} encoder(s), "
                   f"{u['relabelled']} on a \"{SIGNAL_LABEL}\" command buffer; "
                   f"sum {ms(u['gpu_ms_sum'])} ms, max {ms(u['gpu_ms_max'])} ms"
                   + (f", {u['gpu_missing']} frame(s) with no GPU row" if u["gpu_missing"] else "")
                   + "; those frames' drains read missing")
    out.append("")
    c = s["over_budget_charge"]
    out.append(f"Over-budget charge ({c['over_budget']} measured route frames over {budget:.3f} ms)")
    out.append(f"  {clock}")
    out.append("  own: the drain of frame k - 1, whose start opens row k's interval; "
               "in flight: earlier drains still on the GPU when frame k - 1 began")
    out.append(f"  own drain: {c['with_own_drain']}; own or known in-flight drain: "
               f"{c['with_own_or_in_flight_drain']}; possible in-flight drain: "
               f"{c['with_possible_in_flight_drain']}; no drain: {c['with_no_drain']}"
               + (f"; own drain missing: {c['own_missing']}" if c["own_missing"] else "")
               + (f"; in-flight reading missing: {c['in_flight_missing']}"
                  if c["in_flight_missing"] else ""))
    if c["in_flight_blanked_by"]:
        out.append("  in-flight readings left missing, by cause (a row may have several):")
        for b in c["in_flight_blanked_by"]:
            out.append(f"    {b['text']}: {b['rows']} row(s) ({b['row_frames']})")

    def missing_ms(value):
        return "missing" if value is None else ms(value)

    def in_flight_cell(f):
        parts = [",".join(str(j) for j in f["in_flight_frames"])]
        for key, word in (("in_flight_possible_frames", "possibly"),
                          ("in_flight_partial_frames", "partial")):
            if f[key]:
                parts.append(f"{word} " + ",".join(str(j) for j in f[key]))
        if any(c["cause"] in ("uncovered", "not_covered") for c in f["in_flight_causes"]):
            parts.append("earlier drains may be missing from the trace")
        return "; ".join(p for p in parts if p) or "-"

    listed = [f for f in c["frames"]
              if all_frames or f["own_ms"] or f["own_ms"] is None or f["in_flight_frames"]
              or f["in_flight_reason"] is not None]
    if listed:
        if not all_frames and len(listed) < len(c["frames"]):
            out.append("  frames with a drain charged or missing (--all-frames lists every one):")
        rows = [[f["frame"], ms(f["interval_ms"]), missing_ms(f["own_ms"]),
                 missing_ms(f["own_plus_in_flight_ms"]), in_flight_cell(f)]
                for f in listed]
        out += table(["frame", "interval", "own (k - 1)", "own+in flight", "in-flight frames"],
                     rows, "    ")
    out.append("")
    charged = {f["frame"] for f in c["frames"]
               if f["in_flight_frames"] or f["in_flight_possible_frames"]
               or f["in_flight_partial_frames"]}
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
                     drain_cell(f["drains"], "total"), unattributed_cell(f["unattributed"]),
                     tri(f["cold"]), tri(f["pool_grew"]), marks])
    out += table(["frame", "phase", "pose", "interval", "lightmap", "SH", "total",
                  "unattributed", "cold", "growth", "marks"], rows)
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
    # A truncated .xz raises EOFError; a corrupt one, LZMAError.
    except (ReduceError, OSError, ValueError, EOFError, ET.ParseError, lzma.LZMAError) as err:
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
