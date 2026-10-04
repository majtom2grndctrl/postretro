"""One foreground engine run for shadow-fill-cost manual rows M1-M3.

usage: run.py <label> <binary> <map.prl> [engine args...]
env:   RUN_WINDOWS (default 8) complete [CpuTiming] windows before stopping
       RUN_TRACE=1 record a Metal System Trace after RUN_TRACE_AFTER windows
       RUN_TRACE_SECONDS (default 4)
       RUN_FOREGROUND path to the compiled foreground helper
"""
import json, os, re, signal, subprocess, sys, time
from pathlib import Path

root = Path(__file__).resolve().parents[2]
out = Path(__file__).resolve().parent / "runs"
out.mkdir(exist_ok=True)
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
                           "foreground": "foreground=true" in r.stdout, "screensaver": saver})
            if sampling and sampler is None and windows >= 3:
                sampler = subprocess.Popen(["sample", str(p.pid), "10", "1", "-file",
                                            str(out / f"{label}.sample.txt")],
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if tracing and trace is None and windows >= trace_after:
                trace = subprocess.Popen(
                    ["xcrun", "xctrace", "record", "--template", "Metal System Trace",
                     "--attach", str(p.pid), "--time-limit", f"{trace_seconds}s",
                     "--output", str(trace_dir / f"{label}.trace")],
                    stdout=status.open("w"), stderr=subprocess.STDOUT)
            if trace is not None and not paused and "Reached specified time limit" in status.read_text():
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
record = {"label": label, "binary": binary, "map": mapname, "engine_args": engine_args,
          "elapsed": round(time.time() - started, 2), "exit": p.returncode,
          "timing_windows": log.read_text(errors="replace").count("[CpuTiming]"),
          "foreground_all": all(c["foreground"] for c in checks if c["windows"] >= 1),
          "screensaver_seen": any(c["screensaver"] for c in checks),
          "foreground_checks": checks}
(out / f"{label}.run.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({k: v for k, v in record.items() if k != "foreground_checks"}), flush=True)
