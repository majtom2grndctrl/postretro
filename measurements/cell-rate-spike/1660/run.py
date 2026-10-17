"""Metal System Trace batch for the fluid reflection cell-rate spike (Mac).

usage: python3 measurements/cell-rate-spike/run.py [options]
  --rounds N        interleaved rounds (default 2: arm order forward, then reversed)
  --only a,b        run only these arms (names as in --list)
  --list            print the arms and exit
  --dry-run         write the scene files and print the plan; run nothing
  --skip-smoke      skip the untraced 4-frame smoke capture that opens the batch
  --skip-stats      skip the untraced count captures (cells, visible fluid pixels,
                    atlas size) that follow the smoke capture
  --warmup N / --samples N
                    capture frames before / during the timed span (defaults 240 / 240;
                    env CELL_WARMUP / CELL_SAMPLES). The trace records the whole
                    process, so total frames traced = warmup + samples.
env (optional):
  CELL_BIN          engine binary (default target/release/postretro)
  TRACE_DIR         where .trace bundles and exports live while a run is reduced
                    (default $TMPDIR/cell-rate-spike-traces)
  IDLE_GAP          seconds of idle before every launch (default 15)
  RECORD_TIMEOUT    seconds before a run's watchdog kills it (default 1200)
  MIN_FREE_GB       abort when TRACE_DIR has less free space (default 25)
  SKIP_RAW=1        do not keep the gzipped trace exports in raw/

Copied from measurements/reflection-blend-cost-spike/run.py (branch
claude/probe-streaming-brief-364a5e) with the arms, scene levers, per-arm cost
(Fluid Surface Pass + Fluid Reflection Cell Pass) and the count captures
changed; the recording, watchdog, machine-state checks and reduction are that
kit's. Its history: adapted from measurements/shadow-fill-cost/{run.py,clean.sh} and
measurements/sh-compose-row-cost-spike/{run.py,batch.sh}: caffeinate held with
user activity re-declared, 15 s idle gap before every launch, idle ioreg
snapshot, ioreg GPU clock/temperature polling, screen-lock and screen-saver
checks, xctrace export of metal-gpu-intervals, per-encoder interval-union
reduction (gpu_time.py), trace and instruments*.ktrace deleted once reduced,
the batch stopping at the first machine-state failure. The one difference from
the precedent's recording: those runs attached xctrace to a live windowed
engine for 4 s. This batch records a headless `--capture` run with
`xctrace record --launch` (rendering_pipeline.md section 12), so there is no
window and no foreground helper.

Every run is one engine process: `target/release/postretro --capture <scene>`
at the arm's resolution, POSTRETRO_SH_STREAMING=sync-proof, RUST_LOG=info. The
scene file is written to runs/<label>.scene.json. Results: results.json and
results.md.
"""
import argparse, atexit, collections, gzip, json, os, re, shutil, signal, subprocess, sys
import tempfile, threading, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gpu_time import reduce_export, FLUID_LABEL, CELL_LABEL, FLUID_TOTAL_LABEL, FORWARD_LABEL  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RUNS = HERE / "runs"
RAW = HERE / "raw"

RESOLUTIONS = {"4k": [3840, 2160], "1080p": [1920, 1080]}
FOV = 100.0
# Engine coordinates (x = -map y, y = map z, z = -map x, metres; forward is
# (-sin yaw, 0, -cos yaw)).
E24_MAP = "content/dev/maps/e24-environment-test.prl"
PROBES_MAP = "content/dev/maps/reflection-probes-test.prl"
POSES = {
    # e24's still pool (fluids/water_64 at scale 1, 256x256 cells), straight down.
    "near": {"map": E24_MAP, "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -89.0,
             "resolutions": ("4k", "1080p")},
    # reflection-probes-test's pool, straight down from its centre.
    "probes": {"map": PROBES_MAP, "position": [-6.5024, 1.6, -6.5024], "yaw": 0.0, "pitch": -89.0,
               "resolutions": ("4k",)},
    # e24's fluid-motion wing from map (2300, 1000, 400), 10.6 m above the water,
    # looking south (map -y) over both rows of pools, 10-26 m away along the view
    # (the scale-0.25 fine pool, about 1M cells, included): cells near or below a
    # pixel. Positions on the x 2064 line render half void (left out).
    "far": {"map": E24_MAP, "position": [-25.4, 10.16, -58.42], "yaw": -90.0, "pitch": -30.0,
            "resolutions": ("4k", "1080p")},
}
# Fluid path: per-fragment pixel style, the cell buffer, the cell buffer with
# its per-cell frustum cull.
MODES = {"frag": (False, False), "cell": (True, False), "cellcull": (True, True), "cell2": (True, False), "frag2": (False, False),
         "free": (False, False)}
