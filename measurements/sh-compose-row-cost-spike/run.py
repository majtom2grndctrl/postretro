"""One foreground engine run for sh-compose-row-cost-spike (copied from shadow-fill-cost).

usage: run.py <label> <binary> <map.prl> [engine args...]
env:   RUN_WINDOWS (default 8) complete [CpuTiming] windows before stopping
       RUN_TRACE=1 record a Metal System Trace after RUN_TRACE_AFTER windows
       RUN_TRACE_SECONDS (default 4)
       RUN_FOREGROUND path to the compiled foreground helper
       POSTRETRO_SPIKE_ARMS passes through to the engine (recorded per run)
A run is also invalid if its composed rows change after the first
[SH spike counts] window: every later window must hold min == max rows and
the same per-level mix, per pass.
"""
import json, os, re, signal, subprocess, sys, threading, time
from pathlib import Path

root = Path(__file__).resolve().parents[2]
out = Path(__file__).resolve().parent / "runs"
out.mkdir(exist_ok=True)


def screen_locked():
    r = subprocess.run(["ioreg", "-n", "Root", "-d1", "-a"], capture_output=True, text=True)
    i = r.stdout.find("CGSSessionScreenIsLocked")
    return i >= 0 and "<true/>" in r.stdout[i:i + 80]

GPU_FIELDS = ("Core Clock(MHz)", "Memory Clock(MHz)", "Temperature(C)", "Total Power(W)",
              "GPU Activity(%)", "Fan Speed(RPM)")


def gpu_state():
    """One sample of the 5300M's (Navi 14) clocks, temperature and power."""
    r = subprocess.run(["ioreg", "-c", "AMDRadeonX6000_AMDNavi14GraphicsAccelerator", "-r", "-d1", "-w0"],
                       capture_output=True, text=True)
    sample = {}
    for field in GPU_FIELDS:
        m = re.search(r'"' + re.escape(field) + r'"=(\d+)', r.stdout)
        if m:
            sample[field] = int(m[1])
    return sample


gpu_samples, gpu_sampling = [], threading.Event()


def sample_gpu():
    while gpu_sampling.is_set():
        gpu_samples.append({"t": round(time.time() - started, 2), **gpu_state()})
        time.sleep(0.5)

label, binary, mapname, *engine_args = sys.argv[1:]
foreground = os.environ["RUN_FOREGROUND"]
windows_wanted = int(os.environ.get("RUN_WINDOWS", "8"))
tracing = os.environ.get("RUN_TRACE", "0") == "1"
trace_after = int(os.environ.get("RUN_TRACE_AFTER", "2"))
trace_seconds = os.environ.get("RUN_TRACE_SECONDS", "4")
trace_dir = Path(os.environ.get("RUN_TRACE_DIR", str(out)))
sampling = os.environ.get("RUN_SAMPLE", "0") == "1"

