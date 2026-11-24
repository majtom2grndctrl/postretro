"""Metal System Trace batch for the reflection-free fluid pass's base cost (Mac).

usage: python3 measurements/fluid-base-pass-cost/metal/run_ablation.py --arms a,b,... [options]
  --arms       comma list of <config>:<variant>[:<mask hex>[:<reflection quality>]] (variant is a file in
               variants/ without .wgsl, "stock" for the binary's own shader, or
               "before"/"after" for a two-binary A/B: that arm launches
               CELL_BIN_BEFORE / CELL_BIN_AFTER with its own shader; mask
               defaults to 0x13F, the all-terms mask without SPECULAR, which selects
               the reflection-free variant)
  --rounds N   interleaved rounds (default 2: forward, then reversed)
  --warmup N / --samples N   capture frames (default 120 / 120)
  --tag T      results file suffix (results-<T>.{json,md})
  --report glossy   overlay / sums / floor chain / floor march / forward / frame / overlap
                    table (glossy-world-reflections)
  --abort-invalid   stop at the first invalid run (pilot)
  --min-free-gb N   stop before a run when / has less free space (default 16)
env: CELL_BIN (engine binary, must be named postretro), CELL_BIN_BEFORE /
     CELL_BIN_AFTER (per-arm binaries for the before/after variants, each named
     postretro), IDLE_GAP (default 15), MAP_ROOT_BEFORE / MAP_ROOT_AFTER (a
     content/<mod>/maps directory each binary's own prl-build wrote; the
     before/after arms read their map's basename there).

A thin driver over run.py (copied from claude/cell-rate-spike's kit): same
recording, watchdog, clock polling, machine-state checks, complete-frame
reduction and trace deletion. What differs: the arms (pose x shader variant x
light mask), the capture scene (main's schema: no cell levers), and the shader
override, passed to the engine as POSTRETRO_FLUID_WGSL_EXPERIMENT through
xctrace --env.
"""
import argparse, atexit, json, os, shutil, signal, subprocess, sys, tempfile, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run as kit  # noqa: E402
import gpu_time  # noqa: E402

kit.RUNS = HERE / "runs"
kit.RAW = HERE / "raw"
UHD, FHD = kit.UHD, kit.FHD
AUTO = [1536, 960]
QHD = [2560, 1440]
CONFIGS = {"near-4k": ("near", UHD), "near-1080": ("near", FHD), "test-4k": ("test", UHD),
           "mid-4k": ("mid", UHD), "far-4k": ("far", UHD),
           # The 5300M's Auto resolution (landing row M2).
           "near-auto": ("near", AUTO), "test-auto": ("test", AUTO), "mid-auto": ("mid", AUTO),
           # reflection-probe-cell-path's remaining-cost table (shape measurement 1):
           # every view at Auto, 1440p and 4K.
           "low-4k": ("low", UHD), "low-auto": ("low", AUTO), "far-auto": ("far", AUTO),
           "near-1440": ("near", QHD), "test-1440": ("test", QHD), "mid-1440": ("mid", QHD),
           "low-1440": ("low", QHD), "far-1440": ("far", QHD)}
FREE_MASK = 0x13F
# glossy-world-reflections checkpoint 2 (rows M5, M6). The test map
# (content/dev/maps/glossy-reflections-test.map): interior 13 x 13 m, floor at
# y 0, spawn at map (64, 64) facing the monument at the room centre (engine
# -6.5, -6.5). across: spawn, eye height, toward the monument, pitched down so
# the floor fills about half the frame. down: straight down between spawn and
# the plinth, the floor filling the frame. Configs ending -off run reflection
# quality off (forward one-output form, no overlay): append :17F:off.
GLOSSY_MAP = "content/dev/maps/glossy-reflections-test.prl"
kit.POSES.update({
    "glossy-across": {"map": GLOSSY_MAP, "position": [-1.6256, 1.6, -1.6256], "yaw": 45.0, "pitch": -15.0},
    "glossy-down": {"map": GLOSSY_MAP, "position": [-3.5, 1.6, -3.5], "yaw": 45.0, "pitch": -89.0},
})
CONFIGS.update({f"glossy-{v}-{r}": (f"glossy-{v}", res) for v in ("across", "down")
                for r, res in (("auto", AUTO), ("1440", QHD), ("4k", UHD))})
