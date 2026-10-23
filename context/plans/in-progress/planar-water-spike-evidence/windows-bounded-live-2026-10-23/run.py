"""Run bounded live planar/probe experiments from the spike workspace root."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time


def configurations():
    base = dict(on=True, scale=0.5, d=None, r=None, band_d=4, band_r=4)
    return [
        ("ssr-high", dict(base, on=False)),
        ("half-unlimited", base),
        ("half-d30", dict(base, d=30)),
        ("half-d15", dict(base, d=15)),
        ("quarter-d30", dict(base, scale=0.25, d=30)),
        ("quarter-d15", dict(base, scale=0.25, d=15)),
        ("half-d30-r15", dict(base, d=30, r=15)),
        ("half-r8", dict(base, r=8)),
    ]


def reduce_log(path, settings):
    text = path.read_text(encoding="utf-8", errors="replace")
    for bad in ["SolidCellFallback", "Validation Error", "panicked at", "invalid POSTRETRO_"]:
        assert bad not in text, (path.name, bad)
    assert "vsync off (POSTRETRO_UNCAPPED launch seed)" in text, path.name
    assert "indirect-call validation: off" in text, path.name
    assert "[Renderer] GPU adapter: NVIDIA GeForce GTX 1660 SUPER (backend=Vulkan" in text, path.name
    launch = re.search(r"Planar launch settings: PlanarReflectionSettings \{ on: (true|false), scale: ([0-9.]+), d: (None|Some\(([0-9.]+)\)), band_d: ([0-9.]+), r: (None|Some\(([0-9.]+)\)), band_r: ([0-9.]+)", text)
    assert launch, path.name
    effective = dict(on=launch[1] == "true", scale=float(launch[2]), d=float(launch[4]) if launch[4] else None, band_d=float(launch[5]), r=float(launch[7]) if launch[7] else None, band_r=float(launch[8]))
    assert effective == settings, (path.name, effective, settings)
    extents = re.findall(r"render extents: surface=(\d+)x(\d+) scene=(\d+)x(\d+) divisor=(\d+)", text)
    assert extents and all(e == extents[0] for e in extents), path.name
    cpu_lines = [line for line in text.splitlines() if "[CpuTiming] frames=120" in line][-5:]
    gpu_lines = [line for line in text.splitlines() if "[gpu-timing]" in line][-3:]
    assert len(cpu_lines) == 5 and len(gpu_lines) == 3, path.name
    cpu = {}
    for label in ["total", "work", "wait", "visibility", "portal_walk", "mirror_walk", "render_prep", "render_record", "render_submit"]:
        values = []
        for line in cpu_lines:
            match = re.search(r"(?:\s)" + label + r"=([0-9.]+)/([0-9.]+)ms(?:\((\d+)/120\))?", line)
            if match:
                assert match[3] in [None, "120"], (path.name, label, match[3])
                values.append(float(match[1]))
        cpu[label] = statistics.mean(values) if values else None
    assert (cpu["mirror_walk"] is not None) == settings["on"], path.name
    gpu = {}
    for label in ["fluid_reflection", "planar_reflection", "planar_forward", "forward", "fluid_surface", "bloom", "resolve"]:
        values = []
        for line in gpu_lines:
            match = re.search(r"(?:\s|\|)" + label + r" ([0-9.]+)ms(?: \((\d+)/120\))?", line)
            if match:
                assert match[2] in [None, "120"], (path.name, label, match[2])
                values.append(float(match[1]))
        gpu[label] = statistics.mean(values) if values else None
    assert (gpu["planar_reflection"] is not None) == settings["on"], path.name
    assert (gpu["fluid_reflection"] is None) == settings["on"], path.name
    planar = re.findall(r"\[Planar reflection\] plane=([^ ]+) seeds=(\d+) visible=(\d+) reachable=(\d+) mirror-only=(\d+) path=([^ ]+) considered=(\d+) accepted=(\d+)", text)
    if settings["on"]:
        assert planar, path.name
    for line in text.splitlines():
        if "[Lightmap streaming] last" in line:
            assert "0 failed installs, 0 failed reads" in line, path.name
            assert "0 outside baked set, 0 not resident" in line, path.name
    return dict(effective_settings=effective, extents=extents[0], cpu_windows=5, cpu_frames=600, gpu_windows=3, gpu_readbacks=360, cpu_ms=cpu, gpu_ms=gpu, mirror=planar[-1:] if planar else [])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/postretro.exe")
    parser.add_argument("--output", required=True)
    parser.add_argument("--seconds", type=int, default=25)
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--only", choices=[name for name, _ in configurations()])
    args = parser.parse_args()
    root = Path(args.output).resolve()
    if args.seconds < 15 or args.repeats < 1 or (root.exists() and any(root.iterdir())):
        parser.error("at least 15 seconds, positive repeats and a fresh output directory required")
    root.mkdir(parents=True, exist_ok=True)
    binary = Path(args.binary).resolve()
    env = os.environ.copy()
    env.update(WGPU_BACKEND="vulkan", POSTRETRO_UNCAPPED="1", POSTRETRO_CPU_TIMING="1", POSTRETRO_GPU_TIMING="1", RUST_LOG="info")
    temporary = Path("target/planar-bake-tmp").resolve()
    temporary.mkdir(parents=True, exist_ok=True)
    env.update(TEMP=str(temporary), TMP=str(temporary))
    env.pop("POSTRETRO_SH_STREAMING", None)
    env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
    engine_args = ["--mod", "dev", "maps/reflection-lake-test.prl", "--core-root", "core", "--windowed", "--start-pose=-16.8656,1.2,-48.768,90,-10", "--pool-seed", "1"]
    manifest = dict(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), map_sha256=hashlib.sha256(Path("content/dev/maps/reflection-lake-test.prl").read_bytes()).hexdigest(), git_head=subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), profile="release", features=["dev-tools", "capture"], backend="Vulkan", indirect_validation="release default off", streaming="ordinary", seconds=args.seconds, repeats=args.repeats, engine_arguments=engine_args, note="Fixed standing camera; probes remain the distant layer. Tail CPU/GPU windows are asynchronous; live GPU logs round to 0.01 ms.")
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    records = []
    expected_extents = None
    with (root / "telemetry.csv").open("w") as telemetry_file:
        monitor = subprocess.Popen(["nvidia-smi", "--query-gpu=timestamp,pstate,memory.used,utilization.gpu,temperature.gpu,clocks.gr", "--format=csv", "-l", "5"], stdout=telemetry_file, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        try:
            for repeat in range(args.repeats):
                rows = [(name, settings) for name, settings in configurations() if not args.only or name == args.only]
                for name, settings in (reversed(rows) if repeat % 2 else rows):
                    key = f"{name}-run{repeat + 1}"
                    env.update(POSTRETRO_PLANAR_REFLECTION=str(int(settings["on"])), POSTRETRO_PLANAR_SCALE=str(settings["scale"]), POSTRETRO_PLANAR_D=str(settings["d"]) if settings["d"] else "off", POSTRETRO_PLANAR_R=str(settings["r"]) if settings["r"] else "off", POSTRETRO_PLANAR_BAND_D=str(settings["band_d"]), POSTRETRO_PLANAR_BAND_R=str(settings["band_r"]))
                    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
                    with (root / (key + ".log")).open("w") as output:
                        child = subprocess.Popen([str(binary), *engine_args], env=env, stdout=output, stderr=subprocess.STDOUT)
                        try:
                            deadline = time.monotonic() + args.seconds
                            while time.monotonic() < deadline:
                                if child.poll() is not None:
                                    raise RuntimeError(f"{key} exited early: {child.returncode}")
                                time.sleep(0.2)
                        finally:
                            if child.poll() is None:
                                child.terminate()
                            child.wait(timeout=10)
                    result = reduce_log(root / (key + ".log"), settings)
                    if expected_extents is None:
                        expected_extents = result["extents"]
                    assert result["extents"] == expected_extents, key
                    records.append(dict(key=key, configuration=name, repeat=repeat + 1, started_utc=started, settings=settings, **result))
                    (root / "results.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
                    print(key, "frame", round(result["cpu_ms"]["total"], 3), "work", round(result["cpu_ms"]["work"], 3), "mirror", result["cpu_ms"]["mirror_walk"], flush=True)
        finally:
            monitor.terminate()
            monitor.wait(timeout=10)


if __name__ == "__main__":
    main()
