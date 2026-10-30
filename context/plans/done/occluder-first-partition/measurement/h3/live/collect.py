"""Summary tables for one live H3 batch directory (markdown on stdout).

usage: collect.py <batch dir>
Per run (plan.txt order): CpuTiming over the kept windows (cpuwin.py), GPU per frame from
the attached trace (gpulive.py), the AMD clock and temperature while the trace recorded
(the run's .amd poll between its trace_start / trace_end markers), idle state before the
run (pre: line) and the gate verdict. Then the live walk factor: live portal_walk us per
portal test over the in-process tool's us per test at the settled live eye (walk/).
"""
import glob, os, re, statistics, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cpuwin  # noqa: E402

O = sys.argv[1]


def kv(text, key):
    m = re.search(r"(?:^|\s)" + re.escape(key) + r"=([^\s|]+)", text)
    return m.group(1) if m else None


def num(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return float("nan")


def f(v, d=2):
    return "–" if v != v else f"{v:.{d}f}"


def safe(label):
    return label.translate(str.maketrans(":/@", "___"))


def trace_amd(path):
    if not os.path.exists(path):
        return float("nan"), float("nan")
    inside, mhz, temp = False, [], []
    for line in open(path):
        if line.rstrip().endswith("trace_start"):
            inside = True; continue
        if line.rstrip().endswith("trace_end"):
            break
        if inside:
            mhz.append(num(kv(line, "mhz"))); temp.append(num(kv(line, "temp")))
    mhz = [x for x in mhz if x == x]; temp = [x for x in temp if x == x]
    return (statistics.median(mhz) if mhz else float("nan"), max(temp) if temp else float("nan"))


def settle_of(meta):
    return int(kv(meta, "settle") or 0)


plan = [l.split() for l in open(os.path.join(O, "plan.txt"))] if os.path.exists(os.path.join(O, "plan.txt")) else []
runs = []
for rep, lab, m, pose, side in plan:
    tag = f"{m}.{safe(lab)}.{side}.r{rep}"
    mp = os.path.join(O, tag + ".meta")
    if not os.path.exists(mp):
        continue
    meta = open(mp).read()
    verdict = (re.findall(r"^verdict=(.*)$", meta, re.M) or ["(none)"])[-1]
    c = cpuwin.summarise(os.path.join(O, tag + ".log"), settle_of(meta)) if os.path.exists(os.path.join(O, tag + ".log")) else {}
    g = open(os.path.join(O, tag + ".gpu.txt")).readline() if os.path.exists(os.path.join(O, tag + ".gpu.txt")) else ""
    pre = (re.findall(r"^pre: (.*)$", meta, re.M) or [""])[0]
    gate = (re.findall(r"^gate: (.*)$", meta, re.M) or [""])[-1]
    mhz, tmax = trace_amd(os.path.join(O, tag + ".amd"))
    runs.append(dict(tag=tag, rep=rep, lab=lab, map=m, side=side, verdict=verdict, c=c, g=g, mhz=mhz, tmax=tmax,
                     pre_temp=num(kv(pre, "temp")), pre_vram=num(kv(pre, "inUseVidMemoryBytes")) / 2**30,
                     gate=gate.split()[0] if gate else "-", extents=(re.findall(r"^extents: (.*)$", meta, re.M) or ["?"])[0]))

print(f"# Live H3 batch `{os.path.basename(O.rstrip('/'))}`\n")
bm = os.path.join(O, "batch.meta")
if os.path.exists(bm):
    print("```"); print(open(bm).read().rstrip()); print("```\n")
st = os.path.join(O, "status")
if os.path.exists(st):
    lines = open(st).read().splitlines()
    flagged = [l for l in lines if re.search(r"SENTINEL|REGIME|GATE_TIMEOUT|STOPPED|SCREEN|NO_FRAMES|BATCH_DONE", l)]
    print("Sentinels and flags (from `status`):\n\n```"); print("\n".join(flagged)); print("```\n")
if runs:
    print(f"Render extents (first run): {runs[0]['extents']}\n")

print("## Per run\n")
print("ms unless noted; CPU = mean over kept windows; GPU = trace busy fraction × frame total; "
      "MHz / °C = AMD median clock and max temp while tracing; idle = before the run.\n")
print("| pose | side | verdict | gate | total | work | wait_acq | walk | tests | µs/test | trip frames | submit | record | GPU/frame | GPU gfx+cmp | MHz | °C trace | °C idle | VRAM idle GiB |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for r in runs:
    c = r["c"]
    g = lambda k: c.get(k, float("nan"))
    print(f"| {r['map']} `{r['lab']}` r{r['rep']} | {r['side']} | {r['verdict']} | {r['gate']} | {f(g('total'))} | {f(g('work'))} | {f(g('wait_acquire'))} | "
          f"{f(g('portal_walk'), 3)} | {f(g('walk_considered'), 0)} | {f(g('us_per_test'), 3)} | {c.get('walk_step_limit', '–')} | {f(g('render_submit'))} | "
          f"{f(g('render_record'))} | {f(num(kv(r['g'], 'gpu_ms_per_frame')))} | {f(num(kv(r['g'], 'gfxcompute_ms_per_frame')))} | "
          f"{f(r['mhz'], 0)} | {f(r['tmax'], 0)} | {f(r['pre_temp'], 0)} | {f(r['pre_vram'])} |")

# Live walk factor against the in-process tool.
walk = {}
for p in glob.glob(os.path.join(O, "walk", "*.r*.txt")):
    side, m = os.path.basename(p).split(".")[:2]
    for line in open(p):
        if line.startswith("#") or not line.strip():
            continue
        t = line.split()
        try:
            walk.setdefault((side, m, t[0]), []).append((int(t[1]), float(t[-2])))
        except (ValueError, IndexError):
            continue
if walk:
    print("\n## Live walk factor\n")
    print("in-process = walktime-capped (dev) median over 3 repeats at the settled live eye (floor+1.31); "
          "factor = live µs/test ÷ in-process µs/test. Rows under 1,000 live tests are noisy (fixed per-walk overhead).\n")
    print("| pose | side | live tests | live µs/test | in-proc tests | in-proc µs/test | factor | in-proc tests at floor+1.5 |")
    print("|---|---|---|---|---|---|---|---|")
    big = []
    for r in runs:
        c = r["c"]
        w = walk.get((r["side"], r["map"], r["lab"]))
        w15 = walk.get((r["side"], r["map"], r["lab"] + "~e150"))
        if not w or not c:
            continue
        tests = w[0][0]; us = statistics.median(x[1] for x in w)
        ip = us / tests if tests else float("nan")
        fac = c["us_per_test"] / ip if ip == ip and ip > 0 else float("nan")
        if c.get("walk_considered", 0) >= 1000 and fac == fac:
            big.append(fac)
        print(f"| {r['map']} `{r['lab']}` | {r['side']} | {f(c['walk_considered'], 0)} | {f(c['us_per_test'], 3)} | {tests} | {f(ip, 3)} | {f(fac)} | {w15[0][0] if w15 else '–'} |")
    if big:
        print(f"\nFactor over runs with ≥ 1,000 live tests: median {statistics.median(big):.2f}, range {min(big):.2f}–{max(big):.2f} (n={len(big)}). "
              "research.md / H3 estimate: ≈ 1.4 (1.3–1.7).")
