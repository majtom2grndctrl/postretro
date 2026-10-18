"""Metal System Trace batch for the reflection-free fluid pass's base cost (Mac).

usage: python3 measurements/fluid-base-pass-cost/metal/run_ablation.py --arms a,b,... [options]
  --arms       comma list of <config>:<variant>[:<mask hex>] (variant is a file in
               variants/ without .wgsl, or "stock" for the binary's own shader; mask
               defaults to 0x13F, the all-terms mask without SPECULAR, which selects
               the reflection-free variant)
  --rounds N   interleaved rounds (default 2: forward, then reversed)
  --warmup N / --samples N   capture frames (default 120 / 120)
  --tag T      results file suffix (results-<T>.{json,md})
env: CELL_BIN (engine binary, must be named postretro), IDLE_GAP (default 15).

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

kit.RUNS = HERE / "runs"
kit.RAW = HERE / "raw"
UHD, FHD = kit.UHD, kit.FHD
AUTO = [1536, 960]
CONFIGS = {"near-4k": ("near", UHD), "near-1080": ("near", FHD), "test-4k": ("test", UHD),
           "mid-4k": ("mid", UHD), "far-4k": ("far", UHD),
           # The 5300M's Auto resolution (landing row M2).
           "near-auto": ("near", AUTO), "test-auto": ("test", AUTO), "mid-auto": ("mid", AUTO)}
FREE_MASK = 0x13F
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
    path.write_text(json.dumps(scene, indent=2) + "\n")
    return scene


kit.write_scene = write_scene


def parse_arm(text):
    parts = text.split(":")
    config, variant = parts[0], parts[1]
    mask = int(parts[2], 16) if len(parts) > 2 else FREE_MASK
    pose, res = CONFIGS[config]
    name = f"{config}-{variant}" + (f"-m{mask:03x}" if mask != FREE_MASK else "")
    wgsl = None if variant == "stock" else HERE / "variants" / f"{variant}.wgsl"
    if wgsl is not None and not wgsl.is_file():
        raise SystemExit(f"missing variant {wgsl} (run variants.py)")
    return {"name": name, "config": config, "pose": pose, "resolution": res, "mode": variant,
            "ratio_one": False, "mask": mask, "wgsl": str(wgsl) if wgsl else None}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arms", required=True)
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--warmup", type=int, default=120)
    ap.add_argument("--samples", type=int, default=120)
    ap.add_argument("--tag", default="ablation")
    args = ap.parse_args()
    chosen = [parse_arm(a) for a in args.arms.split(",")]
    kit.RUNS.mkdir(exist_ok=True)
    trace_dir = Path(tempfile.gettempdir()) / "fluid-base-pass-traces"
    trace_dir.mkdir(parents=True, exist_ok=True)
    binary = Path(os.environ.get("CELL_BIN", kit.ROOT / "target/release/postretro"))
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
                label = f"{args.tag}-{a['name']}-r{rnd}"
                print(f"run {label} ...", flush=True)
                r = kit.run_one(label, a, rnd, args, trace_dir, flags)
                r["variant"], r["mask"] = a["mode"], a["mask"]
                (kit.RUNS / f"{label}.run.json").write_text(json.dumps(r, indent=2) + "\n")
                records.append(r)
                print(f"  fluid {kit.fmt(r.get('fluid_ms'), 3)} forward {kit.fmt(r.get('forward_ms'), 3)} "
                      f"frame {kit.fmt(r.get('gpu_frame_ms_complete'), 3)} clock {kit.clock_text(r)} "
                      f"valid={r['valid']} {r['problems'] if not r['valid'] else ''}", flush=True)
    finally:
        summarize(records, args.tag)


def summarize(records, tag):
    by = {}
    for r in records:
        by.setdefault(r["arm"], []).append(r)
    mean = lambda xs: sum(xs) / len(xs) if xs else None
    rows = []
    md = [f"# Fluid base-pass ablation ({tag})", "",
          "Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per "
          "complete frame; clock = median busy core clock (min-max) per run.", "",
          "| arm | runs valid | fluid ms | spread | forward ms | GPU frame ms | clocks |",
          "|---|---|---|---|---|---|---|"]
    for name, rs in by.items():
        good = [r for r in rs if r["valid"] and r.get("fluid_ms") is not None]
        f = [r["fluid_ms"] for r in good]
        row = {"arm": name, "valid": len(good), "runs": len(rs), "fluid_ms": mean(f),
               "spread": (max(f) - min(f)) if len(f) > 1 else None,
               "forward_ms": mean([r["forward_ms"] for r in good]),
               "gpu_frame_ms": mean([r["gpu_frame_ms_complete"] for r in good]),
               "clocks": [kit.clock_text(r) for r in rs]}
        rows.append(row)
        md.append(f"| {name} | {len(good)}/{len(rs)} | {kit.fmt(row['fluid_ms'], 3)} | "
                  f"{kit.fmt(row['spread'], 3)} | {kit.fmt(row['forward_ms'], 3)} | "
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
