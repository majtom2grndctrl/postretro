#!/usr/bin/env python3
"""Summarize one Postretro telemetry file (`POSTRETRO_TELEMETRY` JSONL).

The engine and `prl-build` append one versioned JSON envelope per line. A
file may hold several runs from several processes, their lines interleaved.
This reducer joins them per run and per load:

* runs: source, version, argv, producer states (a later `producer_state`
  settles an earlier one) and the exit reason. A run without `run_end` was
  killed.
* bakes: source map, output `.prl` and stage durations. Durations come from
  `bake_summary.timings` only; `bake_stage` transitions overlap or begin late
  under the TUI, so they carry order, never time. A failed bake lists the
  stages it began.
* loads, keyed by run and generation: the load-phase table, geometry install,
  reveal or unload, and the bake whose output names the load's `.prl`. A
  generation missing between two seen ones was cancelled; a cancelled load
  emits nothing.
* windows, grouped by the level in their envelope: SH and lightmap miss
  buckets, CPU stage and GPU pass p50 and max, and camera-route playback
  windows (row counts, worst frame-time p99, frames over 33 ms, drain CPU max,
  late counts, pacing, and cap with how often it changed between windows). A
  null GPU mean or playback figure is absent, never zero. The 33 ms threshold
  is the engine's, not `playback_trace_reduce.py`'s `--budget-ms`.
* log record counts per level.

Standard library only. Usage:

    python3 tools/telemetry_reduce.py <file.jsonl> [--json]
"""

import argparse
import json
import statistics
import sys
from collections import Counter, OrderedDict
from pathlib import Path
from typing import Iterable

ENVELOPE_VERSION = 1
ENVELOPE_FIELDS = ("v", "run", "seq", "t_ms", "source", "level", "kind", "payload")

KNOWN_KINDS = frozenset({
    "run_start", "producer_state", "run_end", "log", "boot_phases", "mod_phases",
    "load_phases", "unload", "geometry_install", "sh_streaming_window",
    "lightmap_streaming_window", "cpu_timing_window", "gpu_timing_window",
    "bake_stage", "bake_summary", "playback_window",
})

# `playback_window` counts, summed across windows.
PLAYBACK_ROW_COUNTS = ("rows", "route_rows", "measured_rows", "excluded_rows")
# Nullable figures: a null is absent, never zero. Maxes and sums skip it.
PLAYBACK_MAXES = ("frame_time_p99_ms", "lightmap_drain_cpu_max_ms", "sh_drain_cpu_max_ms")
PLAYBACK_SUMS = ("frames_over_33ms", "lightmap_late_block_frames", "sh_late_cluster_frames")

SH_MISS_BUCKETS = (
    "misses", "miss_reach", "miss_pressure", "miss_in_flight", "miss_budget",
    "miss_compose", "miss_failed", "miss_device_limit", "miss_map_cap",
)
# Block-frames. The `miss_*` causes split `missed_not_resident`; SH's miss
# buckets count episodes, so the two never sum together.
LIGHTMAP_MISS_BUCKETS = (
    "missed_outside_baked_set", "missed_not_resident", "miss_reach", "miss_in_flight",
    "miss_permit", "miss_in_hand_cap", "miss_budget", "miss_renderer_drain", "miss_deferred",
    "miss_refused", "miss_failed",
)

NO_LEVEL = "no level"
KILLED = "killed"
REVEALED = "revealed"
UNLOADED_BEFORE_REVEAL = "unloaded before reveal"
NOT_REVEALED = "not revealed"
CANCELLED = "cancelled"


class MalformedLine(ValueError):
    pass