# `cell2`: the cell buffer in two draws (the surface through the reflection-free
# pipeline, then a premultiplied reflection overlay; fluid_cell_overlay).
# The `free` baseline: every draw takes the reflection-free pipeline under the
# all-terms mask (the spike's fluid_reflection_off lever), so the fluid pass
# runs its lighting exactly as the reflecting arms do, with no reflection.
# (A lighting mask without one bit also shuts every reflecting gate, but on
# e24 such a mask made the fluid pass ~8 ms cheaper at near 4K even against
# the cell buffer: it changes more than the reflection, so it is no baseline.)
# Live-lighting ratio as shipped, or forced to 1 in both paths (the precompute's floor).
RATIOS = {"live": False, "r1": True}
ENGINE_ENV = {"POSTRETRO_SH_STREAMING": "sync-proof", "RUST_LOG": "info"}
# Another engine, cargo or compiler process skews GPU pass times and the batch
# must not start or continue beside one.
BUSY_PROCESSES = ("postretro", "cargo", "rustc", "prl-build")
XPATH = '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'
GPU_FIELDS = ("Core Clock(MHz)", "Memory Clock(MHz)", "Temperature(C)", "Total Power(W)",
              "GPU Activity(%)", "Fan Speed(RPM)")
# A clock sample counts as "busy" at or above this GPU activity. Setup, shader
# compile and the output readback sit outside the timed span and read idle.
BUSY_ACTIVITY = 50


def views():
    """Every (pose, resolution) pair measured."""
    return [(pose, res) for pose, p in POSES.items() for res in p["resolutions"]]


def arms():
    out = []
    for pose, res in views():
        for ratio, ratio_one in RATIOS.items():
            for mode, (cells, cull) in MODES.items():
                if mode == "free" and ratio_one:
                    continue  # no reflection, so no ratio
                out.append({"name": f"{pose}-{res}-{mode}-{ratio}", "pose": pose, "res": res,
                            "mode": mode, "ratio": ratio, "cell_buffer": cells, "cell_cull": cull,
                            "ratio_one": ratio_one})
    return out


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

