"""Timestamp-query batch for the cell-rate spike, for adapters that support it.

usage: python measurements/cell-rate-spike/run_timestamps.py [--rounds N] [--only a,b]
       [--warmup N] [--samples N] [--tag NAME]

The Mac's 5300M has no timestamp queries, so run.py measures it with Metal
System Traces. Any adapter with TIMESTAMP_QUERY (the owner's GTX 1660 Super on
Windows) can take the same arms and views from the engine's own per-pass GPU
timing instead: POSTRETRO_GPU_TIMING=1 with a dev-tools build, read from each
capture report's `gpu_timing` windows (rendering_pipeline.md section 12). An
arm's cost is fluid_surface + fluid_cell, each pass's mean over the report's
complete 120-readback windows. No trace, no machine-state polling: keep the
machine idle yourself (nothing building, no second engine). Writes
results-<tag>.json and results-<tag>.md beside this script, plus count captures.
"""
import argparse, json, os, sys, tempfile, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run as kit  # noqa: E402

HERE = kit.HERE


def pass_ms(report, label):
    """A pass's mean over the report's windows, weighted by sampled readbacks,
    and the per-window means (their spread is the within-run variation)."""
    gt = report.get("gpu_timing") or {}
    per_window, total, n = [], 0.0, 0
    for w in gt.get("windows") or []:
        for p in w.get("passes") or []:
            if p.get("label") == label and p.get("average_ms") is not None and p.get("sampled_readbacks"):
                per_window.append(p["average_ms"])
                total += p["average_ms"] * p["sampled_readbacks"]
                n += p["sampled_readbacks"]
    return (total / n if n else None), per_window, gt.get("availability")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--only")
    ap.add_argument("--warmup", type=int, default=240)
    ap.add_argument("--samples", type=int, default=360)
    ap.add_argument("--tag", default="1660")
    ap.add_argument("--skip-stats", action="store_true")
    ap.add_argument("--resummarize", action="store_true",
                    help="run nothing: rebuild results-<tag> from every saved runs/ts-<tag>-*.json")
    args = ap.parse_args()
    if args.resummarize:
        args.rounds = 0
        args.skip_stats = True

    chosen = [a for a in kit.arms() if not args.only or a["name"] in args.only.split(",")]
    kit.RUNS.mkdir(exist_ok=True)
    stats = {} if args.skip_stats else kit.count_captures(args)
    records = []
    for rnd in range(1, args.rounds + 1):
        order = chosen if rnd % 2 else list(reversed(chosen))
        for a in order:
            label = f"{a['name']}-r{rnd}"
            time.sleep(float(os.environ.get("IDLE_GAP", "5")))
            with tempfile.TemporaryDirectory() as d:
                ok, rep, tail = kit.untraced(a, Path(d), args.warmup, args.samples,
                                             env_extra={"POSTRETRO_GPU_TIMING": "1"}, timeout=1800)
            rec = {"label": label, "arm": a["name"], "round": rnd, "pose": a["pose"], "res": a["res"],
                   "mode": a["mode"], "ratio": a["ratio"], "problems": []}
            if not ok:
                rec["problems"].append("capture failed: " + tail[-400:])
            else:
                fluid, fw, avail = pass_ms(rep, "fluid_surface")
                cell, cw, _ = pass_ms(rep, "fluid_cell")
                fwd, _, _ = pass_ms(rep, "forward")
                rec.update({"adapter": rep.get("adapter"), "gpu_timing": avail, "fluid_pass_ms": fluid,
                            "cell_pass_ms": cell or 0.0, "fluid_windows_ms": fw, "cell_windows_ms": cw,
                            "forward_ms": fwd, "fluid_ms": (fluid + (cell or 0.0)) if fluid is not None else None,
                            "cpu_completion_median_ms": (rep.get("cpu_completion") or {}).get("median_ms"),
                            "basis": "timestamps"})
                if avail != "available" and fluid is None:
                    rec["problems"].append(f"gpu timing {avail}")
                if a["cell_buffer"] and cell is None:
                    rec["problems"].append("cell arm without fluid_cell timing")
                if not a["cell_buffer"] and cell:
                    rec["problems"].append("per-fragment arm recorded fluid_cell timing")
            rec["valid"] = not rec["problems"]
            records.append(rec)
            (kit.RUNS / f"ts-{args.tag}-{label}.json").write_text(json.dumps(rec, indent=2) + "\n")
            print(f"{label}: cost {kit.fmt(rec.get('fluid_ms'), 4)} ms (fluid {kit.fmt(rec.get('fluid_pass_ms'), 4)} "
                  f"+ cell {kit.fmt(rec.get('cell_pass_ms'), 4)}) valid={rec['valid']}", flush=True)

    meta = {"git_rev": kit.git("rev-parse", "HEAD"), "git_branch": kit.git("rev-parse", "--abbrev-ref", "HEAD"),
            "git_dirty": bool(kit.git("status", "--porcelain")), "binary": str(kit.engine_binary()),
            "machine": args.tag, "warmup": args.warmup, "samples": args.samples, "idle": None,
            "adapter": next((r.get("adapter") for r in records if r.get("adapter")), None),
            "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    # Every saved run of this tag, so a partial rerun (--only) merges with the
    # batch it extends; a rerun label replaces its old record.
    saved = {}
    for f in sorted(kit.RUNS.glob(f"ts-{args.tag}-*.json")):
        r = json.loads(f.read_text())
        saved[r["label"]] = r
    records = sorted(saved.values(), key=lambda r: (r["round"], r["label"]))
    stats_path = kit.RUNS / "count-stats.json"
    if not stats and stats_path.exists():
        stats = json.loads(stats_path.read_text())
    meta["adapter"] = meta["adapter"] or next((r.get("adapter") for r in records if r.get("adapter")), None)
    for r in records:
        r.setdefault("clock", None)
    kit.summarize(records, meta, stats, name=f"results-{args.tag}", title=f"{args.tag} timestamp-query results")
    print((HERE / f"results-{args.tag}.md").read_text())
    return 0


if __name__ == "__main__":
    sys.exit(main())