def parse_envelope(line):
    """Parse one line into an envelope dict, or raise MalformedLine."""
    try:
        env = json.loads(line)
    except json.JSONDecodeError as err:
        raise MalformedLine(f"not JSON: {err}") from None
    if not isinstance(env, dict):
        raise MalformedLine("not a JSON object")
    missing = [f for f in ENVELOPE_FIELDS if f not in env]
    if missing:
        raise MalformedLine("missing " + ", ".join(missing))
    if env["v"] != ENVELOPE_VERSION:
        raise MalformedLine(f"envelope version {env['v']!r}, expected {ENVELOPE_VERSION}")
    if not isinstance(env["run"], str) or not isinstance(env["kind"], str):
        raise MalformedLine("run and kind must be strings")
    if not isinstance(env["seq"], int) or not isinstance(env["payload"], dict):
        raise MalformedLine("seq must be an integer and payload an object")
    level = env["level"]
    if level is not None and not (isinstance(level, dict) and is_int(level.get("gen"))):
        raise MalformedLine("level must be null or carry an integer gen")
    return env


def is_int(value):
    # bool is an int subclass; it is never a count or a generation.
    return isinstance(value, int) and not isinstance(value, bool)


def is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def check(condition, what):
    if not condition:
        raise TypeError(what)


def check_rows(rows, what, name="name", value="ms", optional_value=False):
    """A list of objects each with a string `name` and a number `value`."""
    check(isinstance(rows, list), f"{what} is not a list")
    for row in rows:
        check(isinstance(row, dict) and isinstance(row.get(name), str), f"{what} row lacks {name}")
        number = row.get(value)
        if not (optional_value and number is None):
            check(is_number(number), f"{what} row {row.get(name)!r} has no numeric {value}")


def validate_payload(kind, payload):
    """Raise TypeError unless `payload` carries what `kind`'s reducer reads,
    so a bad line is counted before any of it is applied."""
    if kind == "run_start":
        for entry in payload.get("producers") or []:
            check(isinstance(entry, dict) and isinstance(entry.get("producer"), str),
                  "producer entry lacks producer")
    elif kind == "producer_state":
        check(isinstance(payload.get("producer"), str), "producer_state lacks producer")
    elif kind in ("load_phases", "geometry_install", "unload"):
        check(is_int(payload.get("generation")), f"{kind} generation is not an integer")
        if kind == "load_phases":
            check_rows(payload.get("phases"), "phases")
            for phase in payload["phases"]:
                check_rows(phase.get("marks") or [], "marks")
        elif kind == "geometry_install":
            check_rows(payload.get("phases") or [], "geometry phases")
    elif kind in ("sh_streaming_window", "lightmap_streaming_window"):
        delta = payload.get("delta") or {}
        check(isinstance(delta, dict), "delta is not an object")
        check(all(is_number(v) for v in delta.values()), "delta holds a non-number")
    elif kind == "cpu_timing_window":
        stages = [s for s in payload.get("stages") or [] if isinstance(s, dict)
                  and s.get("kind") == "time"]
        check_rows(stages, "cpu stages", name="label", value="average", optional_value=True)
        check_rows(stages, "cpu stages", name="label", value="max", optional_value=True)
    elif kind == "gpu_timing_window":
        check_rows(payload.get("passes") or [], "gpu passes", name="label", value="mean_ms",
                   optional_value=True)
    elif kind == "bake_summary":
        if payload.get("timings") is not None:
            check_rows(payload["timings"], "bake timings")
    elif kind == "playback_window":
        for name in PLAYBACK_ROW_COUNTS:
            check(is_int(payload.get(name)), f"playback_window {name} is not an integer")
        for name in PLAYBACK_MAXES + PLAYBACK_SUMS:
            value = payload.get(name)
            check(value is None or is_number(value), f"playback_window {name} is not a number")
        pacing = payload.get("pacing")
        check(pacing is None or isinstance(pacing, str), "playback_window pacing is not a string")
        cap = payload.get("cap")
        check(cap is None or isinstance(cap, dict), "playback_window cap is not an object")


def p50(values):
    """Median (statistics.median: an even count averages the middle two)."""
    return statistics.median(values) if values else None


