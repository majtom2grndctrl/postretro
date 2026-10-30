"""Per-run stage table for the 9m release batch. usage: reduce.py <batch dir> [skip=2]
Mean of kept [CpuTiming] window averages per stage; GPU ms/frame from <tag>.gpu.txt when traced."""
import glob, os, re, statistics, sys
sys.path.insert(0, "/Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live")
from cpuwin import parse_line
D = sys.argv[1]; skip = int(sys.argv[2]) if len(sys.argv) > 2 else 2
COLS = ["total", "work", "visibility", "portal_walk", "walk_considered", "render_prep", "lightmap_residency",
        "lightmap_drain", "render_record", "rec_pre_scene", "rec_cull", "rec_ui", "render_sh_drain", "render_submit"]
SH = ["total", "work", "vis", "walk", "tests", "prep", "lm_res", "lm_drn", "record", "pre_scn", "rec_cul", "rec_ui", "sh_drn", "submit"]
rows = {}
for log in sorted(glob.glob(os.path.join(D, "*.rel.r*.log"))):
    tag = os.path.basename(log)[:-4]
    wins = [parse_line(l) for l in open(log, errors="replace") if "[CpuTiming]" in l][skip:skip + 6]
    vals = []
    for k in COLS:
        v = [w[k][1] for w in wins if k in w]
        vals.append(statistics.mean(v) if v else float("nan"))
    g = os.path.join(D, tag + ".gpu.txt"); gpu = float("nan"); gc = float("nan")
    if os.path.exists(g):
        t = open(g).read()
        m = re.search(r" gpu_ms_per_frame=([0-9.na]+)", t); gpu = float(m.group(1)) if m else gpu
        m = re.search(r"gfxcompute_ms_per_frame=([0-9.na]+)", t); gc = float(m.group(1)) if m else gc
    rows[tag] = (vals, gpu, gc, len(wins))
print("# ms (tests = walk_considered count), mean of kept window averages, first %d windows discarded, next 6 kept (pre-trace)" % skip)
print(f"{'tag':38s} " + " ".join(f"{s:>7s}" for s in SH) + "     gpu  gfxcmp  n")
for tag, (vals, gpu, gc, n) in rows.items():
    print(f"{tag:38s} " + " ".join(f"{v:7.3f}" if c != 'walk_considered' else f"{v:7.0f}" for v, c in zip(vals, COLS)) + f" {gpu:7.3f} {gc:7.3f} {n:2d}")
# per pose/side means over reps, and deltas
agg = {}
for tag, (vals, gpu, gc, n) in rows.items():
    m = re.match(r"(.+)\.(before|cap26)\.rel\.r\d+$", tag)
    agg.setdefault((m.group(1), m.group(2)), []).append(vals + [gpu])
print("\n# mean over repeats, and cap26 - before")
for pose in sorted({p for p, s in agg}):
    out = {}
    for side in ("before", "cap26"):
        if (pose, side) in agg:
            rs = agg[(pose, side)]
            out[side] = [statistics.mean([r[i] for r in rs if r[i] == r[i]]) if any(r[i] == r[i] for r in rs) else float("nan") for i in range(len(rs[0]))]
            print(f"{pose + '.' + side:38s} " + " ".join(f"{v:7.3f}" for v in out[side]) + f"  reps={len(rs)}")
    if len(out) == 2:
        print(f"{pose + '.delta':38s} " + " ".join(f"{b - a:+7.3f}" for a, b in zip(out['before'], out['cap26'])))
