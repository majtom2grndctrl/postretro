#!/usr/bin/env python3
"""Q2 cost run: q2.py <scratch> <config>[,<config>...] [repeat]. Config: base, off, t4, t6, t8."""
import json, os, subprocess, sys

SP = sys.argv[1]
CONFIGS = sys.argv[2].split(",")
REPEAT = int(sys.argv[3]) if len(sys.argv) > 3 else 1
MAP = "C:/Users/danhi/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection.prl"
POS = [32.1056, 22.86, 53.2384]
VIEWS = {"a": (-135, -12), "b": (45, -12), "c": (-45, -12)}
OUT = os.path.join(SP, "q2")
os.makedirs(OUT, exist_ok=True)


def run(cfg, view, rep):
    yaw, pitch = VIEWS[view]
    name = f"{cfg}-{view}-{rep}"
    report = os.path.join(OUT, name + ".report.json")
    scene = {"map": MAP, "camera": {"position": POS, "yaw_deg": yaw, "pitch_deg": pitch, "fov_deg": 100},
             "resolution": [1920, 1080], "output": os.path.join(OUT, name + ".png"),
             "measurement": {"report": report, "warmup_frames": 240, "sample_frames": 600}}
    path = os.path.join(OUT, name + ".json")
    json.dump(scene, open(path, "w", encoding="utf-8"))
    env = dict(os.environ, POSTRETRO_SH_STREAMING="sync-proof", POSTRETRO_GPU_TIMING="1", RUST_LOG="info")
    if cfg[0] == "r":
        env["POSTRETRO_PDD_TILES"] = os.path.join(SP, "tiles", "hall.t" + cfg[1:] + ".bin")
    if cfg == "off":
        env["POSTRETRO_PDD_Q3_BITS"] = os.path.join(SP, "q3", "full.bits")
    if cfg[0] in "ts":
        env["POSTRETRO_PDD_Q2_T"] = cfg[1:]
    if cfg[0] == "s":
        env["POSTRETRO_PDD_Q2_SCATTER"] = "1"
    exe = os.path.join(SP, "q2bin-base" if cfg == "base" else "q2bin-spike", "postretro.exe")
    r = subprocess.run([exe, "--capture", path], env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=280)
    open(os.path.join(OUT, name + ".log"), "w", encoding="utf-8").write(r.stdout + r.stderr)
    if r.returncode:
        print("FAIL", name, r.returncode, r.stderr[-300:], flush=True)
        sys.exit(1)
    rep_json = json.load(open(report, encoding="utf-8"))
    g = rep_json["gpu_timing"]
    if g.get("availability") != "available":
        print("NO TIMING", name, g, flush=True)
        sys.exit(1)
    fw = []
    total = []
    for w in g["windows"]:
        passes = w.get("passes") or w.get("rows") or []
        for p in passes:
            label = p.get("label") or p.get("name")
            avg = p.get("avg_ms", p.get("average_ms"))
            if label == "forward":
                fw.append(avg)
        total.append(w)
    if not fw:
        print("window shape:", json.dumps(g["windows"][0])[:900], flush=True)
        sys.exit(1)
    fw.sort()
    med = fw[len(fw) // 2]
    print(f"{name}: forward median {med:.3f} ms over {len(fw)} windows (min {fw[0]:.3f} max {fw[-1]:.3f}), "
          f"frame cpu_completion median {rep_json['cpu_completion']['median_ms']:.3f} ms", flush=True)


for rep in range(1, REPEAT + 1):
    for view in VIEWS:
        for cfg in CONFIGS:
            run(cfg, view, rep)
