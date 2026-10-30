"""Tables from the H3 4K capture runs (batch_cap.py output).

usage: parse_cap.py <scratch h3 dir>
Per map and pose, before / cap19 / cap26: GPU frame time (the capture's per-frame
submit-to-idle wait, GPU saturated at 4K) median and p95, each the median over the
accepted repeats, with the repeat spread; CPU render_record and rec_cull; lightmap pool
layers and active bytes; AMD VRAM in use (peak during the run) and mean core clock.
Also lists every discarded attempt and the regime incidence.
"""
import glob, json, os, re, statistics as st, sys

S = sys.argv[1]; C = f"{S}/capruns"
SIDES = ["before", "cap19", "cap26"]
MAPS = ["occlusion-test", "stress-street", "campaign-test", "stress-warren-showcase", "stress-warren-hallway-inspection"]
status = [l.split() for l in open(f"{S}/batch_cap.status") if l.split()[1].count(".") >= 3]
ok = {l[1] for l in status if l[2] == "OK"}
bad = [l for l in status if l[2] != "OK"]

def stage(d, label):
    vals = [s["average"] for w in d["cpu_stages"].get("windows", []) for s in w["stages"] if s["label"] == label]
    return st.mean(vals) if vals else float("nan")

def amd(tag):
    peak, clocks, utils = 0, [], []
    for line in open(f"{C}/{tag}.amd"):
        kv = dict(x.split("=") for x in line.split() if "=" in x)
        if "inUseVidMemoryBytes" in kv:
            peak = max(peak, int(kv["inUseVidMemoryBytes"]))
        if kv.get("util") and int(kv["util"]) >= 90 and kv.get("mhz"):
            clocks.append(int(kv["mhz"]))
    return peak, (st.mean(clocks) if clocks else float("nan"))

runs = {}
for tag in ok:
    m, rest = tag.split(".", 1)
    label, side, rep = rest.rsplit(".", 2)[0], rest.rsplit(".", 2)[1], rest.rsplit(".", 2)[2]
    if side not in SIDES:  # an attempt suffix: label.side.rN.aK
        label, side, rep = rest.rsplit(".", 3)[0], rest.rsplit(".", 3)[1], rest.rsplit(".", 3)[2]
    d = json.load(open(f"{C}/{tag}.report.json"))
    c = d["cpu_completion"]; ls = d.get("lightmap_streaming") or {}; cn = ls.get("counters", {})
    peak, mhz = amd(tag)
    runs.setdefault((m, label, side), []).append(dict(
        med=c["median_ms"], p95=c["p95_ms"], rec=stage(d, "render_record"), cull=stage(d, "rec_cull"),
        layers=ls.get("pool_layers"), pool=cn.get("pool_bytes", 0), growths=cn.get("growths", 0),
        vram=peak, mhz=mhz))

def agg(rs, k):
    return st.median(r[k] for r in rs)

for m in MAPS:
    keys = sorted({(l) for (mm, l, s) in runs if mm == m}, key=lambda l: l)
    if not keys:
        continue
    order = [l.split()[1] for l in open(f"{os.path.dirname(os.path.abspath(__file__))}/poses/{m}.poses") if l.startswith("pose ")]
    keys = [k for k in [o.replace(":", "_").replace("/", "_").replace("@", "_") for o in order] if k in keys]
    print(f"\n### {m}\n")
    print("| Pose | GPU ms median, b / c19 / c26 | GPU ms p95, b / c19 / c26 | c26 vs b | c19 vs b | render_record ms, b / c19 / c26 | pool layers (MiB), b / c19 / c26 | VRAM peak GiB, b / c19 / c26 | reps |")
    print("|---|---|---|---|---|---|---|---|---|")
    for k in keys:
        rs = {s: runs.get((m, k, s), []) for s in SIDES}
        def col(f, fm):
            return " / ".join(fm(agg(rs[s], f)) if rs[s] else "–" for s in SIDES)
        def rel(a, b):
            return f"{(agg(rs[a], 'med') / agg(rs[b], 'med') - 1) * 100:+.1f}%" if rs[a] and rs[b] else "–"
        pools = " / ".join(f"{rs[s][0]['layers']} ({rs[s][0]['pool']/2**20:.0f})" if rs[s] else "–" for s in SIDES)
        print(f"| `{k}` | {col('med', lambda x: f'{x:.2f}')} | {col('p95', lambda x: f'{x:.2f}')} | {rel('cap26', 'before')} | {rel('cap19', 'before')} | "
              f"{col('rec', lambda x: f'{x:.2f}')} | {pools} | {col('vram', lambda x: f'{x/2**30:.2f}')} | {'/'.join(str(len(rs[s])) for s in SIDES)} |")
    sp = [(max(r['med'] for r in v) - min(r['med'] for r in v)) / agg(v, 'med') for (mm, l, s), v in runs.items() if mm == m and len(v) > 1]
    if sp:
        print(f"\nRepeat spread of the GPU median (max-min / median): median {st.median(sp)*100:.1f}%, max {max(sp)*100:.1f}%.")

allmhz = [r["mhz"] for v in runs.values() for r in v if r["mhz"] == r["mhz"]]
print(f"\nAccepted runs: {sum(len(v) for v in runs.values())}; mean busy core clock {st.mean(allmhz):.0f} MHz (min run {min(allmhz):.0f}, max {max(allmhz):.0f}).")
print(f"Discarded attempts: {len(bad)}")
for l in bad:
    print("  " + " ".join(l))