def level_key(env):
    level = env["level"]
    return NO_LEVEL if level is None else level["gen"]


# ---------------------------------------------------------------- windows


class WindowGroup:
    """Windows of one run that name one level (or none) in their envelope."""

    def __init__(self, first_line=None):
        self.first_line = first_line
        self.map = None
        self.sh = {"windows": 0, "partial": 0, "buckets": Counter()}
        self.lightmap = {"windows": 0, "partial": 0, "buckets": Counter()}
        self.cpu = TimingSeries()
        self.gpu = TimingSeries()
        self.playback = PlaybackSeries()

    def add(self, env):
        kind, payload = env["kind"], env["payload"]
        if env["level"] is not None and self.map is None:
            self.map = env["level"].get("map")
        if kind == "sh_streaming_window":
            add_streaming(self.sh, payload, SH_MISS_BUCKETS)
        elif kind == "lightmap_streaming_window":
            add_streaming(self.lightmap, payload, LIGHTMAP_MISS_BUCKETS)
        elif kind == "cpu_timing_window":
            self.cpu.add_cpu(payload)
        elif kind == "gpu_timing_window":
            self.gpu.add_gpu(payload)
        elif kind == "playback_window":
            self.playback.add(payload)

    def summary(self):
        return {
            "map": self.map,
            "sh": streaming_summary(self.sh, SH_MISS_BUCKETS),
            "lightmap": streaming_summary(self.lightmap, LIGHTMAP_MISS_BUCKETS),
            "cpu": self.cpu.cpu_summary(),
            "gpu": self.gpu.gpu_summary(),
            "playback": self.playback.summary(),
        }


class PlaybackSeries:
    """`playback_window` events: counts summed, nullable figures folded so a
    window's null never reads as zero. Pacings are listed once each, in window
    order. Caps are listed in window order, a repeat of the previous window's
    cap left out, so `cap_changes` counts windows whose cap differs from the
    previous window's."""

    def __init__(self):
        self.windows = 0
        self.partial = 0
        self.counts = Counter()
        self.maxes = {name: None for name in PLAYBACK_MAXES}
        self.sums = {name: None for name in PLAYBACK_SUMS}
        self.pacings = []
        self.caps = []

    def add(self, payload):
        self.windows += 1
        pacing = payload.get("pacing")
        if pacing is not None and pacing not in self.pacings:
            self.pacings.append(pacing)
        cap = payload.get("cap")
        if cap is not None and (not self.caps or cap != self.caps[-1]):
            self.caps.append(cap)
        if payload.get("partial"):
            self.partial += 1
        for name in PLAYBACK_ROW_COUNTS:
            self.counts[name] += payload[name]
        for name in PLAYBACK_MAXES:
            value = payload.get(name)
            if value is not None:
                self.maxes[name] = value if self.maxes[name] is None else max(self.maxes[name], value)
        for name in PLAYBACK_SUMS:
            value = payload.get(name)
            if value is not None:
                self.sums[name] = (self.sums[name] or 0) + value

    def summary(self):
        out = {"windows": self.windows, "partial_windows": self.partial}
        out.update({name: self.counts[name] for name in PLAYBACK_ROW_COUNTS})
        out["frame_time_p99_ms_max"] = self.maxes["frame_time_p99_ms"]
        out["lightmap_drain_cpu_max_ms"] = self.maxes["lightmap_drain_cpu_max_ms"]
        out["sh_drain_cpu_max_ms"] = self.maxes["sh_drain_cpu_max_ms"]
        out.update(self.sums)
        out["pacings"] = list(self.pacings)
        out["caps"] = [dict(cap) for cap in self.caps]
        out["cap_changes"] = max(len(self.caps) - 1, 0)
        return out


def add_streaming(acc, payload, buckets):
    acc["windows"] += 1
    if payload.get("partial"):
        acc["partial"] += 1
    delta = payload.get("delta") or {}
    for name in buckets:
        acc["buckets"][name] += delta.get(name) or 0


