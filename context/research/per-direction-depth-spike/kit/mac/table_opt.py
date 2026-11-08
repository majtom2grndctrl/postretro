#!/usr/bin/env python3
"""Aggregate the optimized-lookup runs (q2mac.py configs base, t6, opt-*, paired).

table_opt.py <runs.jsonl> [...]
Separate runs: view x config, mean of per-run medians (spread, n), deltas.
Paired runs: per run, per mode forward / whole-frame medians and perf state; deltas
over `off` inside the same process, then the mean over runs per view and resolution.
"""
import json, statistics, sys

runs = [json.loads(l) for p in sys.argv[1:] for l in open(p)]
valid = [r for r in runs if not r["invalid"]]
print(f"runs {len(runs)}, valid {len(valid)}, invalid "
      f"{[(r['name'], tuple(r['res']), r['invalid']) for r in runs if r['invalid']]}")
CFGS = ["base", "t6", "opt-off", "opt-all", "opt-sparse"]
MODES = {"0": "off", "1": "all", "2": "sparse"}


def res_key(r):
    return f"{r['res'][0]}x{r['res'][1]}"


sep = [r for r in valid if r["cfg"] != "paired"]
for res in sorted({res_key(r) for r in sep}, reverse=True):
    rs = [r for r in sep if res_key(r) == res]
    views = sorted({r["view"] for r in rs})
    for title, metric in [("forward (Textured Pass) ms", lambda r: r["passes"]["Textured Pass"]["median"]),
                          ("whole-frame GPU ms", lambda r: r["gpu_busy_median"])]:
        print(f"\n{res} separate runs, {title}: mean of runs (spread, n)")
        print("| View | " + " | ".join(CFGS) + " |")
        tab = {}
        for v in views:
            row = []
            for c in CFGS:
                xs = [metric(r) for r in rs if r["view"] == v and r["cfg"] == c]
                if xs:
                    tab[(v, c)] = statistics.fmean(xs)
                    row.append(f"{tab[(v, c)]:.2f} ({max(xs) - min(xs):.2f}, {len(xs)})")
                else:
                    row.append("-")
            print(f"| {v} | " + " | ".join(row) + " |")
        for v in views:
            parts = []
            for c in CFGS[1:]:
                for ref in ("base", "opt-off"):
                    if (v, c) in tab and (v, ref) in tab and c != ref and not (c == "t6" and ref == "opt-off"):
                        parts.append(f"{c}-{ref} {tab[(v, c)] - tab[(v, ref)]:+.2f}")
            print(f"  {v}: " + "; ".join(parts))
    print(f"\n{res} machine state: cfg, view, rep, temp pre/max, clock max, pstate mean, top share, paging")
    for r in rs:
        print(f"  {r['cfg']} {r['view']} {r['rep']}: {r['pre'].get('temp')}/{r['temp_max']} C, "
              f"{r['clock_max']} MHz, pstate {r.get('perf_state_mean') or float('nan'):.2f}, "
              f"top {r.get('perf_state_top_share') or float('nan'):.2f}, paging {r.get('paging_rows_in_window')}, "
              f"completion {r.get('completion_median', float('nan')):.1f}")