def write_scene(path, arm, png, report, warmup, samples, count=False):
    pose = POSES[arm["pose"]]
    scene = {
        "map": pose["map"],
        "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                   "pitch_deg": pose["pitch"], "fov_deg": FOV},
        "resolution": RESOLUTIONS[arm["res"]],
        "output": str(png),
        "measurement": {"report": str(report), "warmup_frames": warmup, "sample_frames": samples},
        "fluid_cell_buffer": arm["cell_buffer"],
        "fluid_cell_cull": arm["cell_cull"],
        "fluid_ratio_one": arm["ratio_one"],
    }
    if arm["mode"] == "free":
        scene["fluid_reflection_off"] = True
    if arm["mode"] == "cell2":
        scene["fluid_cell_overlay"] = True
    if arm["mode"] == "frag2":
        # Per-fragment reflection in two draws: no cells, the split alone.
        scene["fluid_frag_overlay"] = True
    if count:
        # Untraced count captures only: the atomic and the occlusion query cost GPU time.
        scene["fluid_cell_count"] = True
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

    cmd = ["xcrun", "xctrace", "record", "--template", "Metal System Trace", "--output", str(trace)]
    if flags["no_prompt"]:
        cmd += ["--no-prompt"]
    if flags["target_stdout"]:
        cmd += ["--target-stdout", str(log)]
    if flags["env"]:
        for k, v in ENGINE_ENV.items():
            cmd += ["--env", f"{k}={v}"]
    cmd += ["--launch", "--", str(engine_binary()),
            "--capture", str(scene_path)]

    record = {"label": label, "arm": arm["name"], "round": round_i, "pose": arm["pose"],
              "res": arm["res"], "mode": arm["mode"], "ratio": arm["ratio"],
              "map": POSES[arm["pose"]]["map"], "warmup_frames": args.warmup,
              "sample_frames": args.samples, "resolution": RESOLUTIONS[arm["res"]], "command": cmd,
              "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "problems": []}
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
                record["gpu"] = reduce_export(str(xml), args.warmup, args.samples)
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
            # An arm's cost is its fluid pass plus its cell pass, frame by frame.
            record["basis"] = "complete"
            record["steady_frames"] = complete["steady_frames"]
            passes = complete["passes"]
            record["fluid_ms"] = passes[FLUID_TOTAL_LABEL]["per_frame_ms"]
            record["fluid_ms_p10_p90"] = [passes[FLUID_TOTAL_LABEL]["p10_ms"],
                                          passes[FLUID_TOTAL_LABEL]["p90_ms"]]
            record["fluid_pass_ms"] = passes[FLUID_LABEL]["per_frame_ms"]
            record["cell_pass_ms"] = (passes.get(CELL_LABEL) or {}).get("per_frame_ms", 0.0)
            if arm["cell_buffer"] and CELL_LABEL not in passes:
                record["problems"].append("cell arm without a Fluid Reflection Cell Pass in its complete frames")
            if not arm["cell_buffer"] and CELL_LABEL in passes:
                record["problems"].append("per-fragment arm recorded a Fluid Reflection Cell Pass")
            record["forward_ms"] = complete["passes"][FORWARD_LABEL]["per_frame_ms"]
            if steady.get("available"):
                record["fluid_ms_steady_slice"] = steady["passes"][FLUID_LABEL]["per_frame_ms"]
            gc, gi = complete["gpu_frame_ms_complete"], complete["gpu_frame_ms_incomplete"]
            if gc and gi and abs(gc - gi) > 0.03 * gc:
                record["problems"].append(
                    f"GPU frame time differs between complete ({gc} ms) and incomplete ({gi} ms) frames: "
                    "dropped rows lost time, not only labels")
        elif steady.get("available"):
            record["problems"].append("no complete-frame basis: " + str(complete.get("reason", "not reduced")))
            record["basis"] = "steady"
            record["steady_frames"] = steady["frames"]
            record["fluid_ms"] = (steady["passes"][FLUID_LABEL]["per_frame_ms"] or 0.0) + \
                (steady["passes"][CELL_LABEL]["per_frame_ms"] or 0.0)
            record["fluid_ms_per_encoder"] = steady["passes"][FLUID_LABEL]["per_encoder_ms"]
            record["forward_ms"] = steady["passes"][FORWARD_LABEL]["per_frame_ms"]
            record["fluid_steady_encoders"] = steady["passes"][FLUID_LABEL]["encoders"]
        else:
            record["basis"] = "whole-run"
            record["fluid_ms"] = (fluid_all["per_frame_ms"] or 0.0) + \
                (gpu["reported"][CELL_LABEL]["per_frame_ms"] or 0.0)
            record["forward_ms"] = record["forward_ms_whole_run"]
            record["problems"].append("no steady-state slice: " + str(steady.get("reason", "no expected frames")))
        if not fluid_all["encoders"]:
            record["problems"].append("fluid pass absent from the trace (no Fluid Surface Pass encoders)")
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
    record["machine_state_ok"] = not (record["locked_seen"] or record["screensaver_seen"]
                                      or record["foreign_processes_seen"])
    record["valid"] = not record["problems"] and gpu is not None
    (RUNS / f"{label}.run.json").write_text(json.dumps(record, indent=2) + "\n")
    return record


# --- summary ---------------------------------------------------------------

def fmt(v, digits=3):
    return "n/a" if v is None else f"{v:.{digits}f}"


def clock_text(r):
    c = (r.get("clock") or {}).get("clock_mhz")
    return "n/a" if not c else f"{c['median']} ({c['min']}-{c['max']})"


def mean(xs):
    return sum(xs) / len(xs) if xs else None


def stats_rows(stats):
    """Per view: cells, visible fluid pixels, pixels per cell, atlas size."""
    rows = []
    for key, st in stats.items():
        s = st.get("fluid_cell_stats") or {}
        samples = s.get("fluid_samples_passed")
        dispatched = s.get("cells_dispatched")
        evaluated = s.get("cells_evaluated")
        rows.append({
            "view": key, "atlas": f"{s.get('atlas_width')}x{s.get('atlas_height')}" if s else None,
            "atlas_bytes": s.get("atlas_bytes"), "cells_total": s.get("cells_total"),
            "faces_dispatched": s.get("faces_dispatched"), "cells_dispatched": dispatched,
            "cells_evaluated_cull": evaluated, "visible_fluid_pixels": samples,
            "pixels_per_cell": samples / dispatched if samples is not None and dispatched else None,
            "pixels_per_cell_cull": samples / evaluated if samples is not None and evaluated else None,
            "problems": st.get("problems", []),
        })
    return rows


def summarize(records, meta, stats=None, name="results", title="Mac Metal System Trace results"):
    stats = stats or {}
    by_arm = collections.defaultdict(list)
    for r in records:
        by_arm[r["arm"]].append(r)
    arm_rows = []
    frag = {}
    for arm_name, rs in by_arm.items():
        valid = [r for r in rs if r["valid"] and r.get("fluid_ms") is not None]
        good = [r["fluid_ms"] for r in valid]
        row = {"arm": arm_name, "pose": rs[0]["pose"], "res": rs[0]["res"], "mode": rs[0]["mode"],
               "ratio": rs[0]["ratio"], "runs": len(rs), "valid_runs": len(good),
               "fluid_ms_mean": mean(good),
               "fluid_ms_spread": (max(good) - min(good)) if len(good) > 1 else None,
               "fluid_pass_ms_mean": mean([r["fluid_pass_ms"] for r in valid if r.get("fluid_pass_ms") is not None]),
               "cell_pass_ms_mean": mean([r["cell_pass_ms"] for r in valid if r.get("cell_pass_ms") is not None]),
               "forward_ms_mean": mean([r["forward_ms"] for r in valid if r.get("forward_ms") is not None])}
        arm_rows.append(row)
        if row["mode"] == "frag":
            frag[(row["pose"], row["res"], row["ratio"])] = row["fluid_ms_mean"]
    for row in arm_rows:
        base = frag.get((row["pose"], row["res"], row["ratio"]))
        m = row["fluid_ms_mean"]
        row["vs_frag_ms"] = (m - base) if m is not None and base is not None else None
        row["vs_frag_ratio"] = (m / base) if m is not None and base else None
    arm_rows.sort(key=lambda r: (list(POSES).index(r["pose"]), r["res"], r["ratio"], list(MODES).index(r["mode"])))
    srows = stats_rows(stats)

    out = {"meta": meta, "stats": srows,
           "runs": [{k: v for k, v in r.items() if k not in ("gpu_state_samples", "command")} for r in records],
           "arms": arm_rows}
    (HERE / f"{name}.json").write_text(json.dumps(out, indent=2) + "\n")

    md = [f"# Fluid reflection cell-rate spike: {title}", "",
          f"- Commit: `{meta.get('git_rev')}` on `{meta.get('git_branch')}`; engine binary `{meta.get('binary')}`",
          f"- Machine: {meta.get('machine')}",
          f"- Adapter (from the capture reports): {meta.get('adapter')}",
          f"- Capture: warmup {meta['warmup']} + {meta['samples']} frames per run, POSTRETRO_SH_STREAMING=sync-proof, "
          "pixel style, reflectivity 1 (capture's fixed look)",
          f"- Idle before the batch (ioreg IOAccelerator, one value per accelerator): {json.dumps(meta.get('idle'))}",
          "- `cost ms` (basis `complete`) is Fluid Surface Pass + Fluid Reflection Cell Pass GPU time per frame, "
          "averaged over steady frames (the last `samples` frames framed by the Candidate Cull Pass) that hold exactly "
          "one depth, forward and fluid encoder, and one cell encoder in a cell arm; the trace drops rows in some "
          "frames and shifts labels there. `GPU frame` is the label-independent union of every labelled interval per "
          "frame over complete / incomplete frames: agreement means only labels were lost. `clock` is the median core "
          "clock over the busy samples, with min-max. Modes: `frag` per-fragment pixel style; `cell` the cell buffer; "
          "`cellcull` the cell buffer with its per-cell frustum cull. Ratios: `live` the live-lighting ratio as "
          "shipped; `r1` forced to 1 in both paths.", "",
          "## Per arm (valid runs)", "",
          "| arm | cost ms mean | run spread ms | fluid pass ms | cell pass ms | vs frag ms | vs frag x | forward ms | valid/total |",
          "|---|---|---|---|---|---|---|---|---|"]
    for row in arm_rows:
        md.append(f"| {row['arm']} | {fmt(row['fluid_ms_mean'], 4)} | {fmt(row['fluid_ms_spread'], 4)} | "
                  f"{fmt(row['fluid_pass_ms_mean'], 4)} | {fmt(row['cell_pass_ms_mean'], 4)} | "
                  f"{fmt(row['vs_frag_ms'], 4)} | {fmt(row['vs_frag_ratio'], 3)} | "
                  f"{fmt(row['forward_ms_mean'], 3)} | {row['valid_runs']}/{row['runs']} |")
    md += ["", "## Cells and pixels per view (untraced count captures, cell buffer + cull)", "",
           "`visible fluid px` is the fluid pass's occlusion-query sample count (every fluid draw, overdraw counted); "
           "`cells` is every cell of the dispatched faces' rects; `cells (cull)` those the frustum cull kept.", "",
           "| view | atlas | atlas bytes | faces | cells | cells (cull) | visible fluid px | px/cell | px/cell (cull) |",
           "|---|---|---|---|---|---|---|---|---|"]
    for s in srows:
        md.append(f"| {s['view']} | {s['atlas']} | {s['atlas_bytes']} | {s['faces_dispatched']} | {s['cells_dispatched']} | "
                  f"{s['cells_evaluated_cull']} | {s['visible_fluid_pixels']} | {fmt(s['pixels_per_cell'], 2)} | "
                  f"{fmt(s['pixels_per_cell_cull'], 2)} |")
    md += ["", "## Per run", "",
           "| run | cost ms (p10-p90) | fluid / cell ms | basis | forward ms | GPU frame ms complete/incomplete | "
           "frames complete/steady/total | clock MHz | temp C | valid |",
           "|---|---|---|---|---|---|---|---|---|---|"]
    for r in records:
        temp = ((r.get("clock") or {}).get("temp_c") or {}).get("median")
        band = r.get("fluid_ms_p10_p90")
        band_text = f" ({fmt(band[0], 3)}-{fmt(band[1], 3)})" if band else ""
        md.append(f"| {r['label']} | {fmt(r.get('fluid_ms'), 4)}{band_text} | "
                  f"{fmt(r.get('fluid_pass_ms'), 4)} / {fmt(r.get('cell_pass_ms'), 4)} | {r.get('basis', 'n/a')} | "
                  f"{fmt(r.get('forward_ms'), 3)} | "
                  f"{fmt(r.get('gpu_frame_ms_complete'), 3)}/{fmt(r.get('gpu_frame_ms_incomplete'), 3)} | "
                  f"{r.get('complete_frames', 'n/a')}/{r.get('steady_frames', 'n/a')}/{r.get('frames_precedent', 'n/a')} | "
                  f"{clock_text(r)} | {temp if temp is not None else 'n/a'} | {'yes' if r['valid'] else 'NO'} |")
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "## Invalid runs (rerun, do not report)", ""]
        for r in bad:
            md.append(f"- `{r['label']}`: " + "; ".join(r["problems"] or ["no GPU data"]))
    bad_stats = [s for s in srows if s["problems"]]
    if bad_stats:
        md += ["", "## Count captures with problems", ""]
        for s in bad_stats:
            md.append(f"- `{s['view']}`: " + "; ".join(s["problems"]))
    (HERE / f"{name}.md").write_text("\n".join(md) + "\n")
    return out


