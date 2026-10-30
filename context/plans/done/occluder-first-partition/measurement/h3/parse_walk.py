"""Tables from the H3 walk-timing records (batch_walk.sh output).

usage: parse_walk.py <walk dir>
Prints, per map and pose: portal tests and median walk time (us) for before / cap19 /
cap26, capped (the engine's 5,000-step walk) and uncapped, as the median over the 3
repeats; then the per-1,000-test regression over the full street and rooftop sets.
"""
import glob, os, statistics as st, sys

D = sys.argv[1]
SIDES = ["before", "cap19", "cap26"]
MAPS = ["occlusion-test", "stress-street", "campaign-test", "stress-warren-showcase", "stress-warren-hallway-inspection"]

def read(path):
    rows = {}
    if not os.path.exists(path):
        return rows
    for line in open(path):
        if line.startswith("#") or not line.strip():
            continue
        f = line.split()
        # label considered accepted step_limit path... visibility_us walk_us calls
        rows[f[0]] = dict(considered=int(f[1]), accepted=int(f[2]), trip=int(f[3]),
                          path=" ".join(f[4:-3]), vis=float(f[-3]), walk=float(f[-2]), calls=int(f[-1]))
    return rows

def med_over_reps(side, m, v):
    reps = [read(f"{D}/{side}.{m}.{v}.debug.r{r}.txt") for r in (1, 2, 3)]
    reps = [r for r in reps if r]
    if not reps:
        return {}
    out = {}
    for k in reps[0]:
        vals = [r[k] for r in reps if k in r]
        out[k] = dict(vals[0], vis=st.median(x["vis"] for x in vals), walk=st.median(x["walk"] for x in vals),
                      spread=(max(x["vis"] for x in vals) - min(x["vis"] for x in vals)), n=len(vals))
    return out

def fmt_ms(us):
    return f"{us/1000:.3f}"

for m in MAPS:
    cap = {s: med_over_reps(s, m, "capped") for s in SIDES}
    unc = {s: med_over_reps(s, m, "uncapped") for s in SIDES}
    if not any(cap.values()):
        continue
    print(f"\n### {m}\n")
    print("| Pose | tests b / c19 / c26 | `visibility` ms, capped (engine), b / c19 / c26 | trips b / c19 / c26 | uncapped tests b / c19 / c26 | uncapped ms b / c19 / c26 |")
    print("|---|---|---|---|---|---|")
    labels = list(next(v for v in cap.values() if v).keys())
    for k in labels:
        def g(d, s, f, fm=str):
            r = d[s].get(k)
            return fm(r[f]) if r else "–"
        print(f"| `{k}` | {' / '.join(g(cap, s, 'considered') for s in SIDES)} | {' / '.join(g(cap, s, 'vis', fmt_ms) for s in SIDES)} | "
              f"{' / '.join(g(cap, s, 'trip') for s in SIDES)} | {' / '.join(g(unc, s, 'considered') for s in SIDES)} | {' / '.join(g(unc, s, 'vis', fmt_ms) for s in SIDES)} |")
    sp = [r["spread"] / r["vis"] for s in SIDES for r in cap[s].values() if r["vis"] > 20]
    if sp:
        print(f"\nRepeat spread (max-min over 3 repeats / median), poses over 20 us: median {st.median(sp)*100:.1f}%, max {max(sp)*100:.1f}%")

def fit(xs, ys):
    n = len(xs); mx = sum(xs) / n; my = sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs); sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    b = sxy / sxx; a = my - b * mx
    ss = sum((y - a - b * x) ** 2 for x, y in zip(xs, ys)); st_ = sum((y - my) ** 2 for y in ys)
    return a, b, 1 - ss / st_

print("\n### Walk cost per 1,000 portal tests (full street and rooftop sets, uncapped walk)\n")
print("| Side | Set | Profile | poses | fit: us per 1,000 tests | intercept us | R² | ratio us/test, p50 / p95 over poses ≥ 1,000 tests | max tests | `visibility` at max, ms |")
print("|---|---|---|---|---|---|---|---|---|---|")
for s in SIDES:
    for setn in ("street", "rooftop"):
        for prof in ("debug", "release"):
            r = read(f"{D}/{s}.full-{setn}.uncapped.{prof}.txt")
            if not r:
                continue
            xs = [v["considered"] for v in r.values()]; ys = [v["vis"] for v in r.values()]
            a, b, r2 = fit(xs, ys)
            ratios = sorted(v["vis"] / v["considered"] for v in r.values() if v["considered"] >= 1000)
            q = lambda p: ratios[min(len(ratios) - 1, int(p * len(ratios)))]
            worst = max(r.values(), key=lambda v: v["considered"])
            print(f"| {s} | {setn} | {'dev' if prof == 'debug' else 'release'} | {len(r)} | {b*1000:.1f} | {a:.1f} | {r2:.3f} | {q(0.5)*1000:.0f} / {q(0.95)*1000:.0f} | {worst['considered']} | {fmt_ms(worst['vis'])} |")

print("\n### Engine walk (5,000-step cap) on the full sets, dev\n")
print("| Side | Set | trips | `visibility` ms: p50 / p95 / max over poses | tripped poses: median ms |")
print("|---|---|---|---|---|")
for s in SIDES:
    for setn in ("street", "rooftop"):
        r = read(f"{D}/{s}.full-{setn}.capped.debug.txt")
        if not r:
            continue
        v = sorted(x["vis"] for x in r.values()); n = len(v)
        tr = [x["vis"] for x in r.values() if x["trip"]]
        print(f"| {s} | {setn} | {len(tr)} | {fmt_ms(v[n//2])} / {fmt_ms(v[int(0.95*n)])} / {fmt_ms(v[-1])} | {fmt_ms(st.median(tr)) if tr else '–'} |")