def streaming_summary(acc, buckets):
    out = {"windows": acc["windows"], "partial_windows": acc["partial"]}
    out.update({name: acc["buckets"][name] for name in buckets})
    return out


class TimingSeries:
    """Per-label series across CPU or GPU windows, in first-seen order."""

    def __init__(self):
        self.windows = 0
        self.rows = OrderedDict()

    def add_cpu(self, payload):
        self.windows += 1
        for stage in payload.get("stages") or []:
            # Count and marker rows carry no milliseconds.
            if stage.get("kind") != "time":
                continue
            row = self.rows.setdefault(stage["label"], {
                "parent": stage.get("parent"),
                "aggregate": bool(stage.get("aggregate", False)),
                "averages": [],
                "maxes": [],
            })
            if stage.get("average") is not None:
                row["averages"].append(stage["average"])
            if stage.get("max") is not None:
                row["maxes"].append(stage["max"])

    def add_gpu(self, payload):
        self.windows += 1
        for gpu_pass in payload.get("passes") or []:
            row = self.rows.setdefault(gpu_pass["label"], {"means": [], "absent": 0})
            if gpu_pass.get("mean_ms") is None:
                row["absent"] += 1
            else:
                row["means"].append(gpu_pass["mean_ms"])

    def cpu_summary(self):
        stages = []
        for label, row in self.rows.items():
            stages.append({
                "label": label,
                "parent": row["parent"],
                "aggregate": row["aggregate"],
                "windows": len(row["averages"]),
                "p50_average_ms": p50(row["averages"]),
                "max_average_ms": max(row["averages"]) if row["averages"] else None,
                "max_ms": max(row["maxes"]) if row["maxes"] else None,
            })
        return {"windows": self.windows, "stages": stages}

    def gpu_summary(self):
        passes = []
        for label, row in self.rows.items():
            passes.append({
                "label": label,
                "windows": len(row["means"]),
                "absent_windows": row["absent"],
                "p50_mean_ms": p50(row["means"]),
                "max_mean_ms": max(row["means"]) if row["means"] else None,
            })
        return {"windows": self.windows, "passes": passes}


# ----------------------------------------------------------------- reduce


def reduce_lines(lines: Iterable[str]) -> dict:
    """Summarize telemetry JSONL lines into a JSON-serializable dict."""
    lines_read = 0
    malformed = []
    by_run = OrderedDict()
    for number, raw in enumerate(lines, start=1):
        text = raw.strip()
        if not text:
            continue
        lines_read += 1
        try:
            env = parse_envelope(text)
        except MalformedLine as err:
            malformed.append({"line": number, "error": str(err)})
            continue
        by_run.setdefault(env["run"], []).append((number, env))

    unknown_kinds = Counter()
    runs, bakes, loads, gaps = [], [], [], []
    for run_id, entries in by_run.items():
        # One process writes its lines in sequence order; sort anyway so a
        # reordered file still settles producer states correctly.
        entries.sort(key=lambda entry: entry[1]["seq"])
        run, run_bakes, run_loads, run_gaps = reduce_run(
            run_id, entries, unknown_kinds, malformed
        )
        runs.append(run)
        bakes.extend(run_bakes)
        loads.extend(run_loads)
        gaps.extend(run_gaps)

    join_bakes(loads, bakes)
    malformed.sort(key=lambda bad: bad["line"])
    return {
        "lines": lines_read,
        "malformed": len(malformed),
        "malformed_lines": malformed,
        "unknown_kinds": dict(sorted(unknown_kinds.items())),
        "runs": runs,
        "bakes": bakes,
        "loads": loads,
        "generation_gaps": gaps,
    }


