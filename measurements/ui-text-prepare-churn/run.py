"""One foreground timing run of a before/after engine binary (ui-text-prepare-churn M1/M2).

Adapted from measurements/release-indirect-validation/runtime/run-runtime.py.

usage: python3 run.py <label> <bin_dir> <worktree_root> <map.prl> [--sample]

- <bin_dir> holds `postretro` and `scripts-build` copied out of that commit's target.
- The engine runs with cwd = <worktree_root>, so core/ and content/ are that commit's.
- Records machine state (ioreg idle snapshot, pmset therm) before launch, polls the
  GPU clock/temperature during the run, and stops after eight [CpuTiming] windows
  (or after the 10 s `sample` with --sample, which starts after two windows).
"""
import json, os, re, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "runs"
FOREGROUND = os.environ.get("FOREGROUND_BIN", "foreground")
LOCKSTATE = os.environ.get("LOCKSTATE_BIN", "lockstate")

label, bin_dir, wt, mapname = sys.argv[1:5]
do_sample = "--sample" in sys.argv[5:]
OUT.mkdir(exist_ok=True)


def sh(cmd):
    return subprocess.run(cmd, capture_output=True, text=True).stdout


def gpu_stats():
    raw = sh(["ioreg", "-r", "-c", "IOAccelerator"])
    m = re.search(r'"PerformanceStatistics" = \{([^}]*)\}', raw)
    out = {}
    if m:
        for k in ["Temperature(C)", "Core Clock(MHz)", "Device Utilization %", "inUseVidMemoryBytes", "vramFreeBytes"]:
            mm = re.search(r'"' + re.escape(k) + r'"=(\d+)', m[1])
            if mm:
                out[k] = int(mm[1])
    return out


def locked():
    return "locked=1" in sh([LOCKSTATE])


def therm():
    t = sh(["pmset", "-g", "therm"])
    m = re.search(r"CPU_Speed_Limit\s*=\s*(\d+)", t)
    return (int(m[1]) if m else None), t


if locked():
    print(json.dumps({"label": label, "invalid": "screen locked before launch"}))
    sys.exit(3)
(OUT / f"{label}.idle.ioreg.txt").write_text(sh(["ioreg", "-c", "IOAccelerator", "-l", "-w0"]))
limit_before, therm_txt = therm()
(OUT / f"{label}.therm.txt").write_text(therm_txt)
idle = gpu_stats()

env = os.environ.copy()
for k in ["WGPU_VALIDATION_INDIRECT_CALL", "POSTRETRO_GPU_TIMING"]:
    env.pop(k, None)
env.update(RUST_LOG="info", POSTRETRO_CPU_TIMING="1")
log = OUT / f"{label}.log"
started = time.time()
checks, polls = [], []
sampler = None
activated = False
invalid = None
lost_focus = []
with log.open("w") as f:
    p = subprocess.Popen([str(Path(bin_dir) / "postretro"), str(Path(wt) / "content/dev/maps" / mapname)],
                         cwd=wt, env=env, stdout=f, stderr=subprocess.STDOUT)
    try:
        while p.poll() is None:
            el = time.time() - started
            content = log.read_text(errors="replace")
            windows = content.count("[CpuTiming]")
            action = "activate" if not activated and "Window ready" in content else "check"
            if action == "activate":
                activated = True
            r = subprocess.run([FOREGROUND, str(p.pid)], capture_output=True, text=True)
            fg = "foreground=true" in r.stdout
            if action == "activate" or (activated and not fg):
                # NSRunningApplication.activate is refused under macOS cooperative
                # activation; System Events can still raise the engine's window.
                subprocess.run(["osascript", "-e", f'tell application "System Events" to set frontmost of (first process whose unix id is {p.pid}) to true'],
                               capture_output=True, text=True)
                if activated and not fg and windows >= 1:
                    lost_focus.append(round(el, 2))
            checks.append({"elapsed": round(el, 2), "windows": windows, "foreground": fg})
            polls.append({"elapsed": round(el, 2), **gpu_stats(), "cpu_speed_limit": therm()[0]})
            if locked():
                invalid = f"screen locked at {el:.0f} s (windows={windows})"
                break
            if windows == 0 and el > 240:
                invalid = "no timing window within 240 s (stalled at Window ready?)" if "Window ready" in content else "no Window ready within 240 s"
                break
            if do_sample and windows >= 2 and sampler is None:
                sampler = subprocess.Popen(["sample", str(p.pid), "10", "1", "-file", str(OUT / f"{label}.sample.txt")],
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if windows >= 8 and (sampler is None or sampler.poll() is not None):
                break
            time.sleep(1)
    finally:
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()
        if sampler is not None:
            sampler.wait(timeout=30)

text = log.read_text(errors="replace")
if invalid is None and lost_focus:
    invalid = f"engine lost foreground during timing at {lost_focus}"
if invalid is None and p.returncode not in (-15, 0, None) and text.count("[CpuTiming]") < 8:
    invalid = f"engine exited {p.returncode} before 8 windows"
loaded_map = re.findall(r"[^\s/]+\.prl", text)
record = {
    "label": label, "bin_dir": bin_dir, "worktree": wt, "map": mapname, "sample": do_sample,
    "elapsed": round(time.time() - started, 2), "exit": p.returncode,
    "timing_windows": text.count("[CpuTiming]"), "invalid": invalid,
    "map_mentions": sorted(set(loaded_map))[:5],
    "foreground_all_true_after_activate": all(c["foreground"] for c in checks if c["windows"] >= 1),
    "cpu_speed_limit_before": limit_before, "gpu_idle_before": idle,
    "gpu_polls": polls, "foreground_checks": checks,
}
(OUT / f"{label}.run.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({k: v for k, v in record.items() if k not in ("gpu_polls", "foreground_checks")}), flush=True)