# Maps without fluid: the complete-frame basis requires these labels instead
# of the fluid pass (plus the overlay on arms whose tier draws it).
NO_FLUID_POSES = {"glossy-across", "glossy-down"}
# Two-binary A/B: these variants run their own binary's shader, no override.
BINARY_VARIANTS = {"before": "CELL_BIN_BEFORE", "after": "CELL_BIN_AFTER"}
# Tiers that run the glossy floor layer (Task 11: FLOOR_LAYER_PRESETS); an arm
# without a quality field runs the default, high.
FLOOR_TIERS = ("medium", "high", "very_high", "ultra")
BASE_ENV = dict(kit.ENGINE_ENV)


def write_scene(path, arm, png, report, warmup, samples, mapname=None, quick=False):
    pose = kit.POSES[arm["pose"]]
    scene = {
        "map": mapname or pose["map"],
        "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                   "pitch_deg": pose["pitch"], "fov_deg": kit.FOV},
        "resolution": arm["resolution"],
        "output": str(png),
        "measurement": {"report": str(report), "warmup_frames": warmup, "sample_frames": samples},
        "light_term_mask": arm["mask"],
    }
    if arm.get("quality"):
        scene["reflection_quality"] = arm["quality"]
    # Two-binary arms read maps their own prl-build compiled:
    # MAP_ROOT_BEFORE / MAP_ROOT_AFTER name a content/<mod>/maps directory.
    root = os.environ.get(f"MAP_ROOT_{arm['mode'].upper()}") if arm["mode"] in BINARY_VARIANTS else None
    if root:
        scene["map"] = str(Path(root) / Path(scene["map"]).name)
    path.write_text(json.dumps(scene, indent=2) + "\n")
    return scene


kit.write_scene = write_scene


