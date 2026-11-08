#!/usr/bin/env python3
"""Mac Q2 cost run (per-direction depth). Interleaves configs like the spike's q2.py.

q2mac.py <out dir> <config>[,<config>...] <view>[,<view>...] [repeat] [--res WxH] [--warm N] [--sample N]
Config: base, off, t4, t6, t8 (bin-pdd); opt-off, opt-all, opt-sparse (bin-opt, the
optimized lookup); paired (bin-opt, POSTRETRO_PDD_PAIRED: modes alternate in blocks of
--block frames inside one process; --order, default 0,1,2 = off, all, sparse; the first
--settle frames of each block are left out; --block 1 --settle 0 alternates every frame, and
the paired record carries a per-frame log). --timeout sets the xctrace watchdog in seconds
(default 400); --keep-xml 1 keeps the exported tables. Every run is a capture under Metal System Trace
(--launch); forward is the `Textured Pass` encoder interval, per frame.
Stops at the first invalid run.
"""
import json, os, re, shutil, statistics, subprocess, sys, threading, time, glob, plistlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mst_rows import rows as mst_rows

REPO = "/Users/dhiester/Projects/Personal/postretro"
S = "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8a65a298-34f6-4aca-a32c-6780069d4aa6/scratchpad/pdd"
MAP = REPO + "/content/dev/maps/stress-warren-hallway-inspection.prl"
POS = [32.1056, 22.86, 53.2384]
VIEWS = {"a": (-135, -12), "b": (45, -12), "c": (-45, -12)}
TMP = os.environ.get("TMPDIR", "/tmp")

args = [a for a in sys.argv[1:] if not a.startswith("--")]
opts = {}
it = iter(sys.argv[1:])
for a in it:
    if a.startswith("--"):
        opts[a[2:]] = next(it)
args = []
skip = False
for i, a in enumerate(sys.argv[1:]):
    if skip:
        skip = False
        continue
    if a.startswith("--"):
        skip = True
        continue
    args.append(a)
OUT = args[0]
CONFIGS = args[1].split(",")
VIEWLIST = args[2].split(",")
REPEAT = int(args[3]) if len(args) > 3 else 1
RES = [int(x) for x in opts.get("res", "3840x2160").split("x")]
WARM = int(opts.get("warm", "120"))
SAMPLE = int(opts.get("sample", "240"))
TEMP_GATE = int(opts.get("temp-gate", "76"))
BLOCK = int(opts.get("block", "50"))
SETTLE = int(opts.get("settle", "10"))
ORDER = opts.get("order", "0,1,2")
KEEP_XML = opts.get("keep-xml", "0") == "1"
TIMEOUT = int(opts.get("timeout", "400"))
MODE_NAMES = {0: "off", 1: "all", 2: "sparse"}
os.makedirs(OUT, exist_ok=True)


def gpu_stats():
    try:
        raw = subprocess.run(["ioreg", "-c", "IOAccelerator", "-r", "-d", "1", "-a"],
                             capture_output=True, timeout=10).stdout
        for a in plistlib.loads(raw):
            if "AMD" in a.get("IOClass", ""):
                ps = a.get("PerformanceStatistics", {})
                return dict(clock=ps.get("Core Clock(MHz)"), temp=ps.get("Temperature(C)"),
                            util=ps.get("Device Utilization %"),
                            inuse=ps.get("inUseVidMemoryBytes"), vfree=ps.get("vramFreeBytes"))
    except Exception as e:  # noqa
        return dict(error=str(e))
    return {}


