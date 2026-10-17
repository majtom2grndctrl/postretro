"""Metal System Trace batch for the cell-rate spike (Mac).

usage: python3 measurements/cell-rate-spike/run.py [options]
  --rounds N        interleaved rounds (default 2: arm order forward, then reversed)
  --only a,b        run only these arms (names as in --list)
  --list            print the arms and exit
  --dry-run         write the scene files and print the plan; run nothing
  --skip-smoke      skip the untraced 4-frame smoke capture that opens the batch
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
claude/probe-streaming-brief-364a5e). Changes: the arms (pose x resolution x
cell mode x ratio), the scene lever `fluid_reflection_cells`, a per-config
resolution, and the cost figure: Fluid Surface Pass + Fluid Reflection Cell
Pass per complete frame (gpu_time.py `cost`). That kit was adapted from
measurements/shadow-fill-cost/{run.py,clean.sh} and
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
at the config's resolution, POSTRETRO_SH_STREAMING=sync-proof, RUST_LOG=info. The scene file
is written to runs/<label>.scene.json. Results: results.json and results.md.
"""
import argparse, atexit, collections, gzip, json, os, re, shutil, signal, subprocess, sys
import tempfile, threading, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from gpu_time import reduce_export, FLUID_LABEL, CELL_LABEL, FORWARD_LABEL  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RUNS = HERE / "runs"
RAW = HERE / "raw"

