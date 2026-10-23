"""Static stills per walk-budget configuration; run after the live matrix, never alongside it.

Same pattern as windows-bounded-live-2026-10-23/capture_handoff.py: fixed eye
y=1.5, sync-proof preload, GPU timing on, 120 warmup + 120 measured frames.
The walk budget reaches capture through the same launch seed as live runs.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

from run import configurations

POSES = {
    "across": dict(position=[-16.8656, 1.5, -48.768], yaw_deg=90, pitch_deg=-10),
    # knob-sweep README down pose: 12 m over the lake.
    "down": dict(position=[-28.8656, 1.7, -48.768], yaw_deg=90, pitch_deg=-55),
}

parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--only", nargs="+")
parser.add_argument("--pose", choices=sorted(POSES), default="across")
args = parser.parse_args()
root = args.output.resolve()
assert not root.exists(), "Use a fresh output directory"
root.mkdir(parents=True)
binary = Path("target/release/postretro.exe").resolve()
env = os.environ.copy()
env.update(WGPU_BACKEND="vulkan", POSTRETRO_SH_STREAMING="sync-proof", RUST_LOG="info", POSTRETRO_PLANAR_REFLECTION="0", POSTRETRO_CPU_TIMING="1", POSTRETRO_GPU_TIMING="1")
for key in ["POSTRETRO_UNCAPPED", "WGPU_VALIDATION_INDIRECT_CALL", "POSTRETRO_PLANAR_WALK_STEPS"]:
    env.pop(key, None)
temporary = Path("target/planar-bake-tmp").resolve()
env.update(TEMP=str(temporary), TMP=str(temporary))
rows = [(name, settings) for name, settings in configurations()]
rows.append(("probes-only", dict(on=False, scale=0.5, d=None, r=None, band_d=4, band_r=4, quality="low", walk=None)))
summary = {}
for name, settings in rows:
    if args.only and name not in args.only:
        continue
    planar = {k: settings[k] for k in ["on", "scale", "d", "r", "band_d", "band_r"]}
    scene = dict(map="content/dev/maps/reflection-lake-test.prl", camera=POSES[args.pose], resolution=[1600, 900], reflection_quality=settings["quality"], planar_reflection=planar, output=str(root / (name + ".png")), measurement=dict(report=str(root / (name + ".json")), warmup_frames=120, sample_frames=120))
    scene_path = root / (name + "-scene.json")
    scene_path.write_text(json.dumps(scene, indent=2) + "\n")
    run_env = dict(env)
    if settings["walk"] is not None:
        run_env["POSTRETRO_PLANAR_WALK_STEPS"] = str(settings["walk"])
    with (root / (name + ".log")).open("w") as log:
        subprocess.run([str(binary), "--capture", str(scene_path)], env=run_env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    report = json.loads((root / (name + ".json")).read_text())
    log = (root / (name + ".log")).read_text(errors="replace")
    assert "SolidCellFallback" not in log and "Validation Error" not in log and "invalid POSTRETRO_" not in log
    assert report["gpu_timing"]["availability"] == "available"
    walk = re.findall(r"Planar mirror walk steps: (\d+)", log)
    assert walk == [str(20000 if settings["walk"] is None else settings["walk"])], (name, walk)
    mirror = re.findall(r"\[Planar reflection\] (plane=.*)$", log, re.M)
    assert bool(mirror) == settings["on"], name
    summary[name] = dict(walk_steps=int(walk[0]), mirror=mirror)
    print(name, "captured", mirror[-1] if mirror else "", flush=True)
(root / "capture-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
