"""Reduce runs/*.log: per run, median of the final five of eight [CpuTiming] windows.

usage: python3 reduce.py <label> [<label> ...]   (labels without extension)
Prints JSON per run and writes runs/<label>.reduced.json.
"""
import json, re, statistics, sys
from pathlib import Path

RUNS = Path(__file__).resolve().parent / "runs"
ROWS = ["total", "work", "wait", "render", "render_record", "rec_ui", "render_submit", "ui_text_spans_prepared"]


def parse(line):
    out = {}
    for m in re.finditer(r" ([a-z_]+)=([\d.]+)(?:ms)?/([\d.]+)(?:ms)?(?:\((\d+)/(\d+)\))?", line):
        out[m[1]] = {"avg": float(m[2]), "max": float(m[3]), "ran": int(m[4]) if m[4] else None}
    return out


for label in sys.argv[1:]:
    lines = [l for l in (RUNS / f"{label}.log").read_text(errors="replace").splitlines() if "[CpuTiming]" in l]
    wins = [parse(l) for l in lines[:8]]
    final = wins[3:8]
    res = {"label": label, "windows": len(wins)}
    for r in ROWS:
        vals = [w[r]["avg"] for w in final if r in w]
        if vals:
            res[r] = {"median": statistics.median(vals), "avgs": vals, "maxes": [w[r]["max"] for w in final if r in w]}
        else:
            res[r] = None
    # all eight windows' prepare counts, to see settle behaviour
    res["ui_text_spans_prepared_all_windows"] = [(w.get("ui_text_spans_prepared") or {}).get("max") for w in wins]
    res["total_avg_all_windows"] = [w["total"]["avg"] for w in wins if "total" in w]
    (RUNS / f"{label}.reduced.json").write_text(json.dumps(res, indent=2) + "\n")
    print(json.dumps({k: (v["median"] if isinstance(v, dict) and "median" in v else v) for k, v in res.items()}))
