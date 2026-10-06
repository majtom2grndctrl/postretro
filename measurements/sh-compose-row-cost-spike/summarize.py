"""Per-arm per-pass GPU ms for one or more batches.

usage: summarize.py <batch-key>... > results.json
A run counts only if its run.json is valid (foreground, unlocked, no screen
saver, stable composed rows, arms logged) and its trace was reduced. Each
arm reports every valid run's per-frame ms per pass, the median and the
spread (min..max), plus the row mix the runs logged.
"""
import json, re, statistics, sys
from pathlib import Path

runs = Path(__file__).resolve().parent / "runs"
PASSES = {"indirect": "Streamed SH Compose", "animated": "Streamed Animated Direct SH"}
out = {}
for key in sys.argv[1:]:
    batch = {"discarded": [], "arms": {}}
    for run_path in sorted(runs.glob(f"{key}-*-r*.run.json")):
        label = run_path.name[: -len(".run.json")]
        arm = re.match(rf"{re.escape(key)}-(.+)-r\d+$", label)[1]
        record = json.loads(run_path.read_text())
        gpu_path = runs / f"{label}-gpu.json"
        if not record["valid"] or not gpu_path.exists():
            batch["discarded"].append({"label": label, "valid": record["valid"],
                                       "rows_stable": record.get("rows_stable"),
                                       "trace": gpu_path.exists()})
            continue
        gpu = json.loads(gpu_path.read_text())
        entry = batch["arms"].setdefault(arm, {"runs": [], "row_mix": record.get("row_mix")})
        entry["runs"].append({"label": label, "camera_frames": gpu["camera_frames"],
                              **{p: gpu["all_passes_per_frame_ms"].get(name)
                                 for p, name in PASSES.items()}})
        if entry["row_mix"] != record.get("row_mix"):
            entry["row_mix_mismatch"] = True
    for arm, entry in batch["arms"].items():
        for p in PASSES:
            values = [r[p] for r in entry["runs"] if r[p] is not None]
            entry[p] = ({"n": len(values), "median": round(statistics.median(values), 4),
                         "min": min(values), "max": max(values)} if values else None)
    out[key] = batch
print(json.dumps(out, indent=2))