def reduce_run(run_id, entries, unknown_kinds, malformed):
    first_line = entries[0][0]
    run = {
        "run": run_id,
        "source": entries[0][1]["source"],
        "first_line": first_line,
        "epoch_ms": None,
        "version": None,
        "argv": None,
        "sink": None,
        "producers": OrderedDict(),
        "end": {"reason": KILLED, "detail": None},
        "events": len(entries),
        "seq_gaps": 0,
        "log_counts": Counter(),
        "boot_phases": None,
        "mod_phases": None,
    }
    loads = OrderedDict()
    groups = OrderedDict()
    totals = WindowGroup()
    bake = None
    ended = False
    expected_seq = 0

    def load_for(generation, number):
        return loads.setdefault(generation, new_load(run_id, generation, number))

    for number, env in entries:
        if env["seq"] != expected_seq:
            run["seq_gaps"] += 1
        expected_seq = env["seq"] + 1
        kind, payload = env["kind"], env["payload"]
        if kind not in KNOWN_KINDS:
            unknown_kinds[kind] += 1
            continue
        if ended:
            # Nothing is written after run_end; ignore anything that is.
            continue
        try:
            validate_payload(kind, payload)
            if kind == "run_start":
                run.update({
                    "epoch_ms": payload.get("epoch_ms"),
                    "version": payload.get("version"),
                    "argv": payload.get("argv"),
                    "sink": payload.get("sink"),
                })
                for entry in payload.get("producers") or []:
                    run["producers"][entry["producer"]] = producer_entry(entry)
            elif kind == "producer_state":
                run["producers"][payload["producer"]] = producer_entry(payload)
            elif kind == "run_end":
                run["end"] = {"reason": payload.get("reason"), "detail": payload.get("detail")}
                ended = True
            elif kind == "log":
                run["log_counts"][payload.get("level")] += 1
            elif kind in ("boot_phases", "mod_phases"):
                run[kind] = phase_table(payload)
            elif kind == "load_phases":
                load = load_for(payload["generation"], number)
                load["prl"] = payload.get("prl") or load["prl"]
                load["phases"] = phase_table(payload)
            elif kind == "geometry_install":
                load = load_for(payload["generation"], number)
                load["geometry_install"] = {
                    "has_geometry": payload.get("has_geometry"),
                    "total_ms": payload.get("total_ms"),
                    "phases": [{"name": p["name"], "ms": p["ms"]} for p in payload.get("phases") or []],
                    "other_ms": payload.get("other_ms"),
                }
            elif kind == "unload":
                load = load_for(payload["generation"], number)
                load["prl"] = load["prl"] or payload.get("prl")
                load["unload_ms"] = payload.get("unload_ms")
            elif kind in ("sh_streaming_window", "lightmap_streaming_window",
                          "cpu_timing_window", "gpu_timing_window", "playback_window"):
                groups.setdefault(level_key(env), WindowGroup(number)).add(env)
                totals.add(env)
            elif kind in ("bake_stage", "bake_summary"):
                if bake is None:
                    bake = new_bake(run_id, number)
                add_bake_event(bake, kind, payload)
        except (KeyError, TypeError, AttributeError, ValueError) as err:
            # A whole envelope whose payload lacks what its kind promises:
            # counted, never fatal to the rest of the file.
            malformed.append({"line": number, "error": f"{kind} payload: {err!r}"})

    run["producers"] = dict(run["producers"])
    run["log_counts"] = dict(sorted(run["log_counts"].items(), key=lambda kv: str(kv[0])))
    run["cpu"] = totals.cpu.cpu_summary()
    run["gpu"] = totals.gpu.gpu_summary()

    # A window naming a generation with no load event still belongs to it.
    for key, group in groups.items():
        if key != NO_LEVEL:
            load_for(key, group.first_line)
    for generation, load in loads.items():
        group = groups.get(generation)
        load["windows"] = group.summary() if group else WindowGroup().summary()
        if load["prl"] is None and group is not None:
            load["prl"] = group.map
        load["status"] = load_status(load)
    run["no_level_windows"] = groups[NO_LEVEL].summary() if NO_LEVEL in groups else None

    gaps = []
    if loads:
        seen = sorted(loads)
        missing = sorted(set(range(seen[0], seen[-1] + 1)) - set(seen))
        gaps = [{"run": run_id, "generation": g, "status": CANCELLED} for g in missing]

    ordered_loads = [loads[g] for g in sorted(loads)]
    return run, ([finish_bake(bake)] if bake else []), ordered_loads, gaps


