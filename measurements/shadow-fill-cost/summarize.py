"""Aggregate clean before/after runs into one results table.

usage: summarize.py <map-key>... > results.json
Per run: median of window averages over windows 3+ (timing.py, discard 2).
Per build: median across runs. Runs that saw a screen saver or lost the
foreground are reported and excluded.
"""
import json, statistics, subprocess, sys
from pathlib import Path

here = Path(__file__).resolve().parent
runs = here / "runs"
STAGES = ["total", "wait_acquire", "work", "render_submit", "rec_shadow_depth", "rec_shadow_reach"]
out = {}
for key in sys.argv[1:]:
    out[key] = {}
    for build in ("before", "after"):
        per_run, excluded = [], []
        for record in sorted(runs.glob(f"{key}-{build}-clean[0-9].run.json")):
            meta = json.loads(record.read_text())
            label = meta["label"]
            if meta.get("screensaver_seen") or not meta["foreground_all"]:
                excluded.append(label)
                continue
            timing = json.loads(subprocess.run(
                ["python3", str(here / "timing.py"), str(runs / f"{label}.log"), "2"],
                capture_output=True, text=True, check=True).stdout)
            per_run.append({s: timing["stages"][s]["median_avg_ms"]
                            for s in STAGES if s in timing["stages"]} |
                           {"label": label, "windows_used": timing["windows_used"]})
        gpu_path = runs / f"{key}-{build}-cleantrace-gpu.json"
        trace_meta = runs / f"{key}-{build}-cleantrace.run.json"
        gpu = json.loads(gpu_path.read_text()) if gpu_path.exists() else None
        out[key][build] = {
            "runs": per_run,
            "excluded": excluded,
            "median_across_runs_ms": {s: round(statistics.median(r[s] for r in per_run), 4)
                                      for s in STAGES if per_run and all(s in r for r in per_run)},
            "trace": None if gpu is None else {
                "camera_frames": gpu["camera_frames"],
                "screensaver_seen": json.loads(trace_meta.read_text()).get("screensaver_seen"),
                "shadow_total_per_frame_ms": gpu["shadow_total_per_frame_ms"],
                "shadow_passes": gpu["shadow_passes"],
                "sum_all_passes_per_frame_ms": round(sum(v for v in gpu["all_passes_per_frame_ms"].values() if v), 3),
            },
        }
print(json.dumps(out, indent=2))
