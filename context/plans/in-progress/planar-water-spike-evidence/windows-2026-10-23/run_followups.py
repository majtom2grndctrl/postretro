"""Supplement the portable lake sweep. Run from the spike workspace root."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess


def arms(suite):
    base = dict(on=True, scale=0.5, d=None, r=None, band_d=4, band_r=4)
    lake = ("content/dev/maps/reflection-lake-test.prl", [-16.8656, 1.7, -48.768], 90, -10)
    controls = [("ssr-high", "high", dict(base, on=False)),
                ("ssr-medium", "medium", dict(base, on=False)),
                ("planar-half", "high", base)]
    rows = []
    if suite == "bounds":
        for pose, pos, pitch in [("across", lake[1], -10), ("down", [-28.8656, 1.7, -48.768], -55)]:
            for d in [45, 60]:
                rows.append((f"{pose}-d{d}", (lake[0], pos, 90, pitch), [960, 540], "high", dict(base, d=d)))
    elif suite == "long-bounds":
        for d in [80, 100]:
            rows.append((f"across-d{d}", lake, [960, 540], "high", dict(base, d=d)))
    elif suite == "resolution":
        for resolution in [[1920, 1080], [3840, 2160]]:
            for name, quality, settings in controls + [("planar-quarter", "high", dict(base, scale=0.25)), ("planar-full", "high", dict(base, scale=1))]:
                rows.append((f"{resolution[0]}-{name}", lake, resolution, quality, settings))
    elif suite == "fixtures":
        poses = [
            ("e24-across", "e24-environment-test", [-1.016, 1.6, -44.2976], 90, -20),
            ("e24-down", "e24-environment-test", [-4.8768, 1.6, -44.2976], 0, -89),
            ("probes-across", "reflection-probes-test", [-2.6416, 1.6, -25.4], 40, -20),
            ("probes-down", "reflection-probes-test", [-6.5024, 1.6, -6.5024], 0, -89)]
        for pose, map_name, position, yaw, pitch in poses:
            for name, quality, settings in controls:
                rows.append((f"{pose}-{name}", (f"content/dev/maps/{map_name}.prl", position, yaw, pitch), [960, 540], quality, settings))
    elif suite == "coverage":
        down = (lake[0], [-28.8656, 1.7, -48.768], 90, -55)
        for name, quality, settings in controls + [("planar-quarter", "high", dict(base, scale=0.25)), ("planar-full", "high", dict(base, scale=1))]:
            rows.append((f"down-4k-{name}", down, [3840, 2160], quality, settings))
    elif suite == "tiers":
        for quality in ["off", "low", "very_high", "ultra"]:
            rows.append((f"ssr-{quality}", lake, [960, 540], quality, dict(base, on=False)))
        for quality in ["off", "low"]:
            rows.append((f"planar-{quality}", lake, [960, 540], quality, base))
    elif suite == "dx12":
        for name, quality, settings in controls:
            rows.append((name, lake, [960, 540], quality, settings))
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--suite", choices=["bounds", "long-bounds", "resolution", "fixtures", "tiers", "dx12", "coverage"], required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--binary", default="target/debug/postretro.exe")
    parser.add_argument("--repeats", type=int, default=2)
    args = parser.parse_args()
    root = Path(args.output).resolve()
    if args.repeats < 1 or (root.exists() and any(root.iterdir())):
        parser.error("positive repeats and a fresh output directory required")
    root.mkdir(parents=True, exist_ok=True)
    binary = Path(args.binary).resolve()
    env = os.environ.copy()
    env.update(POSTRETRO_PLANAR_REFLECTION="0", POSTRETRO_CPU_TIMING="1", POSTRETRO_GPU_TIMING="1", POSTRETRO_SH_STREAMING="sync-proof", RUST_LOG="info", WGPU_VALIDATION_INDIRECT_CALL="1", WGPU_BACKEND="dx12" if args.suite == "dx12" else "vulkan")
    rows = arms(args.suite)
    manifest = dict(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), git_head=subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), platform=platform.platform(), suite=args.suite, repeats=args.repeats, warmup_frames=120, sample_frames=240, backend=env["WGPU_BACKEND"], indirect_validation="on", profile="dev", features=["dev-tools", "capture"], maps={row[1][0]: hashlib.sha256(Path(row[1][0]).read_bytes()).hexdigest() for row in rows})
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    records = []
    for repeat in range(args.repeats):
        for name, pose, resolution, quality, settings in (reversed(rows) if repeat % 2 else rows):
            key = f"{name}-run{repeat + 1}"
            scene = dict(map=pose[0], camera=dict(position=pose[1], yaw_deg=pose[2], pitch_deg=pose[3]), resolution=resolution, reflection_quality=quality, planar_reflection=settings, output=str(root / (key + ".png")), measurement=dict(report=str(root / (key + ".json")), warmup_frames=120, sample_frames=240))
            scene_path = root / (key + "-scene.json")
            scene_path.write_text(json.dumps(scene, indent=2) + "\n")
            with (root / (key + ".log")).open("w") as log:
                subprocess.run([str(binary), "--capture", str(scene_path)], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=180, check=True)
            report = json.loads((root / (key + ".json")).read_text())
            log = (root / (key + ".log")).read_text()
            assert "SolidCellFallback" not in log and "Validation Error" not in log, key
            if settings["on"] and quality != "off":
                assert "[Planar reflection] plane=" in log, key
            assert report["gpu_timing"]["availability"] == "available", key
            records.append(dict(key=key, repeat=repeat + 1, configuration=name, settings=settings, quality=quality, cpu_completion=report["cpu_completion"], gpu_timing=report["gpu_timing"], adapter=report["adapter"], lightmap_counters=report["lightmap_streaming"]["counters"]))
            (root / "results.json").write_text(json.dumps(records, indent=2) + "\n")
            print(args.suite, key, round(report["cpu_completion"]["median_ms"], 3), flush=True)


if __name__ == "__main__":
    main()