def parse_arm(text):
    parts = text.split(":")
    config, variant = parts[0], parts[1]
    mask = int(parts[2], 16) if len(parts) > 2 else FREE_MASK
    quality = parts[3] if len(parts) > 3 else None
    pose, res = CONFIGS[config]
    name = (f"{config}-{variant}" + (f"-m{mask:03x}" if mask != FREE_MASK else "")
            + (f"-{quality}" if quality else ""))
    own_shader = variant == "stock" or variant in BINARY_VARIANTS
    wgsl = None if own_shader else HERE / "variants" / f"{variant}.wgsl"
    if wgsl is not None and not wgsl.is_file():
        raise SystemExit(f"missing variant {wgsl} (run variants.py)")
    if variant in BINARY_VARIANTS:
        if BINARY_VARIANTS[variant] not in os.environ:
            raise SystemExit(f"variant {variant} needs {BINARY_VARIANTS[variant]}")
        binary = Path(os.environ[BINARY_VARIANTS[variant]])
    else:
        binary = Path(os.environ.get("CELL_BIN", kit.ROOT / "target/release/postretro"))
    return {"name": name, "config": config, "pose": pose, "resolution": res, "mode": variant,
            "ratio_one": False, "mask": mask, "wgsl": str(wgsl) if wgsl else None,
            "binary": str(binary), "quality": quality}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arms", required=True)
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--warmup", type=int, default=120)
    ap.add_argument("--samples", type=int, default=120)
    ap.add_argument("--tag", default="ablation")
    ap.add_argument("--report", choices=("fluid", "glossy"), default="fluid")
    ap.add_argument("--min-free-gb", type=float, default=16.0)
    ap.add_argument("--abort-invalid", action="store_true", help="stop the batch at its first invalid run (pilots)")
    args = ap.parse_args()
    chosen = [parse_arm(a) for a in args.arms.split(",")]
    kit.RUNS.mkdir(exist_ok=True)
    trace_dir = Path(tempfile.gettempdir()) / "fluid-base-pass-traces"
    trace_dir.mkdir(parents=True, exist_ok=True)
    for binary in {Path(a["binary"]) for a in chosen}:
        if binary.name != "postretro" or not binary.is_file():
            raise SystemExit(f"bad engine binary {binary}")
    if kit.busy_processes():
        raise SystemExit(f"busy: {kit.busy_processes()}")
    flags = kit.xctrace_flags()
    caf = subprocess.Popen(["caffeinate", "-d", "-i", "-m", "-s", "-u", "-t", "14400"])
    awake = threading.Event()

    def redeclare():
        while not awake.is_set():
            subprocess.run(["caffeinate", "-u", "-t", "5"])

    threading.Thread(target=redeclare, daemon=True).start()

    def cleanup():
        awake.set()
        caf.terminate()
        shutil.rmtree(trace_dir, ignore_errors=True)

    atexit.register(cleanup)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(130))
    idle = kit.idle_snapshot(kit.RUNS / "idle.ioreg.txt")
    print("idle ioreg:", json.dumps(idle), flush=True)

    records = []
    try:
        for rnd in range(1, args.rounds + 1):
            order = chosen if rnd % 2 else list(reversed(chosen))
            for a in order:
                busy = kit.busy_processes()
                waited = 0
                # Another session's build: wait (never kill), up to 30 min.
                while busy and waited < 1800:
                    print(f"waiting: {busy}", flush=True)
                    time.sleep(30)
                    waited += 30
                    busy = kit.busy_processes()
                if busy:
                    raise SystemExit(f"busy for 30 min: {busy}")
                time.sleep(float(os.environ.get("IDLE_GAP", "15")))
                kit.ENGINE_ENV = dict(BASE_ENV)
                if a["wgsl"]:
                    kit.ENGINE_ENV["POSTRETRO_FLUID_WGSL_EXPERIMENT"] = a["wgsl"]
                os.environ["CELL_BIN"] = a["binary"]  # run.py's run_one launches CELL_BIN
                if kit.free_gb("/") < args.min_free_gb:
                    raise SystemExit(f"under {args.min_free_gb} GB free on /")
                no_fluid = a["pose"] in NO_FLUID_POSES
                kit.REQUIRE_FLUID = not no_fluid
                # Low draws the probe term alone, so the overlay is required
                # there too; floor tiers also require one floor march.
                overlay = (no_fluid and a["mode"] == "after" and a["mask"] == 0x17F
                           and a["quality"] != "off")
                floor = overlay and (a["quality"] or "high") in FLOOR_TIERS
                gpu_time.COMPLETE_REQUIRED = (("Depth Pre-Pass", "Textured Pass")
                                              + (() if no_fluid else ("Fluid Surface Pass",))
                                              + ((gpu_time.GLOSSY_LABEL,) if overlay else ())
                                              + ((gpu_time.FLOOR_MARCH_LABEL,) if floor else ()))
                label = f"{args.tag}-{a['name']}-r{rnd}"
                print(f"run {label} ...", flush=True)
                r = kit.run_one(label, a, rnd, args, trace_dir, flags)
                r["variant"], r["mask"], r["binary"] = a["mode"], a["mask"], a["binary"]
                r["quality"], r["complete_required"] = a["quality"], list(gpu_time.COMPLETE_REQUIRED)
                add_glossy_figures(r)
                (kit.RUNS / f"{label}.run.json").write_text(json.dumps(r, indent=2) + "\n")
                records.append(r)
                print(f"  busy clock {busy_clock_text(r)}", flush=True)
                print(f"  overlay {kit.fmt(r.get('glossy_ms'), 3)} sums {kit.fmt(r.get('sums_any_ms'), 3)} "
                      f"chain {kit.fmt(r.get('chain_ms'), 3)} march {kit.fmt(r.get('march_ms'), 3)} "
                      f"fluid {kit.fmt(r.get('fluid_ms'), 3)} forward {kit.fmt(r.get('forward_ms'), 3)} "
                      f"frame {kit.fmt(r.get('gpu_frame_ms_complete'), 3)} clock {kit.clock_text(r)} "
                      f"valid={r['valid']} {r['problems'] if not r['valid'] else ''}", flush=True)
                if args.abort_invalid and not r["valid"]:
                    raise SystemExit(f"invalid run {label}: aborting the batch")
    finally:
        (summarize_glossy if args.report == "glossy" else summarize)(records, args.tag)


