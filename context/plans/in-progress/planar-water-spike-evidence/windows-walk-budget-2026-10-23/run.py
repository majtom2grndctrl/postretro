"""Mirror-walk budget live matrix; extends windows-bounded-live-2026-10-23/run.py.

Run from the spike workspace root after the release build. Same pose, extents,
streaming, validation policy, tail windows and 20 s method as the bounded-live
matrix. Adds POSTRETRO_PLANAR_WALK_STEPS (mirror walk only) and an SSR
very_high row. very_high is a persisted player setting with no launch seed, so
that row runs under a separate --app-name whose settings.toml is a copy of the
default player file with only reflection_quality changed.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import time

VERY_HIGH_APP = "postretro-walk-budget-vh"
DEFAULT_WALK_STEPS = 20000
POSES = {
    # Bounded-live standing pose (pawn on the deck, settles to eye y~1.5).
    "across": "-16.8656,1.2,-48.768,90,-10",
}


def configurations():
    base = dict(on=True, scale=0.5, d=None, r=None, band_d=4, band_r=4, quality="high", walk=None)
    return [
        ("ssr-high", dict(base, on=False)),
        ("ssr-very-high", dict(base, on=False, quality="very_high")),
        ("half-unlimited", base),
        ("half-unlimited-w2000", dict(base, walk=2000)),
        ("half-unlimited-w500", dict(base, walk=500)),
        ("half-unlimited-w0", dict(base, walk=0)),
        ("half-d15", dict(base, d=15)),
    ]


def planar_settings(settings):
    return {k: settings[k] for k in ["on", "scale", "d", "band_d", "r", "band_r"]}


HIGH_LINE = 'reflection_quality = "high"'
VERY_HIGH_LINE = 'reflection_quality = "very_high"'


def prepare_very_high_profile():
    appdata = Path(os.environ["APPDATA"])
    source = appdata / "postretro" / "config" / "settings.toml"
    target = appdata / VERY_HIGH_APP / "config" / "settings.toml"
    lines = source.read_text(encoding="utf-8").splitlines()
    assert lines.count(HIGH_LINE) == 1, "default profile is not High"
    target.parent.mkdir(parents=True, exist_ok=True)
    copied = [VERY_HIGH_LINE if line == HIGH_LINE else line for line in lines]
    target.write_text("\n".join(copied) + "\n", encoding="utf-8", newline="\n")
    state = appdata / "postretro" / "data" / "postretro.dev" / "state.json"
    if state.exists():
        (appdata / VERY_HIGH_APP / "data" / "postretro.dev").mkdir(parents=True, exist_ok=True)
        shutil.copyfile(state, appdata / VERY_HIGH_APP / "data" / "postretro.dev" / "state.json")
    differing = [(a, b) for a, b in zip(lines, target.read_text(encoding="utf-8").splitlines()) if a != b]
    assert differing == [(HIGH_LINE, VERY_HIGH_LINE)], differing
    return str(target)


PROBE = Path(__file__).resolve().parent / "foreground" / "occlusion.ps1"
OVERLAY = "[CEF-OSC-WIDGET 'NVIDIA GeForce Overlay DT'"


def window_state(path):
    """Classify the probe: user window above or minimized excludes; overlay is a noted confounder."""
    lines = [line for line in path.read_text(encoding="utf-8", errors="replace").splitlines() if "engine pid=" in line]
    above = [line.split("overlapped_above_by=", 1)[1].strip() for line in lines]
    user_above = [a for a in above if a != "none" and not all(part.startswith(OVERLAY) for part in re.findall(r"\[[^\]]*\]", a))]
    minimized = [line for line in lines if "minimized=True" in line]
    foreground = [line for line in lines if re.search(r"fg=(\d+) ", line) and f"pid={re.search(r'fg=(\d+) ', line)[1]} " in line]
    return dict(samples=len(lines), overlay_samples=sum(OVERLAY in a for a in above), user_window_above_samples=len(user_above), minimized_samples=len(minimized), engine_foreground_samples=len(foreground), excluded=bool(user_above or minimized or not lines))


def floats(pattern, text):
    return [float(v) for v in re.findall(pattern, text)]


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
    assert effective == planar_settings(settings), (path.name, effective, settings)
    walk = re.findall(r"\[Renderer\] Planar mirror walk steps: (\d+)", text)
    expected_walk = DEFAULT_WALK_STEPS if settings["walk"] is None else settings["walk"]
    assert walk == [str(expected_walk)], (path.name, walk)
    tiers = re.findall(r"\[Renderer\] Reflection quality: (\w+)", text)
    # High is the renderer default and logs nothing; any other tier logs once.
    assert tiers == ({"high": [], "very_high": ["VeryHigh"]}[settings["quality"]]), (path.name, tiers)
    player = re.findall(r"\[Engine\] Player data: config (\S+),", text)
    expected_app = VERY_HIGH_APP if settings["quality"] == "very_high" else "postretro"
    assert len(player) == 1 and Path(player[0]).parent.name == expected_app, (path.name, player)
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
        assert len(values) in [0, 5], (path.name, label, len(values))
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
        assert len(values) in [0, 3], (path.name, label, len(values))
        gpu[label] = statistics.mean(values) if values else None
    assert (gpu["planar_reflection"] is not None) == settings["on"], path.name
    assert (gpu["fluid_reflection"] is None) == settings["on"], path.name
    planar = re.findall(r"\[Planar reflection\] plane=([^ ]+) seeds=(\d+) visible=(\d+) reachable=(\d+) mirror-only=(\d+) path=([^ ]+) considered=(\d+) accepted=(\d+)", text)
    assert bool(planar) == settings["on"], path.name
    mirror = [dict(plane=float(p[0]), seeds=int(p[1]), drawable=int(p[2]), reachable=int(p[3]), mirror_only=int(p[4]), path=p[5], considered=int(p[6]), accepted=int(p[7])) for p in planar]
    streaming = [line for line in text.splitlines() if "[Lightmap streaming] last" in line]
    joined = "\n".join(streaming)
    counters = dict(
        failed_installs=sum(floats(r"(\d+) failed installs", joined)),
        failed_reads=sum(floats(r"(\d+) failed reads", joined)),
        visible_misses_outside=sum(floats(r"(\d+) outside baked set", joined)),
        visible_misses_not_resident=sum(floats(r"(\d+) not resident", joined)),
        streaming_lines=len(streaming),
    )
    assert all(counters[k] == 0 for k in ["failed_installs", "failed_reads", "visible_misses_outside", "visible_misses_not_resident"]), (path.name, counters)
    return dict(effective_settings=effective, walk_steps=expected_walk, extents=extents[0], cpu_windows=5, cpu_frames=600, gpu_windows=3, gpu_readbacks=360, cpu_ms=cpu, gpu_ms=gpu, mirror=mirror[-1] if mirror else None, mirror_diagnostics=len(mirror), lightmap=counters)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/postretro.exe")
    parser.add_argument("--output", required=True)
    parser.add_argument("--seconds", type=int, default=20)
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--only", nargs="+", choices=[name for name, _ in configurations()])
    parser.add_argument("--pose", choices=sorted(POSES), default="across")
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
    for key in ["POSTRETRO_SH_STREAMING", "WGPU_VALIDATION_INDIRECT_CALL", "POSTRETRO_PLANAR_WALK_STEPS"]:
        env.pop(key, None)
    rows = [(name, settings) for name, settings in configurations() if not args.only or name in args.only]
    very_high_profile = prepare_very_high_profile() if any(s["quality"] == "very_high" for _, s in rows) else None
    engine_args = ["--mod", "dev", "maps/reflection-lake-test.prl", "--core-root", "core", "--windowed", f"--start-pose={POSES[args.pose]}", "--pool-seed", "1"]
    manifest = dict(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), map_sha256=hashlib.sha256(Path("content/dev/maps/reflection-lake-test.prl").read_bytes()).hexdigest(), git_head=subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), profile="release", features=["dev-tools", "capture"], backend="Vulkan", indirect_validation="release default off", streaming="ordinary", seconds=args.seconds, repeats=args.repeats, pose=args.pose, engine_arguments=engine_args, very_high_app_name=VERY_HIGH_APP if very_high_profile else None, very_high_settings=very_high_profile, note="Fixed standing camera. Tail CPU/GPU windows are asynchronous; live GPU logs round to 0.01 ms. mirror_walk times only the mirror portal walk; the fallback per-cell cull is inside visibility.")
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    records = []
    expected_extents = None
    with (root / "telemetry.csv").open("w") as telemetry_file:
        monitor = subprocess.Popen(["nvidia-smi", "--query-gpu=timestamp,pstate,memory.used,utilization.gpu,temperature.gpu,clocks.gr", "--format=csv", "-l", "5"], stdout=telemetry_file, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        try:
            for repeat in range(args.repeats):
                for name, settings in (list(reversed(rows)) if repeat % 2 else rows):
                    key = f"{name}-run{repeat + 1}"
                    env.update(POSTRETRO_PLANAR_REFLECTION=str(int(settings["on"])), POSTRETRO_PLANAR_SCALE=str(settings["scale"]), POSTRETRO_PLANAR_D=str(settings["d"]) if settings["d"] else "off", POSTRETRO_PLANAR_R=str(settings["r"]) if settings["r"] else "off", POSTRETRO_PLANAR_BAND_D=str(settings["band_d"]), POSTRETRO_PLANAR_BAND_R=str(settings["band_r"]))
                    if settings["walk"] is None:
                        env.pop("POSTRETRO_PLANAR_WALK_STEPS", None)
                    else:
                        env["POSTRETRO_PLANAR_WALK_STEPS"] = str(settings["walk"])
                    run_args = list(engine_args)
                    if settings["quality"] == "very_high":
                        run_args += ["--app-name", VERY_HIGH_APP]
                    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
                    occlusion_log = root / (key + "-occlusion.log")
                    with (root / (key + ".log")).open("w") as output:
                        child = subprocess.Popen([str(binary), *run_args], env=env, stdout=output, stderr=subprocess.STDOUT)
                        probe = subprocess.Popen(["pwsh", "-NoProfile", "-File", str(PROBE), "-Seconds", str(args.seconds - 1), "-Out", str(occlusion_log)], stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
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
                            try:
                                probe.wait(timeout=15)
                            except subprocess.TimeoutExpired:
                                probe.kill()
                    result = reduce_log(root / (key + ".log"), settings)
                    result["window"] = window_state(occlusion_log)
                    if expected_extents is None:
                        expected_extents = result["extents"]
                    assert result["extents"] == expected_extents, key
                    records.append(dict(key=key, configuration=name, repeat=repeat + 1, started_utc=started, settings=settings, engine_arguments=run_args, **result))
                    (root / "results.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
                    m = result["mirror"] or {}
                    print(key, "frame", round(result["cpu_ms"]["total"], 3), "mirror", result["cpu_ms"]["mirror_walk"], "vis", round(result["cpu_ms"]["visibility"], 3), "path", m.get("path"), "considered", m.get("considered"), "cells", m.get("drawable"), "window", result["window"], flush=True)
        finally:
            monitor.terminate()
            monitor.wait(timeout=10)


if __name__ == "__main__":
    main()
