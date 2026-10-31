"""Metal System Trace A/B for the mover depth pre-pass change (Mac, two binaries).

usage: python3 measurements/mover-depth-prepass/run.py [options]
  --arms a,b,...    arms to run, <pose>:<binary> (default: every pose x before,after)
  --rounds N        interleaved rounds (default 2: arm order forward, then reversed)
  --warmup N / --samples N   capture frames before / during the timed span (default 120 / 240)
  --tag T           run-label prefix and results file name (results-<T>.{json,md}; default batch)
  --abort-on-invalid  stop at the first invalid run (pilot mode)
  --write-scenes    write scenes/<pose>.json (the capture scenes, minus per-run paths) and exit
env:
  BIN_BEFORE / BIN_AFTER  engine binaries, each named `postretro` in its own directory
  MAP_PRL           mover-setpiece.prl compiled with prl-build (required)
  IDLE_GAP          seconds of idle before every launch (default 15)
  RECORD_TIMEOUT    seconds before a run's watchdog kills it (default 1200)
  MIN_FREE_GB       abort when the trace dir has less free space (default 25)
  KEEP_RAW=1        keep gzipped trace exports in raw/ (default off; they are ~5-10 MB each)

Adapted from measurements/fluid-base-pass-cost/metal/{run.py,run_ablation.py}:
caffeinate held with user activity re-declared, 15 s idle gap before every launch,
idle ioreg snapshot, ioreg GPU clock/temperature polling during each run,
screen-lock / screen-saver / foreign-process checks, `xctrace record --launch` of a
headless `--capture` run with a watchdog, `xctrace export` of metal-gpu-intervals,
per-encoder interval-union reduction (gpu_time.py), trace bundle and
$TMPDIR/instruments*.ktrace deleted once reduced. What differs: the arms (pose x
binary), the map, and the figures (Depth Pre-Pass, Textured Pass, Kinematic Brush
Pass, SDF Shadow Pass, their sum, whole-frame GPU time). Every run is one engine
process: `<bin> --capture <scene>` at 3840x2160 with POSTRETRO_SH_STREAMING=sync-proof
and RUST_LOG=info.
"""
import argparse, atexit, collections, gzip, json, os, re, shutil, signal, subprocess, sys
import tempfile, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from gpu_time import reduce_export, DEPTH, FORWARD, MOVER, SDF  # noqa: E402

ROOT = HERE.parents[1]
RUNS = HERE / "runs"
RAW = HERE / "raw"
SCENES = HERE / "scenes"
UHD = [3840, 2160]
FOV = 100.0
INCH = 0.0254


def engine_pose(x, y, z):
    """TrenchBroom map units (z up) -> engine metres (y up): (x, y, z) -> (-y, z, -x)."""
    return [round(-y * INCH, 4), round(z * INCH, 4), round(-x * INCH, 4)]


# Stations from content/dev/maps/mover-setpiece.README.md. Map yaw keeps its sign in
# the engine (entity_model.md section 4): yaw 0 faces map +x, 90 faces map +y (north).
POSES = {
    # Station 1 facing north at the closed sliding wall: the primary arm.
    "wall": {"map_units": [368, 720, 48], "yaw": 90.0, "pitch": 0.0},
    # Station 3 facing south at the wall's north face.
    "north": {"map_units": [368, 1536, 48], "yaw": 270.0, "pitch": 0.0},
    # Off-screen: station 1 facing south-west. No mover is in the frustum, but the
    # movers' bounds touch visible arena cells, so both builds still record them
    # (GPU-clipped): the cost of an admitted, unseen mover.
    "offscreen": {"map_units": [368, 720, 48], "yaw": 225.0, "pitch": 0.0},
    # Null: the back room's east end facing north. Zero mover draws (no Kinematic
    # Brush Pass in either build), so the two binaries record the same passes.
    "null": {"map_units": [1200, 1500, 48], "yaw": 90.0, "pitch": 0.0},
}
for _p in POSES.values():
    _p["position"] = engine_pose(*_p["map_units"])
