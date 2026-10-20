"""Metal System Trace machinery for the fluid SSR reflections kit (Mac).

Not run directly: measurements/fluid-ssr-reflections/metal/run_sweep.py
imports this module and drives run_one() with its arms.

Copied from measurements/fluid-base-pass-cost/metal/run.py, itself the
cell-rate spike's kit (claude/cell-rate-spike). Kept verbatim: caffeinate and
the idle gap (in the driver), idle ioreg snapshot, ioreg GPU clock/temperature
polling every 0.5 s, screen-lock, screen-saver and busy-process checks every
2 s (cargo, rustc, prl-build or a second postretro mark the run invalid),
`xctrace record --launch` of a headless `--capture` with its watchdog, export
of metal-gpu-intervals, the complete-frame reduction (gpu_time.py), and
deleting the trace bundle and instruments*.ktrace once reduced.

What changed, and why:
- The cell-rate spike's arms, scene levers (`fluid_reflection_cells`, the
  1660 branch's booleans), standalone main() and summary are gone: the
  capture scene is `deny_unknown_fields`, so those levers no longer parse.
  run_sweep.py owns arms, presets and the summary.
- write_scene() writes the fluid SSR scene (`reflection_quality`,
  `reflection_style`, `reflection_march`, each only when the arm sets it; no
  `light_term_mask`, so every term, specular included, is on).
- The Fluid Reflection Cell Pass checks become Fluid Reflection Pass checks:
  expected but absent, or present at quality off, is an anomaly.
- ENGINE_ENV keeps POSTRETRO_SH_STREAMING=sync-proof by default, matching
  the 1660 run sheet (override with SH_STREAMING=<mode>); RUST_LOG=info stays.
- The binary comes from SSR_BIN (CELL_BIN in the old kit).
- Per-run VRAM free: gpu_state() also reads vramFreeBytes, once before
  launch and on every poll.
- Optional CLOCK_FLOOR_MHZ: the old kit had no clock floor, so none is the
  default; set it to mark runs below that busy median clock invalid.
"""
import argparse, atexit, collections, gzip, json, os, re, shutil, signal, subprocess, sys
import tempfile, threading, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gpu_time import reduce_export, FLUID_LABEL, REFLECTION_LABEL, FORWARD_LABEL  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
RUNS = HERE / "runs"
# Results sit one level up: measurements/fluid-ssr-reflections/results-<tag>.*
RESULTS_DIR = HERE.parent
RAW = HERE / "raw"
BIN_ENV = "SSR_BIN"

UHD = [3840, 2160]
FHD = [1920, 1080]
FOV = 100.0
ENGINE_ENV = {
    "RUST_LOG": "info",
    "POSTRETRO_SH_STREAMING": os.environ.get("SH_STREAMING", "sync-proof"),
}
# Another engine, cargo or compiler process skews GPU pass times and the batch
# must not start or continue beside one.
BUSY_PROCESSES = ("postretro", "cargo", "rustc", "prl-build")
XPATH = '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'
GPU_FIELDS = ("Core Clock(MHz)", "Memory Clock(MHz)", "Temperature(C)", "Total Power(W)",
              "GPU Activity(%)", "Fan Speed(RPM)", "vramFreeBytes", "inUseVidMemoryBytes")
# A clock sample counts as "busy" at or above this GPU activity. Setup, shader
# compile and the output readback sit outside the timed span and read idle.
BUSY_ACTIVITY = 50
# Assumed xctrace flags for --dry-run, which must not run xctrace.
DRY_RUN_FLAGS = {"env": True, "target_stdout": True, "no_prompt": True}


def engine_binary():
    return Path(os.environ.get(BIN_ENV, ROOT / "target/release/postretro"))