def producer_entry(entry):
    return {
        "state": entry.get("state"),
        "variable": entry.get("variable"),
        "reason": entry.get("reason"),
    }


def phase_table(payload):
    return [
        {
            "name": phase["name"],
            "ms": phase["ms"],
            "marks": [{"name": m["name"], "ms": m["ms"]} for m in phase.get("marks") or []],
        }
        for phase in payload.get("phases") or []
    ]


def new_load(run_id, generation, first_line):
    return {
        "run": run_id,
        "generation": generation,
        "first_line": first_line,
        "prl": None,
        "status": None,
        "phases": None,
        "geometry_install": None,
        "unload_ms": None,
        "bake": None,
        "windows": None,
    }


def load_status(load):
    if load["phases"] is not None:
        return REVEALED
    if load["unload_ms"] is not None:
        return UNLOADED_BEFORE_REVEAL
    # Geometry install or windows only: the run ended (or was killed) before
    # the reveal, with no unload written.
    return NOT_REVEALED


# ------------------------------------------------------------------ bakes


def new_bake(run_id, first_line):
    return {
        "run": run_id,
        "first_line": first_line,
        "source": None,
        "output": None,
        "success": None,
        "stages": [],
        "non_stage": [],
        "total_ms": None,
        "began": [],
        "skipped": [],
        "background": [],
    }


def add_bake_event(bake, kind, payload):
    if kind == "bake_stage":
        stage, transition = payload.get("stage"), payload.get("transition")
        if transition == "begin":
            bake["began"].append(stage)
            if payload.get("background"):
                bake["background"].append(stage)
        elif transition == "skip":
            bake["skipped"].append(stage)
        return
    bake["source"] = payload.get("source")
    bake["output"] = payload.get("output")
    bake["success"] = payload.get("success")
    bake["total_ms"] = payload.get("total_ms")
    timings = payload.get("timings")
    # Durations come from the summary alone. A failed bake carries none.
    for row in timings or []:
        target = bake["stages"] if row.get("stage") else bake["non_stage"]
        target.append({"name": row["name"], "ms": row["ms"]})


def finish_bake(bake):
    if bake["success"] is None:
        bake["status"] = "no summary"
    else:
        bake["status"] = "ok" if bake["success"] else "failed"
    return bake


def join_bakes(loads, bakes):
    """Join each load to the bake whose summary output equals its `.prl`.

    Paths compare as exact strings; both processes write absolute paths. With
    several candidates, the last bake whose first line precedes the load's
    first event wins, else the last in the file.
    """
    for load in loads:
        if load["prl"] is None:
            continue
        candidates = [b for b in bakes if b["output"] == load["prl"]]
        if not candidates:
            continue
        earlier = [b for b in candidates if b["first_line"] < load["first_line"]]
        chosen = (earlier or candidates)[-1]
        load["bake"] = {
            "run": chosen["run"],
            "source": chosen["source"],
            "success": chosen["success"],
            "stages": chosen["stages"],
            "total_ms": chosen["total_ms"],
        }


# ------------------------------------------------------------------- text


def fmt_ms(value):
    return "-" if value is None else f"{value:.2f}"


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


def render_phases(phases, indent="    "):
    rows = []
    for phase in phases:
        marks = ", ".join(f"{m['name']}={fmt_ms(m['ms'])}" for m in phase["marks"])
        rows.append([phase["name"], fmt_ms(phase["ms"]), marks])
    return table(["phase", "ms", "marks"], rows, indent)