class Poller(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.samples = []
        self.stop = False

    def run(self):
        while not self.stop:
            s = gpu_stats()
            s["t"] = time.time()
            self.samples.append(s)
            time.sleep(1.0)


def display_awake():
    out = subprocess.run(["pmset", "-g", "assertions"], capture_output=True).stdout.decode("utf-8", "replace")
    saver = subprocess.run(["pgrep", "-x", "ScreenSaverEngine"], capture_output=True).returncode == 0
    return saver, out


def wait_cool():
    t0 = time.time()
    while True:
        s = gpu_stats()
        if s.get("temp") is not None and s["temp"] <= TEMP_GATE:
            return s, time.time() - t0
        if time.time() - t0 > 600:
            return s, time.time() - t0
        time.sleep(10)


def union(intervals):
    total = 0
    cur_s = cur_e = None
    for s, e in sorted(intervals):
        if cur_e is None or s > cur_e:
            if cur_e is not None:
                total += cur_e - cur_s
            cur_s, cur_e = s, e
        else:
            cur_e = max(cur_e, e)
    if cur_e is not None:
        total += cur_e - cur_s
    return total


def perf_states(path):
    """gpu-performance-device-state-intervals: (start ns, end ns, state); the trace
    samples the device's performance state about every 10 ms (1 = lowest seen)."""
    s = open(path).read()
    ids, out = {}, []
    for row in re.findall(r"<row>(.*?)</row>", s, re.S):
        vals = []
        for m in re.finditer(r'<[\w-]+ (?:id="(\d+)" fmt="[^"]*">([^<]*)<|ref="(\d+)")', row):
            if m.group(1):
                ids[m.group(1)] = m.group(2)
                vals.append(m.group(2))
            else:
                vals.append(ids[m.group(3)])
        out.append((int(vals[0]), int(vals[0]) + int(vals[1]), int(vals[3])))
    return out


def state_mean(states, lo, hi):
    """Time-weighted mean performance state and the share of time in the top state
    seen, over [lo, hi)."""
    tot = acc = top = 0
    top_state = max((st for _, _, st in states), default=0)
    for a, b, st in states:
        o = min(b, hi) - max(a, lo)
        if o > 0:
            tot += o
            acc += o * st
            top += o if st == top_state else 0
    return (acc / tot, top / tot) if tot else (None, None)


def analyse(xml, states=None):
    rs = [r for r in mst_rows(xml) if r["proc"] and "postretro" in r["proc"]]
    rs.sort(key=lambda r: r["start"])
    lab = lambda r: r["label"] or ""
    paging = [r for r in rs if lab(r).startswith("Page On")]
    work = [r for r in rs if not lab(r).startswith("Page On")]
    cme = [r["start"] for r in work if lab(r).startswith("Capture Measurement Encoder")]
    # sample window: the last SAMPLE frame boundaries
    bounds = cme[-(SAMPLE + 1):]
    t_lo, t_hi = bounds[0], bounds[-1]
    res = {"frames_bounded": len(bounds) - 1}
    passes = {}
    for r in work:
        if t_lo <= r["start"] < t_hi:
            name = lab(r).split(":")[0]
            passes.setdefault(name, []).append(r)
    per_frame = {}
    for name, lst in passes.items():
        per_frame[name] = []
    busy = []
    fi = 0
    frame_rows = [[] for _ in range(len(bounds) - 1)]
    import bisect
    for r in work:
        if t_lo <= r["start"] < t_hi:
            i = bisect.bisect_right(bounds, r["start"]) - 1
            frame_rows[i].append(r)
    pass_frame = {}
    for i, fr in enumerate(frame_rows):
        # A frame whose forward row is missing from the trace would undercount.
        if not any(lab(r).startswith("Textured Pass") for r in fr):
            continue
        busy.append(union([(r["start"], r["start"] + r["dur"]) for r in fr]) / 1e6)
        names = {}
        for r in fr:
            names.setdefault(lab(r).split(":")[0], []).append((r["start"], r["start"] + r["dur"]))
        for n, iv in names.items():
            pass_frame.setdefault(n, []).append(union(iv) / 1e6)
    res["gpu_busy_median"] = statistics.median(busy)
    res["gpu_busy_mean"] = statistics.fmean(busy)
    res["passes"] = {n: dict(frames=len(v), median=statistics.median(v), mean=statistics.fmean(v),
                             max=max(v)) for n, v in pass_frame.items()}
    res["forward_coalesced_rows"] = sum(1 for r in passes.get("Textured Pass", []) if "Coelasced" in lab(r))
    res["paging_rows_in_window"] = sum(1 for r in paging if t_lo <= r["start"] < t_hi)
    res["paging_rows_total"] = len(paging)
    res["frame_period_median"] = statistics.median([(b - a) / 1e6 for a, b in zip(bounds, bounds[1:])])
    if states:
        res["perf_state_mean"], res["perf_state_top_share"] = state_mean(states, t_lo, t_hi)
    return res


def analyse_paired(xml, states=None):
    """Split frames by the mode in the measurement encoder label
    (`Capture Measurement Encoder m<mode> f<frame>`); each frame runs from its
    label row to the next one. Frames before the warmup, the first SETTLE frames of
    every block, and frames without a forward row are left out."""
    import bisect
    rs = [r for r in mst_rows(xml) if r["proc"] and "postretro" in r["proc"]]
    rs.sort(key=lambda r: r["start"])
    lab = lambda r: r["label"] or ""
    work = [r for r in rs if not lab(r).startswith("Page On")]
    marks = []
    for r in work:
        m = re.match(r"Capture Measurement Encoder m(\d+) f(\d+)", lab(r))
        if m:
            marks.append((r["start"], int(m.group(1)), int(m.group(2))))
    # One label can appear on several rows of a frame; keep the first per frame index.
    seen = {}
    for t, mode, f in marks:
        if f not in seen or t < seen[f][0]:
            seen[f] = (t, mode)
    frames = sorted((t, mode, f) for f, (t, mode) in seen.items())
    bounds = [t for t, _, _ in frames]
    frame_rows = [[] for _ in frames]
    for r in work:
        i = bisect.bisect_right(bounds, r["start"]) - 1
        if 0 <= i < len(frames) - 1:  # the last frame has no closing bound
            frame_rows[i].append(r)
    per = {}
    period = []
    frame_log = []
    for i, ((t, mode, f), fr) in enumerate(zip(frames, frame_rows)):
        if i == len(frames) - 1 or f < WARM or (f % BLOCK) < SETTLE:
            continue
        period.append((bounds[i + 1] - t) / 1e6)
        fw = [(r["start"], r["start"] + r["dur"]) for r in fr if lab(r).startswith("Textured Pass")]
        if not fw:
            continue
        d = per.setdefault(mode, dict(forward=[], busy=[], t=[], block=[], ps=[]))
        if states:
            d["ps"].append(state_mean(states, t, bounds[i + 1]))
        d["forward"].append(union(fw) / 1e6)
        d["busy"].append(union([(r["start"], r["start"] + r["dur"]) for r in fr]) / 1e6)
        d["t"].append(t)
        d["block"].append(f // BLOCK)
        frame_log.append([f, mode, round(d["forward"][-1], 4), round(d["busy"][-1], 4),
                          None if not d["ps"] or d["ps"][-1][0] is None else round(d["ps"][-1][0], 3)])
    res = {"frames_labelled": len(frames), "frame_indices": [frames[0][2], frames[-1][2]] if frames else None,
           "frame_period_median": statistics.median(period), "modes": {}, "frame_log": frame_log}
    for mode, d in per.items():
        blocks = {}
        for b, x in zip(d["block"], d["forward"]):
            blocks.setdefault(b, []).append(x)
        res["modes"][str(mode)] = dict(
            frames=len(d["forward"]), forward_median=statistics.median(d["forward"]),
            forward_mean=statistics.fmean(d["forward"]), busy_median=statistics.median(d["busy"]),
            busy_mean=statistics.fmean(d["busy"]), forward_max=max(d["forward"]),
            block_forward_medians={str(b): statistics.median(v) for b, v in sorted(blocks.items())},
            trace_t_range_ms=[min(d["t"]) / 1e6, max(d["t"]) / 1e6])
        ps = [x for x in d["ps"] if x[0] is not None]
        if ps:
            res["modes"][str(mode)]["perf_state_mean"] = statistics.fmean(x[0] for x in ps)
            res["modes"][str(mode)]["perf_state_top_share"] = statistics.fmean(x[1] for x in ps)
    return res


def done_valid(name):
    path = os.path.join(OUT, "runs.jsonl")
    if not os.path.exists(path):
        return False
    return any(json.loads(l)["name"] == name and not json.loads(l)["invalid"] for l in open(path))


def run(cfg, view, rep):
    yaw, pitch = VIEWS[view]
    name = f"{cfg}-{view}-{rep}"
    if done_valid(name):
        return
    report = os.path.join(OUT, name + ".report.json")
    scene = {"map": MAP, "camera": {"position": POS, "yaw_deg": yaw, "pitch_deg": pitch, "fov_deg": 100},
             "resolution": RES, "output": os.path.join(OUT, name + ".png"),
             "measurement": {"report": report, "warmup_frames": WARM, "sample_frames": SAMPLE}}
    path = os.path.join(OUT, name + ".json")
    json.dump(scene, open(path, "w"))
    exe = os.path.join(S, "bin-base" if cfg == "base" else "bin-pdd", "postretro")
    env = ["POSTRETRO_SH_STREAMING=sync-proof", "RUST_LOG=warn"]
    inner = cfg
    if cfg.startswith("opt-"):  # optimized-lookup build, one mode per process
        exe = os.path.join(S, "bin-opt", "postretro")
        env.append("POSTRETRO_PDD_MODE=" + cfg[4:])
        inner = ""
    if cfg == "paired":  # optimized-lookup build, modes alternate in blocks
        exe = os.path.join(S, "bin-opt", "postretro")
        env += ["POSTRETRO_PDD_PAIRED=" + ORDER, f"POSTRETRO_PDD_BLOCK={BLOCK}"]
        inner = ""
    if cfg.startswith("d-"):  # diagnostic build
        exe = os.path.join(S, "bin-diag", "postretro")
        inner = cfg[2:]
    if inner.startswith("t"):
        env.append("POSTRETRO_PDD_T=" + inner[1:])
    if inner.startswith("nf"):
        env += ["POSTRETRO_PDD_T=" + inner[2:], "POSTRETRO_PDD_NOFETCH=1"]
    trace = os.path.join(OUT, name + ".trace")
    shutil.rmtree(trace, ignore_errors=True)
    pre, waited = wait_cool()
    saver, _ = display_awake()
    poll = Poller()
    poll.start()
    cmd = ["xcrun", "xctrace", "record", "--template", "Metal System Trace", "--no-prompt",
           "--output", trace, "--target-stdout", os.path.join(OUT, name + ".stdout")]
    for e in env:
        cmd += ["--env", e]
    cmd += ["--launch", "--", exe, "--capture", path]
    t0 = time.time()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, errors="replace", cwd=REPO, timeout=TIMEOUT)
        rc, xout = r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        rc, xout = -999, "xctrace watchdog timeout"
        subprocess.run(["pkill", "-f", "xctrace record"])
        subprocess.run(["pkill", "-f", exe])
    wall = time.time() - t0
    poll.stop = True
    poll.join()
    open(os.path.join(OUT, name + ".xctrace.log"), "w").write(xout)
    rec = dict(name=name, cfg=cfg, view=view, rep=rep, rc=rc, wall_s=round(wall, 1),
               pre=pre, waited_s=round(waited), saver=saver, res=RES, warm=WARM, sample=SAMPLE,
               t0=t0)
    if cfg == "paired":
        rec.update(order=ORDER, block=BLOCK, settle=SETTLE)
    rec["poll"] = [(round(x["t"] - t0, 2), x.get("clock"), x.get("temp")) for x in poll.samples]
    temps = [s.get("temp") for s in poll.samples if s.get("temp") is not None]
    clocks = [s.get("clock") for s in poll.samples if s.get("clock") is not None]
    rec["temp_max"] = max(temps) if temps else None
    rec["clock_max"] = max(clocks) if clocks else None
    rec["clock_samples"] = clocks
    rec["vfree_min"] = min((s.get("vfree") for s in poll.samples if s.get("vfree") is not None), default=None)
    invalid = []
    if rc != 0:
        invalid.append(f"rc {rc}")
    stdout = open(os.path.join(OUT, name + ".stdout")).read() if os.path.exists(os.path.join(OUT, name + ".stdout")) else ""
    if cfg.startswith("t") or cfg in ("opt-all", "opt-sparse", "paired"):
        m = re.search(r"\[pdd\] synthetic tiles: T (\d+)", stdout + xout)
        rec["pdd_log"] = bool(m)
        if "TILE ATLAS TOO SMALL" in stdout + xout:
            invalid.append("tile atlas too small")
    if saver:
        invalid.append("screen saver running")
    if os.path.exists(report):
        rj = json.load(open(report))
        cc = rj["cpu_completion"]
        rec["completion_median"] = cc["median_ms"]
        rec["completion_p95"] = cc["p95_ms"]
        rec["completion_max"] = max(cc["samples_ms"])
        if cc["median_ms"] > 100:
            invalid.append("slow regime (completion median > 100 ms)")
    else:
        invalid.append("no report")
    if os.path.isdir(trace) and rc == 0:
        xml = os.path.join(OUT, name + ".gpu.xml")
        subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--xpath",
                        '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'],
                       stdout=open(xml, "w"), stderr=subprocess.DEVNULL, timeout=600)
        sxml = os.path.join(OUT, name + ".pstate.xml")
        subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--xpath",
                        '/trace-toc/run[@number="1"]/data/table[@schema="gpu-performance-device-state-intervals"]'],
                       stdout=open(sxml, "w"), stderr=subprocess.DEVNULL, timeout=600)
        try:
            states = perf_states(sxml)
        except Exception as e:  # noqa
            states = None
            rec["perf_state_error"] = str(e)
        if not KEEP_XML:
            os.remove(sxml)
        try:
            if cfg == "paired":
                a = analyse_paired(xml, states)
                rec.update(a)
                expect = (SAMPLE // BLOCK) // len(ORDER.split(",")) * (BLOCK - SETTLE)
                for m, pm in a["modes"].items():
                    if pm["frames"] < 0.75 * expect:
                        invalid.append(f"mode {m} forward frames {pm['frames']} of {expect}")
            else:
                a = analyse(xml, states)
                rec.update(a)
                fw = a["passes"].get("Textured Pass")
                if not fw or fw["frames"] < 0.75 * SAMPLE:
                    invalid.append(f"forward frames {fw and fw['frames']}")
            if a["frame_period_median"] > 100:
                invalid.append("slow regime (trace frame period > 100 ms)")
        except Exception as e:
            invalid.append(f"analyse failed: {e}")
        if not KEEP_XML:
            os.remove(xml)
    shutil.rmtree(trace, ignore_errors=True)
    for k in glob.glob(os.path.join(TMP, "instruments*.ktrace")):
        try:
            os.remove(k)
        except OSError:
            pass
    rec["invalid"] = invalid
    with open(os.path.join(OUT, "runs.jsonl"), "a") as f:
        f.write(json.dumps(rec) + "\n")
    if cfg == "paired":
        print(f"{name}: " + "; ".join(
            f"{MODE_NAMES[int(m)]} forward {pm['forward_median']:.3f} busy {pm['busy_median']:.3f} "
            f"(n {pm['frames']}, pstate {pm.get('perf_state_mean', float('nan')):.2f})"
            for m, pm in sorted(rec.get("modes", {}).items())) +
            f", temp pre {pre.get('temp')} max {rec['temp_max']}, clock max {rec['clock_max']}, "
            f"wall {rec['wall_s']} s, invalid {invalid}", flush=True)
        if invalid:
            print("STOP: invalid run", name, invalid, flush=True)
            sys.exit(1)
        return
    fw = rec.get("passes", {}).get("Textured Pass", {})
    print(f"{name}: forward {fw.get('median', float('nan')):.3f} ms (n {fw.get('frames')}), gpu busy "
          f"{rec.get('gpu_busy_median', float('nan')):.3f}, completion {rec.get('completion_median', float('nan')):.3f}, "
          f"temp pre {pre.get('temp')} max {rec['temp_max']}, clock max {rec['clock_max']}, "
          f"pstate {rec.get('perf_state_mean') or float('nan'):.2f}, "
          f"paging {rec.get('paging_rows_in_window')}, wall {rec['wall_s']} s, invalid {invalid}", flush=True)
    if invalid:
        print("STOP: invalid run", name, invalid, flush=True)
        sys.exit(1)


for rep in range(1, REPEAT + 1):
    for view in VIEWLIST:
        for cfg in CONFIGS:
            run(cfg, view, rep)
