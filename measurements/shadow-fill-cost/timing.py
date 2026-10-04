"""Median CPU stage averages over a run's complete [CpuTiming] windows.

usage: timing.py <run.log> [discard=3]
A stage's avg covers only the frames it ran in; `(ran/frames)` is reported.
"""
import json, re, statistics, sys

STAGES = ["work", "render_record", "rec_shadow_depth", "rec_shadow_reach", "rec_cull",
          "render_submit", "wait_acquire", "total"]
path, discard = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 3
windows = [line for line in open(path, errors="replace") if "[CpuTiming]" in line]
kept = windows[discard:]
result = {"windows_total": len(windows), "windows_used": len(kept), "stages": {}}
for stage in STAGES:
    avgs, ran = [], []
    for line in kept:
        m = re.search(rf"\b{stage}=([0-9.]+)/([0-9.]+)ms(?:\((\d+)/(\d+)\))?", line)
        if m:
            avgs.append(float(m[1]))
            ran.append(int(m[3]) if m[3] else 120)
    if avgs:
        result["stages"][stage] = {"median_avg_ms": round(statistics.median(avgs), 4),
                                   "window_avgs_ms": avgs,
                                   "median_frames_ran": statistics.median(ran)}
print(json.dumps(result, indent=2))