def add_glossy_figures(r):
    """Overlay, sums (either name), forward, the sum of every labelled pass's
    mean per-frame time over complete frames, and that sum minus the union
    frame time: the overlap of passes running concurrently on the GPU."""
    # The kit's clock falls back to every sample when under five read busy,
    # which a short timed span at Auto does; this is the busy samples alone.
    busy = sorted(s["Core Clock(MHz)"] for s in r.get("gpu_state_samples", [])
                  if s.get("GPU Activity(%)", 0) >= kit.BUSY_ACTIVITY and "Core Clock(MHz)" in s)
    r["busy_clock_mhz"] = {"n": len(busy), "median": kit.median(busy),
                           "min": busy[0] if busy else None, "max": busy[-1] if busy else None}
    complete = ((r.get("gpu") or {}).get("complete") or {})
    passes = complete.get("passes") or {}
    get = lambda label: passes[label]["per_frame_ms"] if label in passes else 0.0
    if not passes:
        return
    r["glossy_ms"] = get(gpu_time.GLOSSY_LABEL)
    r["sums_any_ms"] = get(gpu_time.PROBE_SUMS_LABEL) + get(gpu_time.SUMS_LABEL)
    r["chain_ms"] = get(gpu_time.FLOOR_CHAIN_LABEL)
    r["march_ms"] = get(gpu_time.FLOOR_MARCH_LABEL)
    r["pass_sum_ms"] = round(sum(p["per_frame_ms"] for p in passes.values()), 4)
    frame = complete.get("gpu_frame_ms_complete")
    r["overlap_ms"] = round(r["pass_sum_ms"] - frame, 4) if frame is not None else None


def busy_clock_text(r):
    c = r.get("busy_clock_mhz") or {}
    return "n/a" if not c.get("n") else f"{c['median']} ({c['min']}-{c['max']}, n={c['n']})"


