#!/usr/bin/env python3
"""Aggregate runs.jsonl: view x config, mean of per-run medians, spread, run count."""
import json, sys, statistics

runs = [json.loads(l) for l in open(sys.argv[1])]
valid = [r for r in runs if not r["invalid"]]
print(f"runs {len(runs)}, valid {len(valid)}, invalid {[r['name'] for r in runs if r['invalid']]}")
cfgs = ["base", "off", "t4", "t6", "t8"]
views = sorted({r["view"] for r in valid})


def cell(metric):
    out = {}
    for v in views:
        for c in cfgs:
            xs = [metric(r) for r in valid if r["view"] == v and r["cfg"] == c]
            xs = [x for x in xs if x is not None]
            if xs:
                out[(v, c)] = (statistics.fmean(xs), max(xs) - min(xs), len(xs), xs)
    return out


fw = cell(lambda r: r["passes"]["Textured Pass"]["median"])
busy = cell(lambda r: r["gpu_busy_median"])
comp = cell(lambda r: r["completion_median"])
other = {}
for r in valid:
    for k in r["passes"]:
        if any(s in k for s in ("Fog", "Fluid", "Billboard Sprite", "Kinematic", "Skinned")):
            other.setdefault(k, set()).add(r["view"])
print("SH-sharing passes seen:", other or "none")

for name, tab in [("forward (Textured Pass) ms", fw), ("GPU busy per frame ms", busy), ("completion ms", comp)]:
    print(f"\n{name}: mean of runs (spread max-min, n)")
    print("| View | " + " | ".join(cfgs) + " |")
    for v in views:
        row = []
        for c in cfgs:
            if (v, c) in tab:
                m, s, n, _ = tab[(v, c)]
                row.append(f"{m:.2f} ({s:.2f}, {n})")
            else:
                row.append("-")
        print(f"| {v} | " + " | ".join(row) + " |")
    print("delta vs base / vs off:")
    for v in views:
        b = tab[(v, "base")][0]; o = tab[(v, "off")][0]
        parts = [f"off {o-b:+.2f} ({(o-b)/b*100:+.1f}%)"]
        for c in ["t4", "t6", "t8"]:
            if (v, c) in tab:
                t = tab[(v, c)][0]
                parts.append(f"{c} {t-b:+.2f} ({(t-b)/b*100:+.1f}%) / {t-o:+.2f} ({(t-o)/o*100:+.1f}%)")
        print(f"  {v}: " + "; ".join(parts))

print("\nmachine state per config: temp pre / max, clock max, paging rows in window, vfree min MiB")
for c in cfgs:
    rs = [r for r in valid if r["cfg"] == c]
    if rs:
        print(c, "temp pre", sorted({r["pre"].get("temp") for r in rs}), "max", max(r["temp_max"] for r in rs),
              "clock max", sorted({r["clock_max"] for r in rs}), "paging", sorted({r["paging_rows_in_window"] for r in rs}),
              "vfree min", min((r["vfree_min"] or 0) for r in rs) >> 20,
              "forward max frame", round(max(r["passes"]["Textured Pass"]["max"] for r in rs), 2),
              "completion max", round(max(r["completion_max"] for r in rs), 1))
