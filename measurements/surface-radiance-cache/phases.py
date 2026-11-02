#!/usr/bin/env python3
"""Split the SH Bake wall time from a timestamped stderr log (run-bake-ts.sh).

The compiler logs no timed line per surface-radiance phase; the SH Bake progress
bar is the only signal. Patch shading advances it a fraction of a percent over
most of the stage (plateau); the first sample that jumps >= 5 points marks the
hand-off to the rest (later bounces + gather), which fills the remaining percent.
usage: phases.py <runs-dir> <label>...  -> JSON"""
import json, re, sys, os
d = sys.argv[1]; out = {}
for label in sys.argv[2:]:
    t_start = t_end = None; pts = []
    for l in open(os.path.join(d, label + ".err.log"), errors="replace"):
        m = re.match(r"\s*([\d.]+)\s+[\d.]+s\s+SH volume bake\.\.\.", l)
        if m and t_start is None: t_start = float(m.group(1))
        m = re.match(r"\s*([\d.]+)\s+[\d.]+s\s+Delta SH volume bake\.\.\.", l)
        if m and t_end is None: t_end = float(m.group(1))
        m = re.match(r"\s*([\d.]+)\s+SH Bake:\s+(\d+)%", l)
        if m: pts.append((float(m.group(1)), int(m.group(2))))
    if t_start is None or t_end is None or not pts:
        out[label] = {"error": "stage markers missing"}; continue
    split = None; prev = 0
    for t, p in pts:
        if p - prev >= 5: split = t; break
        prev = p
    r = {"sh_bake_s": round(t_end - t_start, 2), "t_sh_start": t_start, "t_sh_end": t_end}
    if split is not None:
        # the sample is taken at the end of its 0.5 s tick, so the hand-off is within +-0.5 s
        r["patch_plateau_s"] = round(split - t_start, 2)
        r["rest_s (later bounces + gather)"] = round(t_end - split, 2)
    out[label] = r
print(json.dumps(out, indent=1))