def render_windows(windows, indent="    "):
    out = []
    sh, lm = windows["sh"], windows["lightmap"]
    if sh["windows"]:
        out.append(f"{indent}SH windows: {sh['windows']} ({sh['partial_windows']} partial)")
        out += table(list(SH_MISS_BUCKETS), [[sh[b] for b in SH_MISS_BUCKETS]], indent + "  ")
    if lm["windows"]:
        out.append(f"{indent}lightmap windows: {lm['windows']} ({lm['partial_windows']} partial)")
        out += table(list(LIGHTMAP_MISS_BUCKETS), [[lm[b] for b in LIGHTMAP_MISS_BUCKETS]],
                     indent + "  ")
    out += render_timing(windows["cpu"], windows["gpu"], indent)
    out += render_playback(windows["playback"], indent)
    return out


def render_playback(playback, indent):
    if not playback["windows"]:
        return []

    def count(value):
        return "-" if value is None else str(value)

    def cap_text(cap):
        return f"{cap.get('source')} {cap.get('decoded_bytes_per_drain')} B"

    out = [f"{indent}playback windows: {playback['windows']} ({playback['partial_windows']} partial);"
           f" rows {playback['rows']}, route {playback['route_rows']}, measured "
           f"{playback['measured_rows']}, excluded {playback['excluded_rows']}"]
    caps = " -> ".join(cap_text(cap) for cap in playback["caps"]) or "-"
    changed = (f" (changed between windows {playback['cap_changes']} time(s))"
               if playback["cap_changes"] else "")
    out.append(f"{indent}  pacing {', '.join(playback['pacings']) or '-'}; cap {caps}{changed}")
    out += table(["p99 max", "over 33 ms", "lightmap drain CPU max", "SH drain CPU max",
                  "late lightmap block-frames", "late SH cluster-frames"],
                 [[fmt_ms(playback["frame_time_p99_ms_max"]), count(playback["frames_over_33ms"]),
                   fmt_ms(playback["lightmap_drain_cpu_max_ms"]),
                   fmt_ms(playback["sh_drain_cpu_max_ms"]),
                   count(playback["lightmap_late_block_frames"]),
                   count(playback["sh_late_cluster_frames"])]], indent + "  ")
    return out


def render_timing(cpu, gpu, indent):
    out = []
    if cpu["windows"]:
        out.append(f"{indent}CPU windows: {cpu['windows']}")
        rows = [[s["label"], s["parent"] or "", "yes" if s["aggregate"] else "",
                 fmt_ms(s["p50_average_ms"]), fmt_ms(s["max_average_ms"]), fmt_ms(s["max_ms"])]
                for s in cpu["stages"]]
        out += table(["stage", "parent", "agg", "p50 avg", "max avg", "max"], rows, indent + "  ")
    if gpu["windows"]:
        out.append(f"{indent}GPU windows: {gpu['windows']}")
        rows = [[p["label"], fmt_ms(p["p50_mean_ms"]), fmt_ms(p["max_mean_ms"]),
                 p["windows"], p["absent_windows"]] for p in gpu["passes"]]
        out += table(["pass", "p50 mean", "max mean", "sampled", "absent"], rows, indent + "  ")
    return out