paired = [r for r in valid if r["cfg"] == "paired"]
for res in sorted({res_key(r) for r in paired}, reverse=True):
    rs = [r for r in paired if res_key(r) == res]
    print(f"\n{res} paired runs: forward median ms / whole-frame median ms / perf state mean (frames)")
    print("| Run | off | all | sparse | all - off | sparse - off | all - off, whole | sparse - off, whole |")
    agg = {}
    for r in rs:
        m = r["modes"]
        cell = lambda k: (f"{m[k]['forward_median']:.2f} / {m[k]['busy_median']:.2f} / "
                          f"{m[k].get('perf_state_mean', float('nan')):.2f} ({m[k]['frames']})")
        d = {k: m[k]["forward_median"] - m["0"]["forward_median"] for k in ("1", "2")}
        dw = {k: m[k]["busy_median"] - m["0"]["busy_median"] for k in ("1", "2")}
        print(f"| {r['name']} | {cell('0')} | {cell('1')} | {cell('2')} | {d['1']:+.2f} | {d['2']:+.2f} | "
              f"{dw['1']:+.2f} | {dw['2']:+.2f} |")
        # State-matched check: frames binned by their mean performance state; the
        # per-bin median deltas over off, weighted by off's frame count per bin.
        fl = r.get("frame_log") or []
        bins = [(0, 1.5), (1.5, 2.5), (2.5, 9)]
        sm = {}
        for k in (1, 2):
            num = den = 0
            for lo, hi in bins:
                xs = {m: [x[2] for x in fl if x[1] == m and x[4] is not None and lo <= x[4] < hi] for m in (0, k)}
                if len(xs[0]) >= 10 and len(xs[k]) >= 10:
                    num += len(xs[0]) * (statistics.median(xs[k]) - statistics.median(xs[0]))
                    den += len(xs[0])
            sm[k] = num / den if den else float("nan")
        # Triplet-paired: with per-frame alternation (block 1, order 0,1,2), frames
        # 3k, 3k+1, 3k+2 are off, all, sparse back to back; the median over complete
        # triplets of (all - off) and (sparse - off), forward and whole-frame.
        tp = {}
        if fl and r.get("block") == 1 and r.get("order") == "0,1,2":
            byf = {x[0]: x for x in fl}
            for col, key in ((2, "fw"), (3, "busy")):
                for k in (1, 2):
                    ds = [byf[f + k][col] - byf[f][col] for f in byf
                          if byf[f][1] == 0 and f + 1 in byf and f + 2 in byf
                          and byf[f + 1][1] == 1 and byf[f + 2][1] == 2]
                    tp[(key, k)] = (statistics.median(ds), len(ds))
        if fl:
            print(f"|   state-matched forward delta | | | | {sm[1]:+.2f} | {sm[2]:+.2f} | | |")
        if tp:
            print(f"|   triplet-paired delta (n {tp[('fw', 1)][1]}) | | | | {tp[('fw', 1)][0]:+.2f} | {tp[('fw', 2)][0]:+.2f} | "
                  f"{tp[('busy', 1)][0]:+.2f} | {tp[('busy', 2)][0]:+.2f} |")
        a = agg.setdefault(r["view"], {})
        for kk, vv in tp.items():
            a.setdefault(("tp",) + kk, []).append(vv[0])
        a.setdefault(("sm", "1"), []).append(sm.get(1, float("nan")))
        a.setdefault(("sm", "2"), []).append(sm.get(2, float("nan")))
        for k in ("0", "1", "2"):
            a.setdefault(("fw", k), []).append(m[k]["forward_median"])
            a.setdefault(("busy", k), []).append(m[k]["busy_median"])
        # Block-to-block spread of the per-block forward medians, per mode.
        for k in ("0", "1", "2"):
            b = list(m[k]["block_forward_medians"].values())
            a.setdefault(("blockspread", k), []).append(max(b) - min(b))
    print(f"\n{res} paired, mean over runs per view (forward | whole-frame)")
    for v, a in sorted(agg.items()):
        fw = {k: statistics.fmean(a[("fw", k)]) for k in ("0", "1", "2")}
        bz = {k: statistics.fmean(a[("busy", k)]) for k in ("0", "1", "2")}
        print(f"  {v} (n {len(a[('fw', '0')])}): off {fw['0']:.2f}, all {fw['1']:.2f} ({fw['1'] - fw['0']:+.2f}), "
              f"sparse {fw['2']:.2f} ({fw['2'] - fw['0']:+.2f}) | off {bz['0']:.2f}, all {bz['1']:.2f} "
              f"({bz['1'] - bz['0']:+.2f}), sparse {bz['2']:.2f} ({bz['2'] - bz['0']:+.2f})")
        d1 = [x - y for x, y in zip(a[("fw", "1")], a[("fw", "0")])]
        d2 = [x - y for x, y in zip(a[("fw", "2")], a[("fw", "0")])]
        print(f"     per-run forward deltas all {[round(x, 2) for x in d1]}, sparse {[round(x, 2) for x in d2]}; "
              f"state-matched mean all {statistics.fmean(a[('sm', '1')]):+.2f}, sparse {statistics.fmean(a[('sm', '2')]):+.2f}; "
              + (f"triplet-paired mean all {statistics.fmean(a[('tp', 'fw', 1)]):+.2f} {[round(x, 2) for x in a[('tp', 'fw', 1)]]}, "
                 f"sparse {statistics.fmean(a[('tp', 'fw', 2)]):+.2f} {[round(x, 2) for x in a[('tp', 'fw', 2)]]}; whole-frame all "
                 f"{statistics.fmean(a[('tp', 'busy', 1)]):+.2f}, sparse {statistics.fmean(a[('tp', 'busy', 2)]):+.2f}; "
                 if ("tp", "fw", 1) in a else "") +
              f"pstate means off/all/sparse {[round(statistics.fmean(r['modes'][k].get('perf_state_mean', 0) for r in rs if r['view'] == v), 2) for k in ('0', '1', '2')]}")
    for r in rs:
        print(f"  {r['name']}: temp {r['pre'].get('temp')}/{r['temp_max']} C, clock max {r['clock_max']} MHz, "
              f"ioreg clocks {min(c for c in r['clock_samples'] if c)}-{max(r['clock_samples'])}, "
              f"frame period {r['frame_period_median']:.1f} ms")