BINARIES = {"before": "BIN_BEFORE", "after": "BIN_AFTER"}
ENGINE_ENV = {"POSTRETRO_SH_STREAMING": "sync-proof", "RUST_LOG": "info"}
BUSY_PROCESSES = ("postretro", "cargo", "rustc", "prl-build")
XPATH = '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'
GPU_FIELDS = ("Core Clock(MHz)", "Memory Clock(MHz)", "Temperature(C)", "Total Power(W)",
              "GPU Activity(%)", "Fan Speed(RPM)")
BUSY_ACTIVITY = 50
# A run whose busy core clock median falls below this is not saturated (the 5300M
# tops out near 1250 MHz under a 4K capture; idle-ish regimes sit near 300-500).
MIN_BUSY_CLOCK_MHZ = 1000


# --- machine state (copied from the fluid kit's run.py) ----------------------

def screen_locked():
    r = subprocess.run(["ioreg", "-n", "Root", "-d1", "-a"], capture_output=True, text=True)
    i = r.stdout.find("CGSSessionScreenIsLocked")
    return i >= 0 and "<true/>" in r.stdout[i:i + 80]


def screensaver_running():
    return subprocess.run(["pgrep", "-f", "ScreenSaverEngine|legacyScreenSaver"],
                          capture_output=True).returncode == 0


def busy_processes():
    found = {}
    for name in BUSY_PROCESSES:
        r = subprocess.run(["pgrep", "-x", name], capture_output=True, text=True)
        if r.returncode == 0:
            found[name] = r.stdout.split()
    return found


def gpu_state():
    r = subprocess.run(["ioreg", "-c", "AMDRadeonX6000_AMDNavi14GraphicsAccelerator", "-r", "-d1", "-w0"],
                       capture_output=True, text=True)
    if "Core Clock(MHz)" not in r.stdout:
        r = subprocess.run(["ioreg", "-c", "IOAccelerator", "-r", "-d1", "-w0"], capture_output=True, text=True)
    sample = {}
    for field in GPU_FIELDS:
        m = re.search(r'"' + re.escape(field) + r'"=(\d+)', r.stdout)
        if m:
            sample[field] = int(m[1])
    return sample