def render_text(summary: dict) -> str:
    out = [
        f"lines: {summary['lines']}  malformed: {summary['malformed']}  "
        f"unknown kinds: {sum(summary['unknown_kinds'].values())}"
    ]
    for bad in summary["malformed_lines"]:
        out.append(f"  malformed line {bad['line']}: {bad['error']}")
    for kind, count in summary["unknown_kinds"].items():
        out.append(f"  unknown kind {kind}: {count}")

    out.append("")
    out.append("Runs")
    for run in summary["runs"]:
        end = run["end"]
        reason = end["reason"] + (f" ({end['detail']})" if end["detail"] else "")
        out.append(f"  {run['run']}  {run['source']}  v{run['version']}  end: {reason}")
        if run["argv"] is not None:
            out.append(f"    argv: {' '.join(run['argv'])}")
        if run["seq_gaps"]:
            out.append(f"    sequence gaps: {run['seq_gaps']}")
        rows = [[name, p["state"], p["variable"] or "", p["reason"] or ""]
                for name, p in run["producers"].items()]
        if rows:
            out += table(["producer", "state", "variable", "reason"], rows, "    ")
        if run["log_counts"]:
            counts = "  ".join(f"{level}={n}" for level, n in run["log_counts"].items())
            out.append(f"    log records: {counts}")
        for kind in ("boot_phases", "mod_phases"):
            if run[kind]:
                out.append(f"    {kind}:")
                out += render_phases(run[kind], "      ")
        if run["cpu"]["windows"] or run["gpu"]["windows"]:
            out.append("    timing across the run:")
            out += render_timing(run["cpu"], run["gpu"], "      ")
        if run["no_level_windows"]:
            out.append(f"    windows with {NO_LEVEL}:")
            out += render_windows(run["no_level_windows"], "      ")

    if summary["bakes"]:
        out.append("")
        out.append("Bakes")
        for bake in summary["bakes"]:
            out.append(f"  {bake['run']}  {bake['status']}  {bake['source']} -> {bake['output']}")
            if bake["stages"]:
                out += table(["stage", "ms"], [[s["name"], fmt_ms(s["ms"])] for s in bake["stages"]],
                             "    ")
                out.append(f"    total_ms: {fmt_ms(bake['total_ms'])}")
            else:
                out.append(f"    no durations; stages begun: {', '.join(bake['began']) or 'none'}")
            if bake["non_stage"]:
                out.append("    non-stage rows:")
                out += table(["row", "ms"],
                             [[s["name"].strip(), fmt_ms(s["ms"])] for s in bake["non_stage"]],
                             "      ")
            if bake["skipped"]:
                out.append(f"    skipped: {', '.join(bake['skipped'])}")

    if summary["loads"]:
        out.append("")
        out.append("Loads")
        for load in summary["loads"]:
            out.append(f"  {load['run']}  gen {load['generation']}  {load['status']}  {load['prl']}")
            if load["phases"]:
                out += render_phases(load["phases"])
            install = load["geometry_install"]
            if install:
                phases = ", ".join(f"{p['name']}={fmt_ms(p['ms'])}" for p in install["phases"])
                out.append(f"    geometry install: total={fmt_ms(install['total_ms'])} "
                           f"({phases}, other={fmt_ms(install['other_ms'])})")
            if load["unload_ms"] is not None:
                out.append(f"    unload: {fmt_ms(load['unload_ms'])} ms")
            bake = load["bake"]
            if bake:
                out.append(f"    baked by {bake['run']} from {bake['source']}"
                           f" ({'ok' if bake['success'] else 'failed'})")
                if bake["stages"]:
                    stages = ", ".join(f"{s['name']}={fmt_ms(s['ms'])}" for s in bake["stages"])
                    out.append(f"      stages: {stages}")
            else:
                out.append("    no bake in this file")
            out += render_windows(load["windows"])

    if summary["generation_gaps"]:
        out.append("")
        out.append("Generation gaps")
        for gap in summary["generation_gaps"]:
            out.append(f"  {gap['run']}  gen {gap['generation']}  {gap['status']}")

    return "\n".join(out) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Summarize one Postretro telemetry JSONL file per run and per load.")
    parser.add_argument("file", type=Path, help="telemetry file (POSTRETRO_TELEMETRY output)")
    parser.add_argument("--json", action="store_true", help="print the summary as JSON")
    args = parser.parse_args(argv)
    try:
        with args.file.open(encoding="utf-8", errors="replace") as handle:
            summary = reduce_lines(handle)
    except OSError as err:
        parser.error(f"cannot read {args.file}: {err}")
    if args.json:
        json.dump(summary, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        sys.stdout.write(render_text(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
