"""1660 Super runs for sh-compose-contributing-rows (old vs new membership).

Arm tokens: old = POSTRETRO_SPIKE_OLD_MEMBERSHIP=1; new = entry-carrying
membership; new-<arms> = new membership plus spike compose arms ('+' for ',').

One arm per launch (unpaired), GPU timestamps via POSTRETRO_GPU_TIMING=1.
Arms are interleaved round by round. Each run discards the first WARM
[gpu-timing] windows with numeric compose values, keeps the next KEEP, then
terminates the engine. nvidia-smi samples clocks through the kept windows.

usage: python run1660.py <out_dir> <rounds> <arm>[,<arm>...] <pose>[,<pose>...]
"""
import json, os, re, subprocess, sys, threading, time

ROOT = r"C:\Users\danhi\Projects\Personal\postretro"
EXE = os.path.join(ROOT, r"target\release\postretro.exe")
POSES = {
    "campaign": [r"content\dev\maps\campaign-test.prl"],
    "arena": [r"content\dev\maps\stress-warren-hallway-inspection.prl",
              "--start-pose=21.13,2.44,30.48,0,0"],
    "kinstation": [r"content\dev\maps\kinematic-platform.prl",
                   "--start-pose=-6.5,1.22,-27.94,0,0"],
}
WARM, KEEP = 3, 8
TIMEOUT_S = 240
IDLE_GAP_S = 10

GPU_RE = re.compile(r"\[gpu-timing\] (.*?) \(avg over (\d+) readbacks")
COUNTS_RE = re.compile(r"\[SH spike counts\] (.*)")
ARMS_RE = re.compile(r"\[SH spike\] compose arms: (\S+)")
MEMB_RE = re.compile(r"\[SH spike\] compose membership: (\S+)")


def parse_gpu(body):
    out = {}
    for part in body.split(" | "):
        m = re.match(r"(\S+) (n/a|[\d.]+)ms(?: \((\d+)/(\d+)\))?", part.strip())
        if not m:
            continue
        label, val, present, total = m.groups()
        out[label] = {"ms": None if val == "n/a" else float(val),
                      "present": int(present) if present else None}
    return out


def nvsmi_sampler(stop, samples):
    q = "clocks.gr,clocks.mem,pstate,temperature.gpu,power.draw,utilization.gpu"
    while not stop.is_set():
        try:
            line = subprocess.run(
                ["nvidia-smi", f"--query-gpu={q}", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=5).stdout.strip()
            samples.append([time.time()] + [s.strip() for s in line.split(",")])
        except Exception as e:  # noqa: BLE001
            samples.append([time.time(), f"error {e}"])
        stop.wait(1.0)


def one_run(out_dir, label, arm, pose):
    env = dict(os.environ)
    old = arm == "old"
    spike_arms = arm[4:] if arm.startswith("new-") else ""
    env.update({"RUST_LOG": "info", "POSTRETRO_GPU_TIMING": "1",
                "POSTRETRO_SPIKE_ARMS": spike_arms})
    env.pop("POSTRETRO_SPIKE_OLD_MEMBERSHIP", None)
    if old:
        env["POSTRETRO_SPIKE_OLD_MEMBERSHIP"] = "1"
    log_path = os.path.join(out_dir, f"{label}.log")
    rec = {"label": label, "arm": arm, "pose": pose, "args": POSES[pose],
           "start": time.time(), "windows": [], "counts": [], "arms_logged": None,
           "nvsmi": [], "valid": False, "reason": None, "membership_logged": None}
    proc = subprocess.Popen([EXE] + POSES[pose], cwd=ROOT, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, encoding="utf-8", errors="replace")
    stop = threading.Event()
    sampler = None
    numeric = 0
    deadline = time.time() + TIMEOUT_S
    with open(log_path, "w", encoding="utf-8") as log:
        for line in proc.stdout:
            log.write(line)
            if (m := ARMS_RE.search(line)) and rec["arms_logged"] is None:
                rec["arms_logged"] = m.group(1)
            if (m := MEMB_RE.search(line)) and rec["membership_logged"] is None:
                rec["membership_logged"] = m.group(1)
            if m := COUNTS_RE.search(line):
                rec["counts"].append({"t": time.time(), "line": m.group(1)})
            if m := GPU_RE.search(line):
                g = parse_gpu(m.group(1))
                sc = g.get("sh_compose", {}).get("ms")
                if sc is not None:
                    numeric += 1
                    if numeric == WARM + 1:
                        sampler = threading.Thread(target=nvsmi_sampler,
                                                   args=(stop, rec["nvsmi"]))
                        sampler.start()
                    if numeric > WARM:
                        rec["windows"].append({"t": time.time(),
                                               "readbacks": int(m.group(2)),
                                               "passes": g})
                    if numeric >= WARM + KEEP:
                        rec["valid"] = True
                        break
            if time.time() > deadline:
                rec["reason"] = "timeout"
                break
            if proc.poll() is not None:
                break
    stop.set()
    if sampler:
        sampler.join()
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            proc.kill()
    if not rec["valid"] and rec["reason"] is None:
        rec["reason"] = f"exited early (code {proc.returncode}), {numeric} windows"
    want_memb = "whole-domain" if old else "entry-carrying"
    want_arms = set(spike_arms.split(",")) if spike_arms else {"baseline"}
    # array-free implies scale-shared, so the logged list may be a superset.
    if rec["valid"] and (rec["membership_logged"] != want_memb or
                         not want_arms <= set((rec["arms_logged"] or "").split(","))):
        rec["valid"] = False
        rec["reason"] = f"arm mismatch: {rec['membership_logged']} / {rec['arms_logged']}"
    rec["end"] = time.time()
    with open(os.path.join(out_dir, f"{label}.run.json"), "w") as f:
        json.dump(rec, f, indent=1)
    return rec


def main():
    out_dir, rounds, arms, poses = sys.argv[1], int(sys.argv[2]), \
        sys.argv[3].split(","), sys.argv[4].split(",")
    os.makedirs(out_dir, exist_ok=True)
    for r in range(1, rounds + 1):
        for pose in poses:
            for arm in arms:
                time.sleep(IDLE_GAP_S)
                label = f"{pose}-{arm.replace('+', '_').replace(',', '_')}-r{r}"
                rec = one_run(out_dir, label, arm.replace("+", ","), pose)
                w = rec["windows"]
                sc = [x["passes"]["sh_compose"]["ms"] for x in w]
                b = [x["passes"].get("animated_direct_sh_compose", {}).get("ms") for x in w]
                print(f"{label}: valid={rec['valid']} {rec['reason'] or ''} "
                      f"ind={sc} B={b}", flush=True)


if __name__ == "__main__":
    main()
