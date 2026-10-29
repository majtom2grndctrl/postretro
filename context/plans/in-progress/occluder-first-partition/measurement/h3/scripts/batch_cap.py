"""H3 offline GPU batch: 4K static captures at every H3 pose on before / cap19 / cap26.

For rep 1..3, for each map and pose, the three sides run back to back in a rotating
order (rep 1 before-cap19-cap26, rep 2 cap19-cap26-before, ...) so slow clock or
thermal drift spreads over the sides. Each run is run_cap.sh. A run is discarded and
repeated (up to 3 attempts) when the screen saver was up, the engine failed, or it
landed in the slowdown regime: median sample > 120 ms, or over 5% of samples > 150 ms.
Every attempt is logged to batch_cap.status with its verdict.
usage: python3 batch_cap.py [map ...]
"""
import json, os, subprocess, sys, time

S = "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3"
W = "/Users/dhiester/Projects/Personal/postretro"
N = f"{W}/context/plans/in-progress/occluder-first-partition/measurement/h3/poses"
MAPS = sys.argv[1:] or ["occlusion-test", "stress-street", "campaign-test", "stress-warren-showcase", "stress-warren-hallway-inspection"]
SIDES = ["before", "cap19", "cap26"]

def prl(side, m):
    if side == "before":
        return f"{W}/target/occluder-baselines/{m}/{m}.prl", f"{W}/target/occluder-baselines/baked"
    if m == "stress-warren-hallway-inspection":
        return f"{W}/target/occluder-h3/{side}/{m}/{m}.prl", f"{W}/target/occluder-9g/baked"
    return f"{W}/target/occluder-9g/{side}/{m}/{m}.prl", f"{W}/target/occluder-9g/baked"

def poses(m):
    for line in open(f"{N}/{m}.poses"):
        if line.startswith("pose "):
            f = line.split()
            yield f[1], f[2:7]

def safe(label):
    return label.replace(":", "_").replace("/", "_").replace("@", "_")

def log(msg):
    with open(f"{S}/batch_cap.status", "a") as fh:
        fh.write(time.strftime("%H:%M:%S ") + msg + "\n")

def verdict(tag, rc):
    if rc == 2:
        return "SCREENSAVER"
    try:
        d = json.load(open(f"{S}/capruns/{tag}.report.json"))
    except Exception:
        return "NO_REPORT"
    c = d["cpu_completion"]; s = c["samples_ms"]
    slow = sum(1 for x in s if x > 150.0) / max(len(s), 1)
    if c["median_ms"] > 120.0 or slow > 0.05:
        return f"REGIME median={c['median_ms']:.1f} slow_frac={slow:.3f}"
    return f"OK median={c['median_ms']:.2f} p95={c['p95_ms']:.2f}"

for rep in (1, 2, 3):
    order = SIDES[rep - 1:] + SIDES[:rep - 1]
    for m in MAPS:
        for label, p in poses(m):
            for side in order:
                path, baked = prl(side, m)
                if not os.path.exists(path):
                    log(f"SKIP {m} {side} (no PRL)")
                    continue
                for attempt in range(1, 4):
                    tag = f"{m}.{safe(label)}.{side}.r{rep}" + ("" if attempt == 1 else f".a{attempt}")
                    if os.path.exists(f"{S}/capruns/{tag}.report.json") and verdict(tag, 0).startswith("OK"):
                        break
                    rc = subprocess.call([f"{S}/run_cap.sh", tag, path, baked] + p)
                    v = verdict(tag, rc)
                    log(f"{tag} {v}")
                    if v.startswith("OK"):
                        break
                    time.sleep(5)
    log(f"rep {rep} done")
log("CAP_BATCH_DONE")