env = os.environ.copy()
env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
env.update(RUST_LOG="info", POSTRETRO_CPU_TIMING="1")
log = out / f"{label}.log"
status = out / f"{label}.trace-status.txt"
started = time.time()
with log.open("w") as f:
    p = subprocess.Popen([binary, str(root / "content/dev/maps" / mapname), *engine_args],
                         cwd=root, env=env, stdout=f, stderr=subprocess.STDOUT)
    print(f"engine_pid={p.pid}", flush=True)
    checks, activated, trace, paused, sampler = [], False, None, False, None
    try:
        while time.time() - started < 240 and p.poll() is None:
            content = log.read_text(errors="replace")
            windows = content.count("[CpuTiming]")
            action = "activate" if not activated and "Window ready" in content else "check"
            activated = activated or action == "activate"
            r = subprocess.run([foreground, str(p.pid), action], capture_output=True, text=True)
            saver = subprocess.run(["pgrep", "-f", "ScreenSaverEngine|legacyScreenSaver"],
                                   capture_output=True).returncode == 0
            checks.append({"elapsed": round(time.time() - started, 2), "windows": windows,
                           "foreground": "foreground=true" in r.stdout, "screensaver": saver,
                           "locked": screen_locked()})
            if sampling and sampler is None and windows >= 3:
                sampler = subprocess.Popen(["sample", str(p.pid), "10", "1", "-file",
                                            str(out / f"{label}.sample.txt")],
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if tracing and trace is None and windows >= trace_after:
                gpu_sampling.set()
                threading.Thread(target=sample_gpu, daemon=True).start()
                trace = subprocess.Popen(
                    ["xcrun", "xctrace", "record", "--template", "Metal System Trace",
                     "--attach", str(p.pid), "--time-limit", f"{trace_seconds}s",
                     "--output", str(trace_dir / f"{label}.trace")],
                    stdout=status.open("w"), stderr=subprocess.STDOUT)
            if trace is not None and not paused and "Reached specified time limit" in status.read_text():
                gpu_sampling.clear()
                p.send_signal(signal.SIGSTOP)
                paused = True
            done_windows = windows >= (trace_after if tracing else windows_wanted)
            sampled = not sampling or (sampler is not None and sampler.poll() is not None)
            if done_windows and sampled and (not tracing or (trace is not None and trace.poll() is not None)):
                break
            time.sleep(1)
    finally:
        if p.poll() is None:
            if paused:
                p.send_signal(signal.SIGCONT)
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()
        if trace is not None:
            trace.wait(timeout=60)
text = log.read_text(errors="replace")
windows_done = text.count("[CpuTiming]")
gpu_sampling.clear()


def median(values):
    values = sorted(values)
    return values[len(values) // 2] if values else None


gpu_summary = {field: median([s[field] for s in gpu_samples if field in s]) for field in GPU_FIELDS}
COUNTS = re.compile(r"\[SH spike counts\] (\S+): frames (\d+) rows min (\d+) max (\d+) \| last rows "
                    r"L0 (\d+) L1 (\d+) L2 (\d+) \| entries L0 (\d+) L1 (\d+) L2 (\d+)")
counts = {}
for m in COUNTS.finditer(text):
    counts.setdefault(m[1], []).append([int(x) for x in m.groups()[1:]])
rows_stable = True
row_mix = {}
for pass_name, windows in counts.items():
    later = windows[1:]
    if not later:
        rows_stable = False
        continue
    mixes = {tuple(w[3:]) for w in later}
    rows_stable &= all(w[1] == w[2] for w in later) and len(mixes) == 1
    row_mix[pass_name] = dict(zip(["rows_L0", "rows_L1", "rows_L2", "entries_L0", "entries_L1", "entries_L2"],
                                  later[-1][3:]))
arms_line = re.search(r"\[SH spike\] compose arms: (\S+)", text)
arms_b_line = re.search(r"\[SH spike\] paired B compose arms: (\S+)", text)
record = {"label": label, "binary": binary, "map": mapname, "engine_args": engine_args,
          "elapsed": round(time.time() - started, 2), "exit": p.returncode,
          "timing_windows": windows_done,
          "foreground_all": any(c["windows"] >= 1 for c in checks)
                            and all(c["foreground"] for c in checks if c["windows"] >= 1),
          "screensaver_seen": any(c["screensaver"] for c in checks),
          "locked_seen": any(c["locked"] for c in checks),
          "arms": arms_line[1] if arms_line else None,
          "arms_b": arms_b_line[1] if arms_b_line else None,
          "gpu_state_median": gpu_summary, "gpu_state_samples": gpu_samples,
          "rows_stable": rows_stable, "row_mix": row_mix, "spike_count_windows": counts,
          "foreground_checks": checks}
# A run counts only if it finished its windows in the foreground, unlocked, with
# no screen saver: an empty check list must never pass vacuously.
record["valid"] = (windows_done >= (trace_after if tracing else windows_wanted)
                   and record["foreground_all"] and not record["screensaver_seen"]
                   and not record["locked_seen"] and record["rows_stable"]
                   and record["arms"] is not None)
(out / f"{label}.run.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({k: v for k, v in record.items() if k != "foreground_checks"}), flush=True)
sys.exit(0 if record["valid"] else 3)
