"""Paired A/B deltas per arm for one or more PAIRED=1 batches.

usage: summarize_paired.py <batch-key>... > paired.json
Within each valid run, A (baseline) and B (the arm) alternate per frame, so
the run's delta B - A per pass carries no launch-to-launch regime. Each arm
reports per-run A, B and delta, the median delta and its min..max spread.
The launch's regime marker (Billboard Direct Scatter Compose per frame) is
recorded beside each run.
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
        if not record["valid"] or not gpu_path.exists() or record.get("arms_b") is None:
            batch["discarded"].append({"label": label, "valid": record["valid"],
                                       "rows_stable": record.get("rows_stable"),
                                       "trace": gpu_path.exists()})
            continue
        gpu = json.loads(gpu_path.read_text())
        per = gpu["compose_per_encoder_ms"]
        run = {"label": label, "arms_b": record["arms_b"],
               "regime_marker_ms": gpu["all_passes_per_frame_ms"].get("Billboard Direct Scatter Compose"),
               "gpu_state": record.get("gpu_state_median")}
        for p, name in PASSES.items():
            a, b = per.get(name), per.get(f"{name} [B]")
            run[p] = {"a": a, "b": b, "delta": round(b - a, 4) if a is not None and b is not None else None,
                      "encoders": [gpu["compose_encoder_counts"].get(name), gpu["compose_encoder_counts"].get(f"{name} [B]")]}
        entry = batch["arms"].setdefault(arm, {"runs": [], "row_mix": record.get("row_mix")})
        entry["runs"].append(run)
    for arm, entry in batch["arms"].items():
        for p in PASSES:
            deltas = [r[p]["delta"] for r in entry["runs"] if r[p]["delta"] is not None]
            a = [r[p]["a"] for r in entry["runs"] if r[p]["a"] is not None]
            entry[p] = ({"n": len(deltas), "median_a": round(statistics.median(a), 4),
                         "median_delta": round(statistics.median(deltas), 4),
                         "min_delta": min(deltas), "max_delta": max(deltas)} if deltas else None)
    out[key] = batch
print(json.dumps(out, indent=2))