# --- main ------------------------------------------------------------------

def git(*a):
    r = subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or None


def engine_binary():
    default = ROOT / "target" / "release" / ("postretro.exe" if os.name == "nt" else "postretro")
    return Path(os.environ.get("CELL_BIN", default))


def untraced(arm, d, warmup, samples, count=False, env_extra=None, timeout=900):
    """One plain capture of `arm` in directory `d`: (ok, report or None, output tail)."""
    scene, png, report = d / "scene.json", d / "capture.png", d / "report.json"
    report.unlink(missing_ok=True)
    write_scene(scene, arm, png, report, warmup, samples, count=count)
    env = engine_env()
    env.update(env_extra or {})
    r = subprocess.run([str(engine_binary()), "--capture", str(scene)], cwd=ROOT, env=env,
                       capture_output=True, text=True, timeout=timeout)
    rep = json.loads(report.read_text()) if report.exists() else None
    return r.returncode == 0 and rep is not None, rep, (r.stdout + r.stderr)[-2000:]


def smoke(args):
    """Untraced 4-frame capture of a cell arm: proves the binary is the spike
    build (scenes are deny_unknown_fields), the .prl exists and capture works."""
    arm = next(a for a in arms() if a["name"] == "near-4k-cellcull-live")
    with tempfile.TemporaryDirectory() as d:
        ok, _, tail = untraced(arm, Path(d), 2, 2)
        if not ok:
            print("smoke capture failed:\n" + tail, file=sys.stderr)
        return ok


