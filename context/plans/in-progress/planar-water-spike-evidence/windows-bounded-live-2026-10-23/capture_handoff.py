"""Static handoff illustrations; run after the live matrix, never alongside it."""
import argparse
import json
import os
from pathlib import Path
import subprocess

from run import configurations

parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent / "handoff-images")
parser.add_argument("--only", nargs="+")
args = parser.parse_args()
root = args.output.resolve()
assert not root.exists(), "Use a fresh output directory"
root.mkdir()
binary = Path("target/release/postretro.exe").resolve()
env = os.environ.copy()
env.update(WGPU_BACKEND="vulkan", POSTRETRO_SH_STREAMING="sync-proof", RUST_LOG="info", POSTRETRO_PLANAR_REFLECTION="0", POSTRETRO_CPU_TIMING="1", POSTRETRO_GPU_TIMING="1")
env.pop("POSTRETRO_UNCAPPED", None)
env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
temporary = Path("target/planar-bake-tmp").resolve()
env.update(TEMP=str(temporary), TMP=str(temporary))
rows = [(name, settings, "high") for name, settings in configurations() if name in ["ssr-high", "half-unlimited", "half-d30", "half-d15", "quarter-d15"]]
rows.append(("probes-only", dict(on=False, scale=0.5, d=None, r=None, band_d=4, band_r=4), "low"))
for name, settings, quality in rows:
    if args.only and name not in args.only:
        continue
    scene = dict(map="content/dev/maps/reflection-lake-test.prl", camera=dict(position=[-16.8656, 1.5, -48.768], yaw_deg=90, pitch_deg=-10), resolution=[1600, 900], reflection_quality=quality, planar_reflection=settings, output=str(root / (name + ".png")), measurement=dict(report=str(root / (name + ".json")), warmup_frames=120, sample_frames=120))
    scene_path = root / (name + "-scene.json")
    scene_path.write_text(json.dumps(scene, indent=2) + "\n")
    with (root / (name + ".log")).open("w") as log:
        subprocess.run([str(binary), "--capture", str(scene_path)], env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    report = json.loads((root / (name + ".json")).read_text())
    log = (root / (name + ".log")).read_text()
    assert "SolidCellFallback" not in log and "Validation Error" not in log
    assert report["gpu_timing"]["availability"] == "available"
    print(name, "captured", flush=True)
