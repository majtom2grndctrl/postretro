"""Summarize run1660.py records: per run the median of kept [gpu-timing] windows,
per arm the median and min..max over launches, and per-round deltas vs baseline.

usage: python summarize1660.py <runs_dir> [<out.json>]
"""
import glob, json, os, re, statistics as st, sys

PASSES = ["sh_compose", "animated_direct_sh_compose"]
# Pass A runs only on some frames; per frame = window avg x present / readbacks.
PASS_A = "direct_sh_compose"
COUNT_RE = re.compile(r"(indirect|animated-direct): frames \d+ rows min (\d+) max (\d+) \| "
                      r"last rows L0 (\d+) L1 (\d+) L2 (\d+) \| entries L0 (\d+) L1 (\d+) L2 (\d+)"
                      r" \| rows with entries (\d+) \| lane-entries (\d+)")


def run_summary(rec):
    out = {p: st.median(w["passes"][p]["ms"] for w in rec["windows"]) for p in PASSES}
    out["window_ranges"] = {p: [min(w["passes"][p]["ms"] for w in rec["windows"]),
                                max(w["passes"][p]["ms"] for w in rec["windows"])] for p in PASSES}
    t0 = rec["windows"][0]["t"] if rec["windows"] else 0
    mixes, unstable = set(), False
    for c in rec["counts"]:
        m = COUNT_RE.search(c["line"])
        if not m or c["t"] < t0 - 1.5:
            continue
        g = m.groups()
        # Pass A and Pass B rows vary by design where promotion weights move.
        if g[0] == "indirect" and g[1] != g[2]:
            unstable = True
        if g[0] == "indirect":
            mixes.add((g[0],) + g[3:])
    out["rows_stable"] = not unstable and len(mixes) <= 1
    out["mix"] = sorted(mixes)
    out["counts_last"] = rec["counts"][-3:] if rec["counts"] else []
    pa = [(w["passes"].get(PASS_A, {}).get("ms") or 0.0) *
          ((w["passes"].get(PASS_A, {}).get("present") or w["readbacks"]) / w["readbacks"]
           if w["passes"].get(PASS_A, {}).get("ms") is not None else 0.0)
          for w in rec["windows"]]
    out["pass_a_per_frame"] = st.median(pa) if pa else None
    clocks = [int(s[1]) for s in rec["nvsmi"] if len(s) > 2 and s[1].isdigit()]
    out["gr_clock_mhz"] = [min(clocks), max(clocks)] if clocks else None
    out["pstates"] = sorted({s[3] for s in rec["nvsmi"] if len(s) > 3})
    return out


def main():
    runs_dir = sys.argv[1]
    recs = [json.load(open(p)) for p in sorted(glob.glob(os.path.join(runs_dir, "*.run.json")))]
    valid, invalid = [], []
    for r in recs:
        s = run_summary(r) if r["valid"] else None
        if s and not s["rows_stable"]:
            r["valid"], r["reason"] = False, "rows changed"
        (valid if r["valid"] else invalid).append((r, s))
    by = {}
    for r, s in valid:
        rnd = int(r["label"].rsplit("-r", 1)[1])
        by.setdefault(r["pose"], {}).setdefault(r["arm"] or "baseline", {})[rnd] = s
    result = {"invalid": [(r["label"], r["reason"]) for r, _ in invalid], "poses": {}}
    for pose, arms in by.items():
        base = arms.get("baseline") or arms.get("old", {})
        pr = result["poses"][pose] = {}
        for arm, rounds in arms.items():
            e = pr[arm] = {"launches": len(rounds), "mix": next(iter(rounds.values()))["mix"],
                           "gr_clock_mhz": [min(v["gr_clock_mhz"][0] for v in rounds.values()),
                                            max(v["gr_clock_mhz"][1] for v in rounds.values())]}
            pa = [v["pass_a_per_frame"] for v in rounds.values()]
            e["pass_a_per_frame"] = {"median": st.median(pa), "range": [min(pa), max(pa)]}
            for p in PASSES:
                vals = [v[p] for v in rounds.values()]
                e[p] = {"median": st.median(vals), "range": [min(vals), max(vals)],
                        "per_launch": {k: v[p] for k, v in sorted(rounds.items())}}
                if arm not in ("baseline", "old"):
                    d = [rounds[k][p] - base[k][p] for k in rounds if k in base]
                    e[p]["delta_vs_baseline"] = {"median": round(st.median(d), 4),
                                                 "range": [round(min(d), 4), round(max(d), 4)],
                                                 "pct": round(100 * st.median(d) / st.median(
                                                     v[p] for v in base.values()), 1)}
    text = json.dumps(result, indent=1)
    if len(sys.argv) > 2:
        open(sys.argv[2], "w").write(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
