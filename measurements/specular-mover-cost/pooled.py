"""Pool per-frame pass times across valid runs: median and p95 per arm, per-run medians,
and each arm's delta against the old build and against HEAD in the same style.

usage: python3 pooled.py <out tag> <results tag> [<results tag> ...]
env:   SPIKE_OUT  directory holding runs/ and results-<tag>.json (default: this directory)

An arm is <pose>:<variant>-<style>. The baselines are <pose>:old-none and
<pose>:head-<style>. Adapted from the specular-texel-sharpness Task 9 kit.
"""
import collections
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = Path(os.environ.get("SPIKE_OUT", str(HERE)))
out_tag, tags = sys.argv[1], sys.argv[2:]
KEYS = (("Kinematic Brush Pass", "mover"), ("Textured Pass", "forward"), ("frame", "GPU frame"))
pool = collections.defaultdict(lambda: collections.defaultdict(list))
perrun = collections.defaultdict(lambda: collections.defaultdict(list))
runs = collections.Counter()
for tag in tags:
    res = json.loads((OUT / f"results-{tag}.json").read_text())
    for r in res["runs"]:
        if not r["valid"]:
            continue
        g = json.loads((OUT / "runs" / f"{r['label']}-gpu.json").read_text())
        arm = f"{r['pose']}:{r['binary_arm']}"
        runs[arm] += 1
        for k, _ in KEYS:
            v = sorted(g["per_frame_ns"][k])
            pool[arm][k] += v
            perrun[arm][k].append(v[len(v) // 2] / 1e6)


def q(v, f):
    return sorted(v)[min(len(v) - 1, int(len(v) * f))] / 1e6


out = {}
for arm in pool:
    row = {"runs": runs[arm], "frames": len(pool[arm]["frame"])}
    for k, n in KEYS:
        v = pool[arm][k]
        row[n] = {"median": round(q(v, .5), 3), "p95": round(q(v, .95), 3),
                  "per_run_median": [round(x, 3) for x in perrun[arm][k]]}
    out[arm] = row


def delta(a, b, n):
    A, B = out.get(a), out.get(b)
    if not A or not B or a == b:
        return None
    d = A[n]["median"] - B[n]["median"]
    ds = [x - y for x in A[n]["per_run_median"] for y in B[n]["per_run_median"]]
    return {"ms": round(d, 3), "pct": round(d / B[n]["median"] * 100, 1),
            "pairs_min": round(min(ds), 3), "pairs_max": round(max(ds), 3)}


def cell(d):
    if d is None:
        return "—"
    return f"{d['ms']:+.3f} ({d['pct']:+.1f}%) [{d['pairs_min']:+.3f}..{d['pairs_max']:+.3f}]"


lines = []
for pose in sorted({a.split(":")[0] for a in out}):
    arms = sorted((a for a in out if a.startswith(pose + ":")),
                  key=lambda a: (a.split(":")[1] != "old-none", a.rsplit("-", 1)[1], a))
    lines += [f"### {pose}", "",
              "| arm | runs | mover median / p95 (ms) | mover per-run medians | Δ mover vs old | "
              "Δ mover vs HEAD same style | forward median | GPU frame median |",
              "|---|---|---|---|---|---|---|---|"]
    for arm in arms:
        row = out[arm]
        style = arm.rsplit("-", 1)[1]
        m = row["mover"]
        d_old = delta(arm, f"{pose}:old-none", "mover")
        d_head = delta(arm, f"{pose}:head-{style}", "mover") if style != "none" else None
        row["delta_mover_vs_old"], row["delta_mover_vs_head"] = d_old, d_head
        pr = m["per_run_median"]
        lines.append(f"| {arm.split(':')[1]} | {row['runs']} | {m['median']:.3f} / {m['p95']:.3f} | "
                     f"{min(pr):.3f}–{max(pr):.3f} | {cell(d_old)} | {cell(d_head)} | "
                     f"{row['forward']['median']:.3f} | {row['GPU frame']['median']:.3f} |")
    lines.append("")
lines.append("Deltas are differences of pooled medians; brackets give the min..max over every "
             "pair of per-run medians.")
(OUT / f"pooled-{out_tag}.json").write_text(json.dumps(out, indent=2) + "\n")
(OUT / f"pooled-{out_tag}.md").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