def count_captures(args):
    """Per view, one untraced capture with the cell buffer, its cull and the
    count lever: cells dispatched and kept, visible fluid pixels (occlusion
    query), atlas size. Never timed; the counters cost GPU time."""
    stats = {}
    for pose, res in views():
        arm = next(a for a in arms() if a["pose"] == pose and a["res"] == res and a["mode"] == "cellcull"
                   and a["ratio"] == "live")
        key = f"{pose}-{res}"
        with tempfile.TemporaryDirectory() as d:
            ok, rep, tail = untraced(arm, Path(d), 4, 4, count=True)
        entry = {"problems": []}
        if not ok:
            entry["problems"].append("count capture failed: " + tail[-400:])
        else:
            entry["fluid_cell_stats"] = rep.get("fluid_cell_stats")
            entry["adapter"] = rep.get("adapter")
            if not entry["fluid_cell_stats"]:
                entry["problems"].append("report has no fluid_cell_stats (cell buffer inactive?)")
        stats[key] = entry
        print(f"  count {key}: {json.dumps(entry.get('fluid_cell_stats'))}", flush=True)
    (RUNS / "count-stats.json").write_text(json.dumps(stats, indent=2) + "\n")
    return stats


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--only")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--skip-smoke", action="store_true")
    ap.add_argument("--skip-stats", action="store_true")
    ap.add_argument("--warmup", type=int, default=int(os.environ.get("CELL_WARMUP", "240")))
    ap.add_argument("--samples", type=int, default=int(os.environ.get("CELL_SAMPLES", "240")))
    args = ap.parse_args()

    all_arms = arms()
    if args.list:
        for a in all_arms:
            print(a["name"], a["mode"], a["ratio"], "x".join(map(str, RESOLUTIONS[a["res"]])), POSES[a["pose"]]["map"])
        return 0
    chosen = [a for a in all_arms if not args.only or a["name"] in args.only.split(",")]
    if not chosen:
        print("no arms match --only", file=sys.stderr)
        return 2
    RUNS.mkdir(exist_ok=True)
    trace_dir = Path(os.environ.get("TRACE_DIR", Path(tempfile.gettempdir()) / "cell-rate-spike-traces"))
    trace_dir.mkdir(parents=True, exist_ok=True)

    binary = engine_binary()
    if args.dry_run:
        for rnd in range(1, args.rounds + 1):
            order = chosen if rnd % 2 else list(reversed(chosen))
            for a in order:
                label = f"{a['name']}-r{rnd}"
                write_scene(RUNS / f"{label}.scene.json", a, RUNS / f"{label}.png", RUNS / f"{label}.report.json",
                            args.warmup, args.samples)
                print(label)
        return 0

    problems = []
    if not binary.is_file():
        problems.append(f"engine binary not found: {binary} (cargo build --release -p postretro --features capture,dev-tools)")
    for m in sorted({POSES[a["pose"]]["map"] for a in chosen}):
        if not (ROOT / m).is_file():
            problems.append(f"missing map: {m} (compile it with prl-build, see README.md)")
    if busy_processes():
        problems.append(f"another process is running: {busy_processes()}")
    if free_gb(trace_dir) < float(os.environ.get("MIN_FREE_GB", "25")):
        problems.append(f"less than {os.environ.get('MIN_FREE_GB', '25')} GB free in {trace_dir}")
    if subprocess.run(["xcrun", "--find", "xctrace"], capture_output=True).returncode != 0:
        problems.append("xctrace not found (install Xcode or the Command Line Tools)")
    if problems:
        print("\n".join("ABORT: " + p for p in problems), file=sys.stderr)
        return 3

    flags = xctrace_flags()
    caf = subprocess.Popen(["caffeinate", "-d", "-i", "-u", "-t", "28800"])
    awake = threading.Event()

    def redeclare():
        # Re-declare user activity every few seconds so the idle lock never arms.
        while not awake.is_set():
            subprocess.run(["caffeinate", "-u", "-t", "5"])

    threading.Thread(target=redeclare, daemon=True).start()

    def cleanup():
        awake.set()
        caf.terminate()
        shutil.rmtree(trace_dir, ignore_errors=True)

    atexit.register(cleanup)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(130))

    idle = idle_snapshot(RUNS / "idle.ioreg.txt")
    print("idle ioreg:", json.dumps(idle))
    if any(v > 10 for v in idle["Device Utilization %"]):
        print("WARNING: GPU utilisation is above 10% at idle; close other GPU apps (threshold is the kit's own, not a repo pin)",
              file=sys.stderr)
    meta = {"git_rev": git("rev-parse", "HEAD"), "git_branch": git("rev-parse", "--abbrev-ref", "HEAD"),
            "git_dirty": bool(git("status", "--porcelain")), "binary": str(binary),
            "binary_mtime": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(binary.stat().st_mtime)),
            "machine": " | ".join(filter(None, [
                subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
                subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip()])),
            "xctrace_flags": flags, "idle": idle, "warmup": args.warmup, "samples": args.samples,
            "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "adapter": None}
    if meta["git_branch"] != "claude/cell-rate-spike":
        print(f"WARNING: branch is {meta['git_branch']}, not claude/cell-rate-spike", file=sys.stderr)

    if not args.skip_smoke:
        print("smoke capture ...", flush=True)
        if not smoke(args):
            return 3
    stats = {}
    if not args.skip_stats:
        print("count captures ...", flush=True)
        stats = count_captures(args)
    # Leftovers of an earlier aborted batch live only in this tool's own trace dir.
    for old in trace_dir.iterdir():
        shutil.rmtree(old, ignore_errors=True) if old.is_dir() else old.unlink()

    records = []
    code = 0
    try:
        for rnd in range(1, args.rounds + 1):
            order = chosen if rnd % 2 else list(reversed(chosen))
            for a in order:
                busy = busy_processes()
                if busy:
                    print(f"ABORT: another process is running: {busy}", file=sys.stderr)
                    code = 4
                    raise StopIteration
                time.sleep(float(os.environ.get("IDLE_GAP", "15")))
                label = f"{a['name']}-r{rnd}"
                print(f"run {label} ...", flush=True)
                r = run_one(label, a, rnd, args, trace_dir, flags)
                records.append(r)
                if r.get("adapter") and not meta["adapter"]:
                    meta["adapter"] = r["adapter"]
                print(f"  cost {fmt(r.get('fluid_ms'), 4)} ms (fluid {fmt(r.get('fluid_pass_ms'), 4)} + cell "
                      f"{fmt(r.get('cell_pass_ms'), 4)}, {r.get('basis', 'n/a')}), clock {clock_text(r)} MHz, "
                      f"valid={r['valid']}" + ("" if r["valid"] else f" problems={r['problems']}"), flush=True)
                # A machine-state problem invalidates the rest of the interleave too.
                if not r["machine_state_ok"]:
                    print(f"ABORT: batch stopped at {label} (machine state)", file=sys.stderr)
                    code = 4
                    raise StopIteration
    except StopIteration:
        pass
    finally:
        if records or stats:
            summarize(records, meta, stats)
            print((HERE / "results.md").read_text())
    return code


if __name__ == "__main__":
    sys.exit(main())