UHD = [3840, 2160]
FHD = [1920, 1080]
FOV = 100.0
# Engine coordinates (x = -map y, y = map z, z = -map x, metres), poses from the
# spike prompt; the far pose is chosen in the findings note.
TEST_MAP = "content/dev/maps/reflection-probes-test.prl"
E24_MAP = "content/dev/maps/e24-environment-test.prl"
POSES = {
    "near": {"map": E24_MAP, "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -89.0},
    "test": {"map": TEST_MAP, "position": [-6.5024, 1.6, -6.5024], "yaw": 0.0, "pitch": -89.0},
    "far": json.loads(os.environ["CELL_FAR_POSE"]) if os.environ.get("CELL_FAR_POSE") else
           # e24's fluid-motion wing from its west end, 4 m up, looking east along
           # the pools: still pool out of frame below, current pool (scale 1) and
           # fine pool (scale 0.25) at about 8-23 m, partial-push pool at 7-14 m.
           {"map": E24_MAP, "position": [-4.877, 4.0, -40.64], "yaw": 0.0, "pitch": 5.0},
}
# (config, pose, resolution): 4K everywhere, 1080p for near and far.
CONFIGS = (("near-4k", "near", UHD), ("near-1080", "near", FHD), ("test-4k", "test", UHD),
           ("far-4k", "far", UHD), ("far-1080", "far", FHD))
# Cell mode -> scene lever; frag is today's per-fragment pixel style.
MODES = {"frag": {"cells": False, "frustum_cull": False},
         "cells": {"cells": True, "frustum_cull": False},
         "cull": {"cells": True, "frustum_cull": True}}
RATIOS = {"": False, "r1": True}
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


def arms():
    out = []
    for config, pose, resolution in CONFIGS:
        for ratio, ratio_one in RATIOS.items():
            for mode in MODES:
                name = f"{config}-{mode}" + (f"-{ratio}" if ratio else "")
                out.append({"name": name, "config": config, "pose": pose, "resolution": resolution,
                            "mode": mode, "ratio_one": ratio_one})
    return out


def levers(arm):
    return {**MODES[arm["mode"]], "ratio_one": arm["ratio_one"]}


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

def write_scene(path, arm, png, report, warmup, samples, mapname=None, quick=False):
    pose = POSES[arm["pose"]]
    scene = {
        "map": mapname or pose["map"],
        "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                   "pitch_deg": pose["pitch"], "fov_deg": FOV},
        "resolution": arm["resolution"],
        "output": str(png),
        "measurement": {"report": str(report), "warmup_frames": warmup, "sample_frames": samples},
        "fluid_reflection_cells": levers(arm),
    }
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
    cmd += ["--launch", "--", str(Path(os.environ.get("CELL_BIN", ROOT / "target/release/postretro"))),
            "--capture", str(scene_path)]

    record = {"label": label, "arm": arm["name"], "round": round_i, "config": arm["config"],
              "pose": arm["pose"], "mode": arm["mode"], "ratio_one": arm["ratio_one"],
              "map": POSES[arm["pose"]]["map"], "camera": POSES[arm["pose"]], "warmup_frames": args.warmup,
              "sample_frames": args.samples, "resolution": arm["resolution"], "command": cmd,
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
                record["gpu"] = reduce_export(str(xml), args.warmup, args.samples, arm["mode"] != "frag")
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
            cell = complete["passes"].get(CELL_LABEL)
            record["cell_ms"] = cell["per_frame_ms"] if cell else 0.0
            record["cost_ms"] = complete["cost"]["per_frame_ms"]
            record["cost_ms_p10_p90"] = [complete["cost"]["p10_ms"], complete["cost"]["p90_ms"]]
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
        cell_all = gpu["reported"][CELL_LABEL]["encoders"]
        if arm["mode"] != "frag" and not cell_all:
            record["problems"].append("cell arm recorded no Fluid Reflection Cell Pass encoders")
        if arm["mode"] == "frag" and cell_all:
            record["problems"].append("per-fragment arm recorded Fluid Reflection Cell Pass encoders")
        if record.get("basis") != "complete":
            record["problems"].append("cost needs the complete-frame basis (fluid + cell per frame)")
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


def summarize(records, meta):
    by_arm = collections.defaultdict(list)
    for r in records:
        by_arm[r["arm"]].append(r)
    arm_rows = []
    frag = {}
    mean = lambda xs: sum(xs) / len(xs) if xs else None
    for name, rs in by_arm.items():
        good = [r for r in rs if r["valid"] and r.get("cost_ms") is not None]
        costs = [r["cost_ms"] for r in good]
        row = {"arm": name, "config": rs[0]["config"], "pose": rs[0]["pose"], "mode": rs[0]["mode"],
               "ratio_one": rs[0]["ratio_one"], "resolution": rs[0]["resolution"], "runs": len(rs),
               "valid_runs": len(good), "cost_ms_mean": mean(costs),
               "cost_ms_spread": (max(costs) - min(costs)) if len(costs) > 1 else None,
               "fluid_ms_mean": mean([r["fluid_ms"] for r in good]),
               "cell_ms_mean": mean([r.get("cell_ms", 0.0) for r in good]),
               "forward_ms_mean": mean([r["forward_ms"] for r in good if r.get("forward_ms") is not None])}
        arm_rows.append(row)
        if rs[0]["mode"] == "frag":
            frag[(rs[0]["config"], rs[0]["ratio_one"])] = row["cost_ms_mean"]
    for row in arm_rows:
        base = frag.get((row["config"], row["ratio_one"]))
        c = row["cost_ms_mean"]
        row["vs_frag_ms"] = (c - base) if c is not None and base is not None else None
        row["vs_frag_ratio"] = (c / base) if c is not None and base else None

    out = {"meta": meta, "runs": [{k: v for k, v in r.items() if k not in ("gpu_state_samples", "command")}
                                  for r in records], "arms": arm_rows}
    (HERE / "results.json").write_text(json.dumps(out, indent=2) + "\n")

    md = ["# Cell-rate spike: Mac Metal System Trace results", "",
          f"- Commit: `{meta.get('git_rev')}` on `{meta.get('git_branch')}`; engine binary `{meta.get('binary')}`",
          f"- Machine: {meta.get('machine')}",
          f"- Adapter (from the capture reports): {meta.get('adapter')}",
          f"- Capture: warmup {meta['warmup']} + {meta['samples']} frames per run, POSTRETRO_SH_STREAMING=sync-proof",
          f"- Poses: {json.dumps(POSES)}",
          f"- Idle before the batch (ioreg IOAccelerator, one value per accelerator): {json.dumps(meta.get('idle'))}",
          "- `cost ms` is Fluid Surface Pass + Fluid Reflection Cell Pass GPU time per frame, averaged over "
          "complete steady frames (the last `samples` frames framed by the Candidate Cull Pass that hold exactly "
          "one depth, forward and fluid encoder, and exactly one cell-pass encoder in cell arms, none in frag arms). "
          "`GPU frame` is the label-independent union of every labelled interval per frame over complete / "
          "incomplete frames. `clock` is the median core clock over busy samples, with min-max. Modes: `frag` "
          "per-fragment pixel style; `cells` cell buffer; `cull` cell buffer with per-cell frustum cull; `-r1` "
          "the live-lighting ratio forced to 1 in both paths.", "",
          "## Per run", "",
          "| arm | run | cost ms (p10-p90) | fluid ms | cell ms | forward ms | GPU frame ms complete/incomplete | "
          "frames complete/steady/total | clock MHz | temp C | valid |",
          "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in records:
        temp = ((r.get("clock") or {}).get("temp_c") or {}).get("median")
        band = r.get("cost_ms_p10_p90")
        band_text = f" ({fmt(band[0], 3)}-{fmt(band[1], 3)})" if band else ""
        md.append(f"| {r['arm']} | {r['round']} | {fmt(r.get('cost_ms'), 4)}{band_text} | "
                  f"{fmt(r.get('fluid_ms'), 4)} | {fmt(r.get('cell_ms'), 4)} | {fmt(r.get('forward_ms'), 3)} | "
                  f"{fmt(r.get('gpu_frame_ms_complete'), 3)}/{fmt(r.get('gpu_frame_ms_incomplete'), 3)} | "
                  f"{r.get('complete_frames', 'n/a')}/{r.get('steady_frames', 'n/a')}/{r.get('frames_precedent', 'n/a')} | "
                  f"{clock_text(r)} | "
                  f"{temp if temp is not None else 'n/a'} | {'yes' if r['valid'] else 'NO'} |")
    md += ["", "## Per arm (valid runs)", "",
           "| arm | cost ms mean | run spread ms | fluid ms | cell ms | vs frag ms | vs frag x | forward ms | valid/total |",
           "|---|---|---|---|---|---|---|---|---|"]
    for row in arm_rows:
        md.append(f"| {row['arm']} | {fmt(row['cost_ms_mean'], 4)} | {fmt(row['cost_ms_spread'], 4)} | "
                  f"{fmt(row['fluid_ms_mean'], 4)} | {fmt(row['cell_ms_mean'], 4)} | "
                  f"{fmt(row['vs_frag_ms'], 4)} | {fmt(row['vs_frag_ratio'], 3)} | "
                  f"{fmt(row['forward_ms_mean'], 3)} | {row['valid_runs']}/{row['runs']} |")
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "## Invalid runs (rerun, do not report)", ""]
        for r in bad:
            md.append(f"- `{r['label']}`: " + "; ".join(r["problems"] or ["no GPU data"]))
    (HERE / "results.md").write_text("\n".join(md) + "\n")
    return out


# --- main ------------------------------------------------------------------

def git(*a):
    r = subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or None


def smoke(args):
    """Untraced 4-frame capture of the first arm: proves the binary is the spike
    build (scenes are deny_unknown_fields), the .prl exists and capture works."""
    arm = next(a for a in arms() if a["mode"] == "cull")
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        write_scene(d / "scene.json", arm, d / "smoke.png", d / "smoke.report.json", 2, 2)
        r = subprocess.run([str(Path(os.environ.get("CELL_BIN", ROOT / "target/release/postretro"))),
                            "--capture", str(d / "scene.json")],
                           cwd=ROOT, env=engine_env(), capture_output=True, text=True, timeout=600)
        ok = r.returncode == 0 and (d / "smoke.report.json").exists()
        if not ok:
            print("smoke capture failed:\n" + (r.stdout + r.stderr)[-2000:], file=sys.stderr)
        return ok


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--only")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--skip-smoke", action="store_true")
    ap.add_argument("--warmup", type=int, default=int(os.environ.get("CELL_WARMUP", "240")))
    ap.add_argument("--samples", type=int, default=int(os.environ.get("CELL_SAMPLES", "240")))
    args = ap.parse_args()

    all_arms = arms()
    if args.list:
        for a in all_arms:
            print(a["name"], a["config"], a["mode"], "ratio=1" if a["ratio_one"] else "ratio", POSES[a["pose"]]["map"])
        return 0
    chosen = [a for a in all_arms if not args.only or a["name"] in args.only.split(",")]
    if not chosen:
        print("no arms match --only", file=sys.stderr)
        return 2
    RUNS.mkdir(exist_ok=True)
    trace_dir = Path(os.environ.get("TRACE_DIR", Path(tempfile.gettempdir()) / "cell-rate-spike-traces"))
    trace_dir.mkdir(parents=True, exist_ok=True)

    binary = Path(os.environ.get("CELL_BIN", ROOT / "target/release/postretro"))
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
    caf = subprocess.Popen(["caffeinate", "-d", "-i", "-u", "-t", "14400"])
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
                print(f"  cost {fmt(r.get('cost_ms'), 4)} ms (fluid {fmt(r.get('fluid_ms'), 4)}, cell {fmt(r.get('cell_ms'), 4)}; "
                      f"{r.get('basis', 'n/a')}), clock {clock_text(r)} MHz, "
                      f"valid={r['valid']}" + ("" if r["valid"] else f" problems={r['problems']}"), flush=True)
                # A machine-state problem invalidates the rest of the interleave too.
                if not r["machine_state_ok"]:
                    print(f"ABORT: batch stopped at {label} (machine state)", file=sys.stderr)
                    code = 4
                    raise StopIteration
    except StopIteration:
        pass
    finally:
        if records:
            summarize(records, meta)
            print((HERE / "results.md").read_text())
    return code


if __name__ == "__main__":
    sys.exit(main())