# --- machine state ---------------------------------------------------------

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
    """One sample of the 5300M's (Navi 14) clocks, temperature and power."""
    r = subprocess.run(["ioreg", "-c", "AMDRadeonX6000_AMDNavi14GraphicsAccelerator", "-r", "-d1", "-w0"],
                       capture_output=True, text=True)
    if "Core Clock(MHz)" not in r.stdout:
        # Not the Navi 14 class name: fall back to the generic accelerator list
        # (rendering_pipeline.md section 12 reads PerformanceStatistics there).
        r = subprocess.run(["ioreg", "-c", "IOAccelerator", "-r", "-d1", "-w0"],
                           capture_output=True, text=True)
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
    """Core clock and temperature over the busy samples, else over all of them."""
    busy = [s for s in samples if s.get("GPU Activity(%)", 0) >= BUSY_ACTIVITY]
    basis, pool = ("busy", busy) if len(busy) >= 5 else ("all", samples)
    out = {"basis": basis, "samples": len(pool), "samples_total": len(samples)}
    for field, key in (("Core Clock(MHz)", "clock_mhz"), ("Temperature(C)", "temp_c"),
                       ("Total Power(W)", "power_w"), ("GPU Activity(%)", "activity_pct")):
        vals = [s[field] for s in pool if field in s]
        out[key] = {"min": min(vals), "median": median(vals), "max": max(vals)} if vals else None
    # VRAM over every sample, not the busy pool: paging pressure does not
    # follow GPU activity. MB, from PerformanceStatistics' vramFreeBytes.
    vram = [s["vramFreeBytes"] // (1 << 20) for s in samples if "vramFreeBytes" in s]
    out["vram_free_mb"] = {"min": min(vram), "median": median(vram), "max": max(vram)} if vram else None
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


# --- scenes ----------------------------------------------------------------

# The three fluid SSR scene fields. They are written through verbatim when an
# arm sets them and omitted otherwise (crates/postretro/src/capture/scene.rs
# is deny_unknown_fields, so a binary without them rejects any scene that
# sets one; the smoke capture catches that before the batch).
SCENE_FIELDS = ("reflection_quality", "reflection_style", "reflection_march")


def scene_dict(arm, png, report, warmup, samples):
    scene = {
        "map": arm["map"],
        "camera": dict(arm["camera"]),
        "resolution": arm["resolution"],
        "output": str(png),
        "measurement": {"report": str(report), "warmup_frames": warmup, "sample_frames": samples},
    }
    # light_term_mask omitted: every term (the old kit forced 0x13F, which
    # drops specular and so selects the reflection-free pipeline).
    for key in SCENE_FIELDS:
        if arm.get(key) is not None:
            scene[key] = arm[key]
    return scene


def write_scene(path, arm, png, report, warmup, samples):
    scene = scene_dict(arm, png, report, warmup, samples)
    path.write_text(json.dumps(scene, indent=2) + "\n")
    return scene


# --- one run ---------------------------------------------------------------

def engine_env():
    env = os.environ.copy()
    # Release builds default indirect-call validation off; an inherited override
    # would change render_submit cost between runs.
    env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
    env.update(ENGINE_ENV)
    return env


def xctrace_flags():
    r = subprocess.run(["xcrun", "xctrace", "record", "-h"], capture_output=True, text=True)
    text = r.stdout + r.stderr
    return {"env": "--env" in text, "target_stdout": "--target-stdout" in text, "no_prompt": "--no-prompt" in text}


def kill_engine():
    """Stop engine processes. The batch refuses to start beside another one, so
    any postretro running during a run is this run's."""
    subprocess.run(["pkill", "-x", "postretro"], capture_output=True)


def delete_ktrace(since):
    tmp = Path(os.environ.get("TMPDIR", "/tmp"))
    for f in tmp.glob("instruments*.ktrace"):
        try:
            if f.stat().st_mtime >= since:
                shutil.rmtree(f) if f.is_dir() else f.unlink()
        except OSError as e:
            print(f"could not delete {f}: {e}", file=sys.stderr)


def record_command(trace, log, scene_path, flags):
    cmd = ["xcrun", "xctrace", "record", "--template", "Metal System Trace", "--output", str(trace)]
    if flags["no_prompt"]:
        cmd += ["--no-prompt"]
    if flags["target_stdout"]:
        cmd += ["--target-stdout", str(log)]
    if flags["env"]:
        for k, v in ENGINE_ENV.items():
            cmd += ["--env", f"{k}={v}"]
    cmd += ["--launch", "--", str(engine_binary()), "--capture", str(scene_path)]
    return cmd

def run_one(label, arm, round_i, args, trace_dir, flags):
    """Record, export, reduce and delete one run. Returns its record."""
    png = RUNS / f"{arm['name']}.png" if round_i == 1 else trace_dir / f"{label}.png"
    report = RUNS / f"{label}.report.json"
    scene_path = RUNS / f"{label}.scene.json"
    log = RUNS / f"{label}.log"
    status = RUNS / f"{label}.trace-status.txt"
    trace = trace_dir / f"{label}.trace"
    xml = trace_dir / f"{label}-gpu.xml"
    for stale in (report, png):
        stale.unlink(missing_ok=True)
    write_scene(scene_path, arm, png, report, args.warmup, args.samples)

    cmd = record_command(trace, log, scene_path, flags)

    record = {"label": label, "arm": arm["name"], "spec": arm["spec"], "round": round_i,
              "pose": arm["pose"], "res": arm["res"], "config": arm["config"],
              "quality": arm["quality"], "style": arm["style"], "march": arm["march"],
              "reflection_expected": arm["reflection_expected"],
              "map": arm["map"], "camera": arm["camera"], "warmup_frames": args.warmup,
              "sample_frames": args.samples, "resolution": arm["resolution"], "command": cmd,
              "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "problems": [],
              "anomalies": []}
    # The GPU's state just before launch, after the idle gap: VRAM free here is
    # what other GPU clients left the engine.
    record["pre_launch"] = gpu_state()
    gpu_samples, sampling = [], threading.Event()
    checks = []
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

    x = None
    poller = None
    try:
        sampling.set()
        poller = threading.Thread(target=poll_state, daemon=True)
        poller.start()
        with status.open("w") as sf:
            x = subprocess.Popen(cmd, cwd=ROOT, env=engine_env(), stdout=sf, stderr=subprocess.STDOUT)
            report_seen = interrupted = None
            timeout = float(os.environ.get("RECORD_TIMEOUT", "1200"))
            # Watchdog: xctrace record can hang after the target exits and stall
            # a batch (rendering_pipeline.md section 12).
            while x.poll() is None:
                now = time.time()
                if report_seen is None and report.exists():
                    report_seen = now
                    sampling.clear()  # the timed span is over; setup and finalize are not clock-relevant
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
                    record["watchdog"] = True
                    record["problems"].append(f"watchdog: run exceeded {timeout:.0f} s; engine and xctrace stopped")
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
    # The batch's own children never match; anything else seen is a foreign process.
    record["foreign_processes_seen"] = sorted({n for c in checks for n in c["busy"] if n != "postretro"})

    # The capture report is the engine's own proof that it finished; the log
    # holds only what xctrace forwarded from the target's stdout.
    try:
        rep = json.loads(report.read_text())
        record["adapter"] = rep.get("adapter")
        record["cpu_completion_median_ms"] = (rep.get("cpu_completion") or {}).get("median_ms")
        record["capture"] = rep.get("capture")
        record["engine_revision"] = rep.get("revision")
    except (OSError, ValueError) as e:
        record["problems"].append(f"no capture report: {e}")
    adapter = json.dumps(record.get("adapter") or {}).lower()
    if record.get("adapter") and not ("discrete" in adapter and "metal" in adapter):
        record["problems"].append(f"adapter is not a discrete Metal GPU: {record['adapter']}")

    # Export and reduce, then delete the ~GB trace and its ktrace temp file.
    try:
        if trace.exists():
            r = subprocess.run(["xcrun", "xctrace", "export", "--input", str(trace), "--xpath", XPATH,
                                "--output", str(xml)], capture_output=True, text=True)
            if r.returncode != 0 or not xml.exists():
                record["problems"].append(f"xctrace export failed: {r.stderr.strip()[:300]}")
            else:
                expected = arm["reflection_expected"]
                record["gpu"] = reduce_export(str(xml), args.warmup, args.samples,
                                              "auto" if expected is None else expected)
                if os.environ.get("SKIP_RAW") != "1":
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
        delete_ktrace(started - 1)
        if round_i != 1:
            png.unlink(missing_ok=True)

    gpu = record.get("gpu")
    if gpu:
        steady = gpu.get("steady") or {}
        fluid_all = gpu["reported"][FLUID_LABEL]
        record["fluid_encoders"] = fluid_all["encoders"]
        record["fluid_ms_whole_run"] = fluid_all["per_frame_ms"]
        record["forward_ms_whole_run"] = gpu["reported"][FORWARD_LABEL]["per_frame_ms"]
        record["frames_precedent"] = gpu["camera_frames"]
        complete = gpu.get("complete") or {}
        record["complete_frames"] = complete.get("complete_frames")
        record["gpu_frame_ms_complete"] = complete.get("gpu_frame_ms_complete")
        record["gpu_frame_ms_incomplete"] = complete.get("gpu_frame_ms_incomplete")
        if complete.get("available"):
            # Primary basis (gpu_time.py docstring): frames where the trace kept
            # every required row, so no label is shifted.
            record["basis"] = "complete"
            record["steady_frames"] = complete["steady_frames"]
            record["fluid_ms"] = complete["passes"][FLUID_LABEL]["per_frame_ms"]
            record["fluid_ms_p10_p90"] = [complete["passes"][FLUID_LABEL]["p10_ms"],
                                          complete["passes"][FLUID_LABEL]["p90_ms"]]
            record["forward_ms"] = complete["passes"][FORWARD_LABEL]["per_frame_ms"]
            refl = complete["passes"].get(REFLECTION_LABEL)
            # Absent is None, never 0: an expected-but-absent pass is an
            # anomaly below, and an arm without the pass has no figure.
            record["fluid_reflection_ms"] = refl["per_frame_ms"] if refl else None
            if refl:
                record["fluid_reflection_ms_p10_p90"] = [refl["p10_ms"], refl["p90_ms"]]
            record["cost_ms"] = complete["cost"]["per_frame_ms"]
            record["cost_ms_p10_p90"] = [complete["cost"]["p10_ms"], complete["cost"]["p90_ms"]]
            if steady.get("available"):
                record["fluid_ms_steady_slice"] = steady["passes"][FLUID_LABEL]["per_frame_ms"]
            gc, gi = complete["gpu_frame_ms_complete"], complete["gpu_frame_ms_incomplete"]
            # The cross-check needs enough incomplete frames to have a mean:
            # a couple of incomplete frames out of 360 (a clock dip, a
            # coalesced row) is noise, not dropped time. Require 5% of steady.
            n_incomplete = complete["steady_frames"] - complete["complete_frames"]
            if gc and gi and n_incomplete >= 0.05 * complete["steady_frames"] and abs(gc - gi) > 0.03 * gc:
                record["problems"].append(
                    f"GPU frame time differs between complete ({gc} ms) and incomplete ({gi} ms) frames: "
                    "dropped rows lost time, not only labels")
        elif steady.get("available"):
            record["problems"].append("no complete-frame basis: " + str(complete.get("reason", "not reduced")))
            record["basis"] = "steady"
            record["steady_frames"] = steady["frames"]
            record["fluid_ms"] = steady["passes"][FLUID_LABEL]["per_frame_ms"]
            record["fluid_ms_per_encoder"] = steady["passes"][FLUID_LABEL]["per_encoder_ms"]
            record["forward_ms"] = steady["passes"][FORWARD_LABEL]["per_frame_ms"]
            record["fluid_steady_encoders"] = steady["passes"][FLUID_LABEL]["encoders"]
        else:
            record["basis"] = "whole-run"
            record["fluid_ms"] = fluid_all["per_frame_ms"]
            record["forward_ms"] = record["forward_ms_whole_run"]
            record["problems"].append("no steady-state slice: " + str(steady.get("reason", "no expected frames")))
        if not fluid_all["encoders"]:
            record["problems"].append("fluid pass absent from the trace (no Fluid Surface Pass encoders)")
        refl_all = gpu["reported"][REFLECTION_LABEL]["encoders"]
        record["fluid_reflection_encoders"] = refl_all
        expected = arm["reflection_expected"]
        if expected == 1 and not refl_all:
            record["fluid_reflection_ms"] = None
            record["anomalies"].append(
                f"Fluid Reflection Pass expected at quality {arm['quality']} but absent from the trace")
        if expected == 0 and refl_all:
            record["anomalies"].append(
                f"Fluid Reflection Pass recorded {refl_all} encoders at quality {arm['quality']}, which records none")
        if expected is None:
            record["reflection_inferred"] = gpu.get("reflection_inferred")
        record["problems"] += record["anomalies"]
        if record.get("basis") != "complete":
            record["problems"].append("figures need the complete-frame basis (per-pass time per complete frame)")
        elif record.get("basis") != "complete" and steady.get("available")                 and steady["passes"][FLUID_LABEL]["encoders"] < 0.95 * steady["frames"]:
            record["problems"].append(
                f"fluid pass in only {steady['passes'][FLUID_LABEL]['encoders']} of {steady['frames']} steady frames")
        if gpu.get("frame_count_matches_expected") is False:
            record["problems"].append(
                f"frame count {gpu['camera_frames']} ({gpu['frame_count_pass']}) differs from expected {gpu['expected_frames']}")
        (RUNS / f"{label}-gpu.json").write_text(json.dumps(gpu, indent=2) + "\n")
    if record["locked_seen"]:
        record["problems"].append("screen locked during the run")
    if record["screensaver_seen"]:
        record["problems"].append("screen saver ran during the run")
    if record["foreign_processes_seen"]:
        record["problems"].append(f"other processes seen during the run: {record['foreign_processes_seen']}")
    floor = os.environ.get("CLOCK_FLOOR_MHZ")
    busy_clock = (record["clock"] or {}).get("clock_mhz")
    if floor and busy_clock and busy_clock["median"] < float(floor):
        record["problems"].append(f"GPU clock median {busy_clock['median']} MHz below CLOCK_FLOOR_MHZ {floor}")
    record["machine_state_ok"] = not (record["locked_seen"] or record["screensaver_seen"]
                                      or record["foreign_processes_seen"])
    record["valid"] = not record["problems"] and gpu is not None
    (RUNS / f"{label}.run.json").write_text(json.dumps(record, indent=2) + "\n")
    return record

# --- summary helpers -------------------------------------------------------

def fmt(v, digits=3):
    return "n/a" if v is None else f"{v:.{digits}f}"


def clock_text(r):
    c = (r.get("clock") or {}).get("clock_mhz")
    return "n/a" if not c else f"{c['median']} ({c['min']}-{c['max']})"



def git(*a):
    r = subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or None


def smoke(arm):
    """Untraced 2+2-frame capture of one arm: proves the binary parses this
    arm's scene (deny_unknown_fields: a binary without the SSR scene fields
    fails here), the .prl exists and capture works. Returns (ok, output)."""
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        write_scene(d / "scene.json", arm, d / "smoke.png", d / "smoke.report.json", 2, 2)
        r = subprocess.run([str(engine_binary()), "--capture", str(d / "scene.json")],
                           cwd=ROOT, env=engine_env(), capture_output=True, text=True, timeout=600)
        ok = r.returncode == 0 and (d / "smoke.report.json").exists()
        return ok, (r.stdout + r.stderr)[-2000:]
