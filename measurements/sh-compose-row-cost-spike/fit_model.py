"""Fit the per-pass compose cost model and project post-filter savings.

usage: fit_model.py > model.json
Inputs are the paired batches: baseline time T per pose = the A halves of
runs whose A is the baseline build (floor/X pairs are excluded); floor delta and stacked-lever delta
per pose = that arm's paired B - A. Row and entry counts come from the
`counts-*` runs' [SH spike counts] lines (same fixtures, same poses).

Model (all composed rows are L0 on every target pose; ids 27/45 are uniform
L0 by compiler policy, so the L1/L2 terms have no rows to fit):
  floor time   F = c + r * rows          (fixed per dispatch + per row)
  entry share  E = -delta_floor          (reported per entry and per lane-entry)
  T = F + E
Spread: 4000 bootstrap draws, each picking one run per pose for T, the floor
delta and the stacked delta; 5th..95th percentiles.
Post-filter projection: the contributing-row filter keeps only rows with
entries (rows_f). T_post = c + r*rows_f + E; the stacked lever saves
s * rows_f, where s = -delta_stack / rows is its per-row saving at that pose.
"""
import json, random, re, statistics
from pathlib import Path

here = Path(__file__).resolve().parent
runs = here / "runs"
POSES = {  # pose: (paired batch for T and floor, batch holding the stacked arm, counts run)
    "arena": ("arenaQ", "arenaR", "counts-arena"),
    "campaign": ("campaignP", "campaignP", "counts-campaign"),
    "kinematic": ("kinspawnP", "kinspawnP", "counts-kinematic"),
    "kinematicmid": ("kinmidP", "kinmidP", "counts-kinematicmid"),
}
STACK = "array-free+unroll36"
PASSES = {"indirect": ("Streamed SH Compose", "indirect"),
          "animated": ("Streamed Animated Direct SH", "animated-direct")}
COUNTS = re.compile(r"\[SH spike counts\] (\S+): .*last rows L0 (\d+) L1 (\d+) L2 (\d+) \| entries L0 (\d+) "
                    r"L1 (\d+) L2 (\d+) \| rows with entries (\d+) \| lane-entries (\d+)")


def paired(batch):
    out = {}
    for path in runs.glob(f"{batch}-*-r*.run.json"):
        record = json.loads(path.read_text())
        label = path.name[: -len(".run.json")]
        gpu_path = runs / f"{label}-gpu.json"
        if not record["valid"] or record.get("arms_b") is None or not gpu_path.exists():
            continue
        arm = re.match(rf"{batch}-(.+)-r\d+$", label)[1]
        # Only baseline/X pairs carry a baseline A half; floor/X pairs run the
        # floor as A, so they never feed T (spike review).
        if record.get("arms") != "baseline":
            continue
        per = json.loads(gpu_path.read_text())["compose_per_encoder_ms"]
        out.setdefault(arm, []).append({p: (per[name], per[f"{name} [B]"]) for p, (name, _) in PASSES.items()})
    return out


def counts(run):
    found = {}
    for m in COUNTS.finditer((runs / f"{run}.log").read_text(errors="replace")):
        found[m[1]] = {"rows": int(m[2]) + int(m[3]) + int(m[4]),
                       "entries": int(m[5]) + int(m[6]) + int(m[7]),
                       "rows_with_entries": int(m[8]), "lane_entries": int(m[9])}
    return found


data = {}
for pose, (batch, stack_batch, count_run) in POSES.items():
    p, s, c = paired(batch), paired(stack_batch), counts(count_run)
    data[pose] = {
        "T": {k: [r[k][0] for runs_ in p.values() for r in runs_] for k in PASSES},
        "floor": {k: [r[k][1] - r[k][0] for r in p["floor"]] for k in PASSES},
        "stack": {k: [r[k][1] - r[k][0] for r in s[STACK]] for k in PASSES},
        "counts": {k: c[log] for k, (_, log) in PASSES.items()},
    }


def ols(xs, ys):
    mx, my = statistics.fmean(xs), statistics.fmean(ys)
    slope = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)
    return my - slope * mx, slope


def fit(pick):
    model = {}
    for k in PASSES:
        rows = [data[q]["counts"][k]["rows"] for q in POSES]
        T = [pick(data[q]["T"][k]) for q in POSES]
        fl = [pick(data[q]["floor"][k]) for q in POSES]
        st = [pick(data[q]["stack"][k]) for q in POSES]
        c, r = ols(rows, [t + f for t, f in zip(T, fl)])
        poses = {}
        for q, t, f, sd in zip(POSES, T, fl, st):
            n = data[q]["counts"][k]
            E = -f
            per_row_saving = -sd / n["rows"]
            T_post = c + r * n["rows_with_entries"] + E
            poses[q] = {"T_ms": t, "floor_ms": t + f, "entry_share_ms": E,
                        "entry_share_pct": 100 * E / t,
                        "us_per_entry": 1000 * E / n["entries"] if n["entries"] else None,
                        "ns_per_lane_entry": 1e6 * E / n["lane_entries"] if n["lane_entries"] else None,
                        "fit_residual_ms": t + f - (c + r * n["rows"]),
                        "stack_saving_ms": -sd, "stack_saving_pct": 100 * -sd / t,
                        "post_filter_T_ms": T_post,
                        "post_filter_stack_saving_ms": per_row_saving * n["rows_with_entries"]}
        model[k] = {"c_ms": c, "r_us_per_row": 1000 * r, "poses": poses}
    return model


point = fit(statistics.median)
random.seed(7)
draws = [fit(random.choice) for _ in range(4000)]


def spread(path):
    vals = sorted(eval(path, {}, {"m": d}) for d in draws)
    return [round(vals[int(0.05 * len(vals))], 4), round(vals[int(0.95 * len(vals))], 4)]


report = {"stack_arm": STACK, "counts": {q: data[q]["counts"] for q in POSES}, "passes": {}}
for k in PASSES:
    entry = {"c_ms": round(point[k]["c_ms"], 4), "c_ms_p5_p95": spread(f"m['{k}']['c_ms']"),
             "r_us_per_row": round(point[k]["r_us_per_row"], 4),
             "r_us_per_row_p5_p95": spread(f"m['{k}']['r_us_per_row']"), "poses": {}}
    for q in POSES:
        entry["poses"][q] = {
            name: (round(v, 4) if v is not None else None)
            for name, v in point[k]["poses"][q].items()}
        for name in ("T_ms", "entry_share_ms", "stack_saving_ms", "post_filter_T_ms",
                     "post_filter_stack_saving_ms"):
            entry["poses"][q][f"{name}_p5_p95"] = spread(f"m['{k}']['poses']['{q}']['{name}']")
    report["passes"][k] = entry
print(json.dumps(report, indent=2))