def summarize_glossy(records, tag):
    by = {}
    for r in records:
        by.setdefault(r["arm"], []).append(r)
    mean = lambda xs: sum(xs) / len(xs) if xs else None
    spread = lambda xs: (max(xs) - min(xs)) if len(xs) > 1 else None
    rows = []
    md = [f"# Glossy world reflections, 5300M ({tag})", "",
          "Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. "
          "frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; "
          "overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; "
          "march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy "
          "samples (GPU activity >= 50%), with their count, per run.", "",
          "| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | "
          "GPU frame ms | spread | pass sum ms | overlap ms | clocks |",
          "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for name, rs in by.items():
        good = [r for r in rs if r["valid"] and r.get("forward_ms") is not None]
        col = lambda k: [r[k] for r in good if r.get(k) is not None]
        row = {"arm": name, "valid": len(good), "runs": len(rs),
               "glossy_ms": mean(col("glossy_ms")), "glossy_spread": spread(col("glossy_ms")),
               "sums_ms": mean(col("sums_any_ms")),
               "chain_ms": mean(col("chain_ms")), "chain_spread": spread(col("chain_ms")),
               "march_ms": mean(col("march_ms")), "march_spread": spread(col("march_ms")),
               "forward_ms": mean(col("forward_ms")), "forward_spread": spread(col("forward_ms")),
               "gpu_frame_ms": mean(col("gpu_frame_ms_complete")),
               "frame_spread": spread(col("gpu_frame_ms_complete")),
               "pass_sum_ms": mean(col("pass_sum_ms")), "overlap_ms": mean(col("overlap_ms")),
               "clocks": [busy_clock_text(r) for r in rs]}
        rows.append(row)
        f = kit.fmt
        md.append(f"| {name} | {len(good)}/{len(rs)} | {f(row['glossy_ms'], 3)} | {f(row['glossy_spread'], 3)} | "
                  f"{f(row['sums_ms'], 3)} | {f(row['chain_ms'], 3)} | {f(row['march_ms'], 3)} | "
                  f"{f(row['forward_ms'], 3)} | {f(row['forward_spread'], 3)} | "
                  f"{f(row['gpu_frame_ms'], 3)} | {f(row['frame_spread'], 3)} | {f(row['pass_sum_ms'], 3)} | "
                  f"{f(row['overlap_ms'], 3)} | {'; '.join(row['clocks'])} |")
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "Invalid runs:"] + [f"- {r['label']}: {'; '.join(r['problems'])}" for r in bad]
    out = {"runs": [{k: v for k, v in r.items() if k not in ("gpu_state_samples", "command")} for r in records],
           "arms": rows}
    (HERE.parent / f"results-{tag}.json").write_text(json.dumps(out, indent=2) + "\n")
    (HERE.parent / f"results-{tag}.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))


def summarize(records, tag):
    by = {}
    for r in records:
        by.setdefault(r["arm"], []).append(r)
    mean = lambda xs: sum(xs) / len(xs) if xs else None
    rows = []
    md = [f"# Fluid base-pass ablation ({tag})", "",
          "Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per "
          "complete frame; clock = median busy core clock (min-max) per run.", "",
          "| arm | runs valid | fluid ms | spread | sums ms | fluid + sums ms | SSR ms | forward ms | GPU frame ms | clocks |",
          "|---|---|---|---|---|---|---|---|---|---|"]
    for name, rs in by.items():
        good = [r for r in rs if r["valid"] and r.get("fluid_ms") is not None]
        f = [r["fluid_ms"] for r in good]
        row = {"arm": name, "valid": len(good), "runs": len(rs), "fluid_ms": mean(f),
               "spread": (max(f) - min(f)) if len(f) > 1 else None,
               "sums_ms": mean([r.get("sums_ms", 0.0) for r in good]),
               "fluid_plus_sums_ms": mean([r["fluid_ms"] + r.get("sums_ms", 0.0) for r in good]),
               "ssr_ms": mean([r.get("ssr_ms", 0.0) for r in good]),
               "forward_ms": mean([r["forward_ms"] for r in good]),
               "gpu_frame_ms": mean([r["gpu_frame_ms_complete"] for r in good]),
               "clocks": [kit.clock_text(r) for r in rs]}
        rows.append(row)
        md.append(f"| {name} | {len(good)}/{len(rs)} | {kit.fmt(row['fluid_ms'], 3)} | "
                  f"{kit.fmt(row['spread'], 3)} | {kit.fmt(row['sums_ms'], 3)} | "
                  f"{kit.fmt(row['fluid_plus_sums_ms'], 3)} | {kit.fmt(row['ssr_ms'], 3)} | "
                  f"{kit.fmt(row['forward_ms'], 3)} | "
                  f"{kit.fmt(row['gpu_frame_ms'], 3)} | {'; '.join(row['clocks'])} |")
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "Invalid runs:"] + [f"- {r['label']}: {'; '.join(r['problems'])}" for r in bad]
    out = {"runs": [{k: v for k, v in r.items() if k not in ("gpu_state_samples", "command")} for r in records],
           "arms": rows}
    (HERE.parent / f"results-{tag}.json").write_text(json.dumps(out, indent=2) + "\n")
    (HERE.parent / f"results-{tag}.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))


if __name__ == "__main__":
    main()