def median(values):
    values = sorted(values)
    return values[len(values) // 2] if values else None


def clock_summary(samples):
    busy = [s for s in samples if s.get("GPU Activity(%)", 0) >= BUSY_ACTIVITY]
    basis, pool = ("busy", busy) if len(busy) >= 5 else ("all", samples)
    out = {"basis": basis, "samples": len(pool), "samples_total": len(samples)}
    for field, key in (("Core Clock(MHz)", "clock_mhz"), ("Temperature(C)", "temp_c"),
                       ("Total Power(W)", "power_w"), ("GPU Activity(%)", "activity_pct")):
        vals = [s[field] for s in pool if field in s]
        out[key] = {"min": min(vals), "median": median(vals), "max": max(vals)} if vals else None
    return out


def idle_snapshot(path):
    r = subprocess.run(["ioreg", "-c", "IOAccelerator", "-r", "-d1"], capture_output=True, text=True)
    lines = [l for l in r.stdout.splitlines()
             if re.search(r"inUseVidMemoryBytes|vramFreeBytes|Device Utilization %", l)]
    path.write_text("\n".join(lines) + "\n")
    parsed = {}
    for key in ("inUseVidMemoryBytes", "vramFreeBytes", "Device Utilization %"):
        parsed[key] = [int(v) for v in re.findall(r'"' + re.escape(key) + r'"=(\d+)', r.stdout)]
    return parsed


def free_gb(path):
    return shutil.disk_usage(path).free / 1e9


def engine_env():
    env = os.environ.copy()
    env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
    env.update(ENGINE_ENV)
    return env


def xctrace_flags():
    r = subprocess.run(["xcrun", "xctrace", "record", "-h"], capture_output=True, text=True)
    text = r.stdout + r.stderr
    return {"env": "--env" in text, "target_stdout": "--target-stdout" in text, "no_prompt": "--no-prompt" in text}


def kill_engine():
    subprocess.run(["pkill", "-x", "postretro"], capture_output=True)


def delete_ktrace(since):
    tmp = Path(os.environ.get("TMPDIR", "/tmp"))
    for f in tmp.glob("instruments*.ktrace"):
        try:
            if f.stat().st_mtime >= since:
                shutil.rmtree(f) if f.is_dir() else f.unlink()
        except OSError as e:
            print(f"could not delete {f}: {e}", file=sys.stderr)


# --- scenes -------------------------------------------------------------------

def scene_for(pose_name, mapname, png, report, warmup, samples, resolution=UHD):
    pose = POSES[pose_name]
    scene = {"map": str(mapname),
             "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                        "pitch_deg": pose["pitch"], "fov_deg": FOV},
             "resolution": resolution, "output": str(png)}
    if warmup is not None:
        scene["measurement"] = {"report": str(report), "warmup_frames": warmup, "sample_frames": samples}
    return scene


def map_path():
    m = os.environ.get("MAP_PRL")
    if not m or not Path(m).is_file():
        raise SystemExit("MAP_PRL must name the compiled mover-setpiece.prl")
    return Path(m)


# --- one run ------------------------------------------------------------------

def run_one(label, arm, args, trace_dir, flags, round_i):
    png = trace_dir / f"{label}.png"
    report = RUNS / f"{label}.report.json"
    scene_path = RUNS / f"{label}.scene.json"
    log = RUNS / f"{label}.log"
    status = RUNS / f"{label}.trace-status.txt"
    trace = trace_dir / f"{label}.trace"
    xml = trace_dir / f"{label}-gpu.xml"
    report.unlink(missing_ok=True)
    scene = scene_for(arm["pose"], map_path(), png, report, args.warmup, args.samples)
    scene_path.write_text(json.dumps(scene, indent=2) + "\n")

    cmd = ["xcrun", "xctrace", "record", "--template", "Metal System Trace", "--output", str(trace)]
    if flags["no_prompt"]:
        cmd += ["--no-prompt"]
    if flags["target_stdout"]:
        cmd += ["--target-stdout", str(log)]
    if flags["env"]:
        for k, v in ENGINE_ENV.items():
            cmd += ["--env", f"{k}={v}"]
    cmd += ["--launch", "--", arm["binary"], "--capture", str(scene_path)]

    record = {"label": label, "arm": arm["name"], "round": round_i, "pose": arm["pose"], "binary_arm": arm["bin"],
              "binary": arm["binary"], "camera": POSES[arm["pose"]], "warmup_frames": args.warmup,
              "sample_frames": args.samples, "resolution": UHD,
              "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "problems": []}
    gpu_samples, sampling, checks = [], threading.Event(), []
    started = time.time()

    def poll_state():
        n = 0
        while sampling.is_set():
            gpu_samples.append({"t": round(time.time() - started, 2), **gpu_state()})
            if n % 4 == 0:
                checks.append({"t": round(time.time() - started, 2), "locked": screen_locked(),
                               "screensaver": screensaver_running(), "busy": busy_processes()})
            n += 1
            time.sleep(0.5)

    x = poller = None
    try:
        sampling.set()
        poller = threading.Thread(target=poll_state, daemon=True)
        poller.start()
        with status.open("w") as sf:
            x = subprocess.Popen(cmd, cwd=ROOT, env=engine_env(), stdout=sf, stderr=subprocess.STDOUT)
            report_seen = interrupted = None
            timeout = float(os.environ.get("RECORD_TIMEOUT", "1200"))
            while x.poll() is None:
                now = time.time()
                if report_seen is None and report.exists():
                    report_seen = now
                    sampling.clear()
                if report_seen and interrupted is None and now - report_seen > 180:
                    record["problems"].append("xctrace still running 180 s after the engine finished; interrupted")
                    x.send_signal(signal.SIGINT)
                    interrupted = now
                if interrupted and now - interrupted > 180:
                    record["problems"].append("xctrace ignored SIGINT for 180 s; terminated")
                    x.terminate()
                    try:
                        x.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        x.kill()
                    break
                if now - started > timeout:
                    record["problems"].append(f"watchdog: run exceeded {timeout:.0f} s")
                    kill_engine()
                    x.send_signal(signal.SIGINT)
                    try:
                        x.wait(timeout=60)
                    except subprocess.TimeoutExpired:
                        x.terminate()
                    break
                time.sleep(1)
        record["xctrace_exit"] = x.returncode
    finally:
        sampling.clear()
        if poller is not None:
            poller.join(timeout=5)
        if x is not None and x.poll() is None:
            kill_engine()
            x.terminate()
            try:
                x.wait(timeout=15)
            except subprocess.TimeoutExpired:
                x.kill()
    record["elapsed_s"] = round(time.time() - started, 1)
    record["clock"] = clock_summary(gpu_samples)
    record["gpu_state_samples"] = gpu_samples
    record["locked_seen"] = any(c["locked"] for c in checks)
    record["screensaver_seen"] = any(c["screensaver"] for c in checks)
    record["foreign_processes_seen"] = sorted({n for c in checks for n in c["busy"] if n != "postretro"})

    try:
        rep = json.loads(report.read_text())
        record["adapter"] = rep.get("adapter")
        record["capture"] = rep.get("capture")
        record["engine_revision"] = rep.get("revision")
        record["report_keys"] = sorted(rep.keys())
    except (OSError, ValueError) as e:
        record["problems"].append(f"no capture report: {e}")
    adapter = json.dumps(record.get("adapter") or {}).lower()
    if record.get("adapter") and not ("5300m" in adapter or ("discrete" in adapter and "metal" in adapter)):
        record["problems"].append(f"adapter is not the discrete Metal GPU: {record['adapter']}")
    if log.exists():
        # Committed extents (rendering_pipeline.md section 12: record surface and scene extents).
        text = log.read_text(errors="replace")
        record["extent_lines"] = [l.strip()[-240:] for l in text.splitlines()
                                  if re.search(r"extent|resolution", l, re.I)][:8]

    try:
        if trace.exists():
            r = subprocess.run(["xcrun", "xctrace", "export", "--input", str(trace), "--xpath", XPATH,
                                "--output", str(xml)], capture_output=True, text=True)
            if r.returncode != 0 or not xml.exists():
                record["problems"].append(f"xctrace export failed: {r.stderr.strip()[:300]}")
            else:
                record["gpu"] = reduce_export(str(xml), args.warmup, args.samples)
                if os.environ.get("KEEP_RAW") == "1":
                    RAW.mkdir(exist_ok=True)
                    with xml.open("rb") as src, gzip.open(RAW / f"{label}-gpu.xml.gz", "wb") as dst:
                        shutil.copyfileobj(src, dst)
        else:
            record["problems"].append("no trace bundle was written")
    except Exception as e:  # a parse failure must not strand a trace
        record["problems"].append(f"reduction failed: {e!r}")
    finally:
        shutil.rmtree(trace, ignore_errors=True)
        xml.unlink(missing_ok=True)
        png.unlink(missing_ok=True)
        delete_ktrace(started - 1)

    gpu = record.get("gpu")
    if gpu:
        if not gpu.get("available"):
            record["problems"].append("no complete-frame basis: " + str(gpu.get("reason")))
        if gpu.get("frame_count_ok") is False:
            record["problems"].append(
                f"frame marker count {gpu.get('marker_count')} ({gpu.get('marker')}) does not fit expected "
                f"{gpu.get('expected_frames')}")
        if gpu.get("steady_frames") != args.samples:
            record["problems"].append(f"steady frames {gpu.get('steady_frames')} != samples {args.samples}")
        modes = gpu.get("modal_counts") or {}
        for label_ in (DEPTH, FORWARD):
            if modes.get(label_) != 1:
                record["problems"].append(f"{label_} modal count {modes.get(label_)} != 1")
        want_mover = 0 if arm["pose"] == "null" else 1
        if modes.get(MOVER) != want_mover:
            record["problems"].append(f"{MOVER} modal count {modes.get(MOVER)} != {want_mover} for pose {arm['pose']}")
        if gpu.get("complete_frames", 0) < 0.9 * args.samples:
            record["problems"].append(f"only {gpu.get('complete_frames')} of {args.samples} steady frames complete")
        gc = (gpu.get("gpu_frame_complete") or {}).get("per_frame_ms")
        gi = (gpu.get("gpu_frame_incomplete") or {}).get("per_frame_ms")
        if gc and gi and abs(gc - gi) > 0.03 * gc:
            record["problems"].append(f"GPU frame differs complete {gc} vs incomplete {gi} ms: dropped rows lost time")
        p = gpu.get("passes") or {}
        pf = lambda k: (p.get(k) or {}).get("per_frame_ms")
        record.update(depth_ms=pf(DEPTH), forward_ms=pf(FORWARD), mover_ms=pf(MOVER), sdf_ms=pf(SDF),
                      sum_ms=(gpu.get("sum") or {}).get("per_frame_ms"), frame_ms=gc,
                      complete_frames=gpu.get("complete_frames"))
        (RUNS / f"{label}-gpu.json").write_text(json.dumps(gpu, indent=2) + "\n")
    clk = (record["clock"].get("clock_mhz") or {}).get("median")
    if record["clock"]["basis"] != "busy" or clk is None or clk < MIN_BUSY_CLOCK_MHZ:
        record["problems"].append(f"GPU clock not saturated: {record['clock']['basis']} median {clk} MHz")
    if record["locked_seen"]:
        record["problems"].append("screen locked during the run")
    if record["screensaver_seen"]:
        record["problems"].append("screen saver ran during the run")
    if record["foreign_processes_seen"]:
        record["problems"].append(f"other processes seen during the run: {record['foreign_processes_seen']}")
    record["machine_state_ok"] = not (record["locked_seen"] or record["screensaver_seen"]
                                      or record["foreign_processes_seen"])
    record["valid"] = not record["problems"] and gpu is not None
    slim = {k: v for k, v in record.items() if k != "gpu_state_samples"}
    (RUNS / f"{label}.run.json").write_text(json.dumps(slim, indent=2) + "\n")
    (RUNS / f"{label}.clock.json").write_text(json.dumps(gpu_samples) + "\n")
    return record


# --- summary --------------------------------------------------------------------

def fmt(v, digits=3):
    return "n/a" if v is None else f"{v:.{digits}f}"


def clock_text(r):
    c = (r.get("clock") or {}).get("clock_mhz")
    return "n/a" if not c else f"{c['median']} ({c['min']}-{c['max']})"


def temp_text(r):
    c = (r.get("clock") or {}).get("temp_c")
    return "n/a" if not c else f"{c['median']} ({c['min']}-{c['max']})"


FIGS = (("depth_ms", "Depth"), ("forward_ms", "Forward"), ("mover_ms", "Kinematic"),
        ("sdf_ms", "SDF"), ("sum_ms", "Sum"), ("frame_ms", "GPU frame"))


def summarize(records, tag, meta):
    mean = lambda xs: sum(xs) / len(xs) if xs else None
    by = collections.defaultdict(list)
    for r in records:
        by[(r["pose"], r["binary_arm"])].append(r)
    arms = {}
    for (pose, b), rs in by.items():
        good = [r for r in rs if r["valid"]]
        row = {"pose": pose, "binary": b, "runs": len(rs), "valid": len(good)}
        for key, _ in FIGS:
            vals = [r[key] for r in good if r.get(key) is not None]
            row[key] = mean(vals)
            row[key + "_runs"] = vals
        row["clocks"] = [clock_text(r) for r in good]
        row["temps"] = [temp_text(r) for r in good]
        arms[f"{pose}:{b}"] = row
    deltas = {}
    for pose in POSES:
        a, bf = arms.get(f"{pose}:after"), arms.get(f"{pose}:before")
        if not a or not bf or not a["valid"] or not bf["valid"]:
            continue
        d = {}
        for key, _ in FIGS:
            if a[key] is None or bf[key] is None:
                continue
            # Spread: paired per round (after_rN - before_rN) min..max.
            pairs = []
            for ra in by[(pose, "after")]:
                rb = next((x for x in by[(pose, "before")] if x["round"] == ra["round"] and x["valid"]), None)
                if ra["valid"] and rb and ra.get(key) is not None and rb.get(key) is not None:
                    pairs.append(ra[key] - rb[key])
            d[key] = {"delta_ms": a[key] - bf[key], "per_round_ms": pairs,
                      "pct": (a[key] - bf[key]) / bf[key] * 100 if bf[key] else None}
        deltas[pose] = d
    out = {"meta": meta,
           "runs": [{k: v for k, v in r.items() if k != "gpu_state_samples"} for r in records],
           "arms": arms, "deltas": deltas}
    (HERE / f"results-{tag}.json").write_text(json.dumps(out, indent=2) + "\n")
    md = [f"# Mover depth pre-pass A/B ({tag})", "",
          "Per complete steady frame, mean ms. Sum = Depth + Forward (Textured Pass) + Kinematic + SDF. "
          "GPU frame = union of every labelled interval in a frame.", "",
          "| run | valid | " + " | ".join(n for _, n in FIGS) + " | frames | clock MHz | temp C |",
          "|---|---|" + "---|" * len(FIGS) + "---|---|---|"]
    for r in records:
        md.append(f"| {r['label']} | {'yes' if r['valid'] else 'NO'} | "
                  + " | ".join(fmt(r.get(k), 3) for k, _ in FIGS)
                  + f" | {r.get('complete_frames', 'n/a')} | {clock_text(r)} | {temp_text(r)} |")
    md += ["", "| arm | valid/runs | " + " | ".join(n for _, n in FIGS) + " |",
           "|---|---|" + "---|" * len(FIGS)]
    for name, row in arms.items():
        md.append(f"| {name} | {row['valid']}/{row['runs']} | " + " | ".join(fmt(row[k], 3) for k, _ in FIGS) + " |")
    md += ["", "| pose | " + " | ".join(f"Δ {n}" for _, n in FIGS) + " |", "|---|" + "---|" * len(FIGS)]
    for pose, d in deltas.items():
        cells = []
        for k, _ in FIGS:
            if k not in d:
                cells.append("n/a")
                continue
            pr = d[k]["per_round_ms"]
            rng = f" [{min(pr):+.3f}..{max(pr):+.3f}]" if pr else ""
            cells.append(f"{d[k]['delta_ms']:+.3f}{rng}")
        md.append(f"| {pose} | " + " | ".join(cells) + " |")
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "Invalid runs:"] + [f"- {r['label']}: {'; '.join(r['problems'])}" for r in bad]
    (HERE / f"results-{tag}.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))


# --- main -----------------------------------------------------------------------

def git(*a):
    r = subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or None


def parse_arms(text):
    names = text.split(",") if text else [f"{p}:{b}" for p in POSES for b in BINARIES]
    arms = []
    for n in names:
        pose, b = n.split(":")
        if pose not in POSES or b not in BINARIES:
            raise SystemExit(f"bad arm {n}")
        if BINARIES[b] not in os.environ:
            raise SystemExit(f"arm {n} needs {BINARIES[b]}")
        binary = Path(os.environ[BINARIES[b]])
        if binary.name != "postretro" or not binary.is_file():
            raise SystemExit(f"bad engine binary {binary} (must exist and be named postretro)")
        arms.append({"name": n.replace(":", "-"), "pose": pose, "bin": b, "binary": str(binary)})
    return arms


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arms")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--warmup", type=int, default=120)
    ap.add_argument("--samples", type=int, default=240)
    ap.add_argument("--tag", default="batch")
    ap.add_argument("--abort-on-invalid", action="store_true")
    ap.add_argument("--write-scenes", action="store_true")
    args = ap.parse_args()
    if args.write_scenes:
        SCENES.mkdir(exist_ok=True)
        for pose in POSES:
            s = scene_for(pose, "<path to mover-setpiece.prl>", "<output.png>", "<report.json>",
                          args.warmup, args.samples)
            (SCENES / f"{pose}.json").write_text(json.dumps(s, indent=2) + "\n")
        return 0
    arms = parse_arms(args.arms)
    map_path()
    RUNS.mkdir(exist_ok=True)
    trace_dir = Path(tempfile.gettempdir()) / "mover-depth-prepass-traces"
    trace_dir.mkdir(parents=True, exist_ok=True)
    problems = []
    if busy_processes():
        problems.append(f"another process is running: {busy_processes()}")
    if free_gb(trace_dir) < float(os.environ.get("MIN_FREE_GB", "25")):
        problems.append(f"less than {os.environ.get('MIN_FREE_GB', '25')} GB free in {trace_dir}")
    if screen_locked():
        problems.append("screen is locked")
    if problems:
        print("\n".join("ABORT: " + p for p in problems), file=sys.stderr)
        return 3
    flags = xctrace_flags()
    caf = subprocess.Popen(["caffeinate", "-d", "-i", "-m", "-s", "-u", "-t", "14400"])
    awake = threading.Event()

    def redeclare():
        while not awake.is_set():
            subprocess.run(["caffeinate", "-u", "-t", "5"])

    threading.Thread(target=redeclare, daemon=True).start()

    def cleanup():
        awake.set()
        caf.terminate()
        shutil.rmtree(trace_dir, ignore_errors=True)

    atexit.register(cleanup)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(130))
    idle = idle_snapshot(RUNS / f"{args.tag}-idle.ioreg.txt")
    print("idle ioreg:", json.dumps(idle), flush=True)
    meta = {"git_rev": git("rev-parse", "HEAD"), "git_branch": git("rev-parse", "--abbrev-ref", "HEAD"),
            "binaries": {a["bin"]: a["binary"] for a in arms}, "map": str(map_path()),
            "machine": " | ".join(filter(None, [
                subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True).stdout.strip(),
                subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
                "macOS " + subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip()])),
            "xctrace_flags": flags, "idle": idle, "warmup": args.warmup, "samples": args.samples,
            "rounds": args.rounds, "idle_gap_s": float(os.environ.get("IDLE_GAP", "15")),
            "poses": POSES, "resolution": UHD, "fov_deg": FOV, "engine_env": ENGINE_ENV,
            "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    records, code = [], 0
    try:
        for rnd in range(1, args.rounds + 1):
            order = arms if rnd % 2 else list(reversed(arms))
            for a in order:
                busy = busy_processes()
                if busy:
                    print(f"ABORT: another process is running: {busy}", file=sys.stderr)
                    code = 4
                    raise StopIteration
                if free_gb(trace_dir) < float(os.environ.get("MIN_FREE_GB", "25")):
                    print("ABORT: disk below MIN_FREE_GB", file=sys.stderr)
                    code = 4
                    raise StopIteration
                time.sleep(float(os.environ.get("IDLE_GAP", "15")))
                label = f"{args.tag}-{a['name']}-r{rnd}"
                print(f"run {label} ...", flush=True)
                r = run_one(label, a, args, trace_dir, flags, rnd)
                records.append(r)
                print(f"  depth {fmt(r.get('depth_ms'))} fwd {fmt(r.get('forward_ms'))} mover {fmt(r.get('mover_ms'))} "
                      f"sdf {fmt(r.get('sdf_ms'))} sum {fmt(r.get('sum_ms'))} frame {fmt(r.get('frame_ms'))} "
                      f"clock {clock_text(r)} temp {temp_text(r)} valid={r['valid']} "
                      f"{r['problems'] if not r['valid'] else ''}", flush=True)
                if not r["machine_state_ok"] or (args.abort_on_invalid and not r["valid"]):
                    print(f"ABORT: batch stopped at {label}", file=sys.stderr)
                    code = 4
                    raise StopIteration
    except StopIteration:
        pass
    finally:
        if records:
            summarize(records, args.tag, meta)
    return code


if __name__ == "__main__":
    sys.exit(main())
