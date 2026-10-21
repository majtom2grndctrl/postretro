"""Metal System Trace batch for the fluid SSR reflection pass (Mac, 5300M).

usage: python3 measurements/fluid-ssr-reflections/metal/run_sweep.py
           (--preset NAME | --arms SPEC,SPEC,...) [options]

Arm spec:  <pose>-<res>:<quality>[:<style>][:<k>=<v>,...]
  pose     near | probes | oblique-near | oblique-probes | oblique-large
  res      4k (3840x2160) | auto (1536x960, the 5300M's Auto) |
           ext (the external monitor's play extent, from SSR_EXT_RES=WxH)
  quality  off | low | medium | high | very_high | ultra   -> "reflection_quality"
  style    pixel | smooth                                   -> "reflection_style"
  k=v      fraction=<f>, steps=<n>, divisor=<n>, thickness=<m>, any subset,
           comma-separated in one segment                    -> "reflection_march"
  Style and march are optional and may come in either order; an omitted
  field is omitted from the scene (the engine default applies).
  e.g. near-auto:high:pixel:steps=128,divisor=270,thickness=1.0,fraction=1.0

Presets (--preset):
  tiers           every quality x {near, probes} x {4k, auto}; near-auto:medium:steps=48;
                  near-ext:off and probes-ext:off; every quality x the three
                  oblique poses x {auto, ext}
  sweep           near-auto:high:pixel, fraction 1.0, thickness 1.0,
                  steps {64,96,128,192,256} x divisor {135,180,270,350}
  pilot           near-auto:off, near-auto:high, near-4k:ultra, probes-auto:medium
  confirm-smooth  each --arms spec re-run with style smooth (the finalists)
  finalists-ext   each --arms spec re-run at res ext
  --arms without --preset runs exactly those specs; with tiers/sweep/pilot it
  appends them.

options:
  --pilot          one launch per distinct arm, round 1 only; stop at the first
                   invalid run (machine state, missing expected pass, watchdog,
                   clock floor if CLOCK_FLOOR_MHZ is set)
  --dry-run        print each arm's scene JSON and launch command, and the run
                   order; launch nothing, run no xctrace, write nothing
  --skip-pending   drop arms whose pose or res is still pending instead of refusing
  --poses FILE     JSON {pose name: scene fragment} filling placeholder poses
                   (also $SSR_POSES); a fragment is {"map": ..., "camera":
                   {"position": [x,y,z], "yaw_deg": .., "pitch_deg": .. [, "fov_deg": ..]}}
  --rounds N       interleaved rounds (default 2, ABBA: forward, then reversed)
  --warmup N / --samples N   capture frames (default 120 / 360)
  --skip-smoke     skip the untraced smoke capture that opens the batch
  --tag T          results file suffix (default: the preset name, else "arms")
env: SSR_BIN (engine binary, must be named postretro), SSR_EXT_RES (WxH),
     IDLE_GAP (default 15), RECORD_TIMEOUT, MIN_FREE_GB, SKIP_RAW, CLOCK_FLOOR_MHZ,
     SH_STREAMING (POSTRETRO_SH_STREAMING; default sync-proof, as on the 1660).

Modelled on measurements/fluid-base-pass-cost/metal/run_ablation.py: a thin
driver over run.py (recording, watchdog, clock polling, machine-state checks,
complete-frame reduction, trace deletion), with its own arms and summary.
"""
import argparse, atexit, json, math, os, re, shlex, shutil, signal, subprocess, sys, tempfile, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run as kit  # noqa: E402

# --- poses -----------------------------------------------------------------

# Engine coordinates (x = -map y, y = map z, z = -map x, metres).
PROBES_MAP = "content/dev/maps/reflection-probes-test.prl"
E24_MAP = "content/dev/maps/e24-environment-test.prl"
VAULT_MAP = "content/dev/maps/vault-test.prl"
POSES = {
    # e24's still pool from 1.6 m, straight down: one fluid draw covering the screen.
    "near": {"map": E24_MAP, "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -89.0},
    # The reflection-probes-test pool, straight down (the old kit's "test").
    "probes": {"map": PROBES_MAP, "position": [-6.5024, 1.6, -6.5024], "yaw": 0.0, "pitch": -89.0},
    # Oblique gameplay poses from the Windows session (af8bfef61). Checked by a
    # brush ray-cast only, not a render: eyeball each pilot PNG.
    # (C) e24 still pool seen from its south ledge; same pool as "near".
    "oblique-near": {"map": E24_MAP, "position": [-1.016, 1.6, -44.2976], "yaw": 90.0, "pitch": -20.0},
    # (A) Room B basin from its south-west corner, pillar centred 5.5 m away; water ~44% of frame.
    "oblique-probes": {"map": PROBES_MAP, "position": [-2.6416, 1.6, -25.4], "yaw": 40.0, "pitch": -20.0},
    # (B) Largest water: vault-test's 348 m2 pool along its 30 m axis from the east deck,
    # eye 2.8 m above the water.
    "oblique-large": {"map": VAULT_MAP, "position": [-35.052, 4.394, -75.1332], "yaw": -125.0, "pitch": -20.0},
}


def load_poses(path):
    """Fill poses from scene JSON fragments: {name: {"map", "camera": {...}}}."""
    for name, frag in json.loads(Path(path).read_text()).items():
        cam = frag.get("camera", frag)
        pose = {"map": frag.get("map") or POSES.get(name, {}).get("map"),
                "position": cam["position"], "yaw": cam.get("yaw_deg", 0.0), "pitch": cam["pitch_deg"]}
        if "fov_deg" in cam:
            pose["fov"] = cam["fov_deg"]
        if not pose["map"]:
            raise SystemExit(f"pose {name} in {path} has no map")
        POSES[name] = pose


# --- resolutions -----------------------------------------------------------

def ext_resolution():
    text = os.environ.get("SSR_EXT_RES", "")
    m = re.fullmatch(r"\s*(\d+)\s*[xX]\s*(\d+)\s*", text)
    return [int(m[1]), int(m[2])] if m else None


RESOLUTIONS = {"4k": kit.UHD, "auto": [1536, 960], "ext": ext_resolution()}

# --- arms ------------------------------------------------------------------

QUALITIES = ("off", "low", "medium", "high", "very_high", "ultra")
STYLES = ("pixel", "smooth")
MARCH_KEYS = {"fraction": float, "steps": int, "divisor": int, "thickness": float}
# Whether the Fluid Reflection Pass must appear: medium and above must record
# it (absent is an anomaly, not 0), off and low must not (low is probe
# reflections only under the six-tier ladder, so a recorded pass is an anomaly).
REFLECTION_EXPECTED = {"off": 0, "low": 0, "medium": 1, "high": 1, "very_high": 1, "ultra": 1}


def parse_arm(spec):
    parts = spec.strip().split(":")
    if len(parts) < 2 or "-" not in parts[0]:
        raise SystemExit(f"bad arm spec {spec!r}: want <pose>-<res>:<quality>[:style][:k=v,...]")
    pose, res = parts[0].rsplit("-", 1)
    quality = parts[1]
    if pose not in POSES:
        raise SystemExit(f"arm {spec!r}: unknown pose {pose!r} (have {', '.join(POSES)})")
    if res not in RESOLUTIONS:
        raise SystemExit(f"arm {spec!r}: unknown res {res!r} (have {', '.join(RESOLUTIONS)})")
    if quality not in QUALITIES:
        raise SystemExit(f"arm {spec!r}: unknown quality {quality!r} (have {', '.join(QUALITIES)})")
    style, march = None, None
    for part in parts[2:]:
        if "=" in part:
            if march is not None:
                raise SystemExit(f"arm {spec!r}: one k=v segment only")
            march = {}
            for kv in part.split(","):
                k, _, v = kv.partition("=")
                if k not in MARCH_KEYS:
                    raise SystemExit(f"arm {spec!r}: unknown march key {k!r} (have {', '.join(MARCH_KEYS)})")
                try:
                    march[k] = MARCH_KEYS[k](v)
                except ValueError:
                    raise SystemExit(f"arm {spec!r}: {k}={v!r} is not a {MARCH_KEYS[k].__name__}")
        elif part in STYLES:
            if style is not None:
                raise SystemExit(f"arm {spec!r}: one style only")
            style = part
        else:
            raise SystemExit(f"arm {spec!r}: {part!r} is neither a style ({'/'.join(STYLES)}) nor k=v")
    return make_arm(pose, res, quality, style, march)


def make_arm(pose, res, quality, style=None, march=None):
    p = POSES[pose]
    pending = []
    if p.get("pending"):
        pending.append("pose pending")
    resolution = RESOLUTIONS[res]
    if resolution is None:
        pending.append("res pending (set SSR_EXT_RES=WxH)")
    # Canonical order: march keys as MARCH_KEYS lists them.
    march = {k: march[k] for k in MARCH_KEYS if march and k in march} or None
    config = ":".join([quality] + ([style] if style else []) +
                      ([",".join(f"{k}={v}" for k, v in march.items())] if march else []))
    spec = f"{pose}-{res}:{config}"
    name = f"{pose}-{res}-{quality}" + (f"-{style}" if style else "") + \
        "".join(f"-{k}{v}" for k, v in (march or {}).items())
    camera = None
    if not p.get("pending"):
        camera = {"position": p["position"], "yaw_deg": p["yaw"], "pitch_deg": p["pitch"],
                  "fov_deg": p.get("fov", kit.FOV)}
    return {"spec": spec, "name": name, "pose": pose, "res": res, "config": config,
            "quality": quality, "style": style, "march": march, "resolution": resolution,
            "map": p.get("map"), "camera": camera, "pending": pending,
            "reflection_expected": REFLECTION_EXPECTED[quality],
            # The three scene fields, written through by run.write_scene.
            "reflection_quality": quality, "reflection_style": style, "reflection_march": march}


def with_change(arm, **change):
    a = {k: arm[k] for k in ("pose", "res", "quality", "style", "march")}
    a.update(change)
    return make_arm(a["pose"], a["res"], a["quality"], a["style"], a["march"])


def preset_arms(name, given):
    if name == "tiers":
        # Six tiers at the straight-down and oblique poses at Auto (1536x960, the
        # binding extent); near and the e24 oblique (C) at 4K too, as the 1660
        # cross-check; then off/medium/high at the external monitor extent.
        out = [make_arm(pose, res, q) for pose, res in (("near", "4k"), ("near", "auto"),
                                                        ("probes", "auto"), ("oblique-near", "4k"),
                                                        ("oblique-near", "auto"), ("oblique-probes", "auto"),
                                                        ("oblique-large", "auto"))
               for q in QUALITIES]
        out += [make_arm(pose, "ext", q) for pose in ("near", "oblique-large") for q in ("off", "medium", "high")]
        return out
    if name == "sweep":
        # Reach saturates near steps = 1.07 x divisor (1660 sweep, af8bfef61), so
        # the full-res sweep walks the divisor at saturated steps; Medium walks the
        # fraction at short unsaturated budgets on its preset divisor.
        out = [make_arm("near", "auto", "high", "pixel",
                        {"fraction": 1.0, "steps": math.ceil(1.07 * d), "divisor": d, "thickness": 1.5})
               for d in (135, 180, 270, 360, 540)]
        out += [make_arm("near", "auto", "medium", "pixel", {"fraction": f, "steps": st})
                for f in (0.375, 0.5, 0.75) for st in (48, 64)]
        return out
    if name == "quick":
        # Just-enough set for the Mac's two decisions (Medium, High at Auto):
        # off baselines, tiers at the worst poses, a saturated divisor walk and
        # the Medium fraction bounds. Run with --rounds 1.
        out = [make_arm("near", "auto", q) for q in ("off", "medium", "high", "very_high")]
        out += [make_arm("oblique-large", "auto", q) for q in ("off", "medium", "high")]
        out += [make_arm("oblique-probes", "auto", q) for q in ("off", "high")]
        out += [make_arm("near", "auto", "high", "pixel",
                         {"fraction": 1.0, "steps": math.ceil(1.07 * d), "divisor": d, "thickness": 1.5})
                for d in (135, 270, 540)]
        out += [make_arm("near", "auto", "medium", "pixel", {"fraction": f, "steps": 64}) for f in (0.375, 0.75)]
        return out
    if name == "pilot":
        return [make_arm("near", "auto", "off"), make_arm("near", "auto", "high"),
                make_arm("near", "4k", "ultra"), make_arm("probes", "auto", "medium")]
    if name == "confirm-smooth":
        if not given:
            raise SystemExit("--preset confirm-smooth needs --arms (the finalists)")
        return [with_change(a, style="smooth") for a in given]
    if name == "finalists-ext":
        if not given:
            raise SystemExit("--preset finalists-ext needs --arms (the finalists)")
        return [with_change(a, res="ext") for a in given]
    raise SystemExit(f"unknown preset {name}")


PRESETS = ("tiers", "sweep", "quick", "pilot", "confirm-smooth", "finalists-ext")
TRANSFORM_PRESETS = ("confirm-smooth", "finalists-ext")


def dedupe(arms):
    seen, out = set(), []
    for a in arms:
        if a["name"] not in seen:
            seen.add(a["name"])
            out.append(a)
    return out


def schedule(arms, rounds):
    """ABBA: odd rounds forward, even rounds reversed."""
    return [(rnd, a) for rnd in range(1, rounds + 1) for a in (arms if rnd % 2 else list(reversed(arms)))]


# --- main ------------------------------------------------------------------

def dry_run(arms, plan, args):
    runs = Path("<runs>")
    trace = Path("<trace-dir>")
    for a in arms:
        print(f"=== {a['spec']}  ({a['name']})")
        if a["pending"]:
            print(f"    PENDING: {'; '.join(a['pending'])}"
                  + (f" -- {POSES[a['pose']].get('note')}" if POSES[a['pose']].get("pending") else ""))
            continue
        label = f"{args.tag}-{a['name']}-r1"
        scene = kit.scene_dict(a, runs / f"{a['name']}.png", runs / f"{label}.report.json",
                               args.warmup, args.samples)
        print(json.dumps(scene, indent=2))
        cmd = kit.record_command(trace / f"{label}.trace", runs / f"{label}.log",
                                 runs / f"{label}.scene.json", kit.DRY_RUN_FLAGS)
        print("$ " + shlex.join(cmd))
    print(f"\n--- run order ({len(plan)} launches, {args.rounds} round(s), "
          f"{args.warmup}+{args.samples} frames, {os.environ.get('IDLE_GAP', '15')} s idle before each)")
    for rnd, a in plan:
        print(f"  r{rnd} {a['spec']}" + (f"   [{'; '.join(a['pending'])}]" if a["pending"] else ""))
    pending = sorted({a["spec"] for a in arms if a["pending"]})
    if pending:
        print(f"\n{len(pending)} arm(s) pending; a real run refuses them unless --skip-pending.")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    ap.add_argument("--preset", choices=PRESETS)
    ap.add_argument("--arms")
    ap.add_argument("--pilot", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--skip-pending", action="store_true")
    ap.add_argument("--skip-smoke", action="store_true")
    ap.add_argument("--poses", default=os.environ.get("SSR_POSES"))
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--warmup", type=int, default=120)
    ap.add_argument("--samples", type=int, default=360)
    ap.add_argument("--tag")
    args = ap.parse_args()
    if args.poses:
        load_poses(args.poses)
    if not args.preset and not args.arms:
        ap.error("give --preset or --arms")
    # March k=v pairs contain commas, so --arms splits on commas only between
    # specs: pieces that do not start a new spec are rejoined.
    given = [parse_arm(s) for s in split_specs(args.arms)] if args.arms else []
    if args.preset in TRANSFORM_PRESETS:
        chosen = preset_arms(args.preset, given)
    elif args.preset:
        chosen = preset_arms(args.preset, given) + given
    else:
        chosen = given
    chosen = dedupe(chosen)
    if args.pilot:
        args.rounds = 1
    args.tag = args.tag or args.preset or "arms"
    if args.pilot and not args.tag.endswith("pilot"):
        args.tag += "-pilot"
    plan = schedule(chosen, args.rounds)

    if args.dry_run:
        dry_run(chosen, plan, args)
        return 0

    pending = [a for a in chosen if a["pending"]]
    if pending and not args.skip_pending:
        for a in pending:
            print(f"REFUSED: {a['spec']}: {'; '.join(a['pending'])}", file=sys.stderr)
        print("fill the pose (--poses / POSES) or SSR_EXT_RES, or pass --skip-pending", file=sys.stderr)
        return 2
    if pending:
        print(f"skipping {len(pending)} pending arm(s): {', '.join(a['spec'] for a in pending)}", flush=True)
    chosen = [a for a in chosen if not a["pending"]]
    if not chosen:
        print("no runnable arms", file=sys.stderr)
        return 2
    plan = schedule(chosen, args.rounds)

    kit.RUNS.mkdir(exist_ok=True)
    trace_dir = Path(os.environ.get("TRACE_DIR", Path(tempfile.gettempdir()) / "fluid-ssr-reflections-traces"))
    trace_dir.mkdir(parents=True, exist_ok=True)
    binary = kit.engine_binary()
    problems = []
    if not binary.is_file():
        problems.append(f"engine binary not found: {binary} (set {kit.BIN_ENV}; see README.md)")
    if binary.name != "postretro":
        # gpu_time.py keeps only trace rows whose process is "postretro (pid)".
        problems.append(f"engine binary must be named postretro, got {binary.name}")
    for m in sorted({a["map"] for a in chosen}):
        if not (kit.ROOT / m).is_file():
            problems.append(f"missing map: {m} (compile it with prl-build, see README.md)")
    if kit.busy_processes():
        problems.append(f"another process is running: {kit.busy_processes()}")
    if kit.free_gb(trace_dir) < float(os.environ.get("MIN_FREE_GB", "25")):
        problems.append(f"less than {os.environ.get('MIN_FREE_GB', '25')} GB free in {trace_dir}")
    if subprocess.run(["xcrun", "--find", "xctrace"], capture_output=True).returncode != 0:
        problems.append("xctrace not found (install Xcode or the Command Line Tools)")
    if problems:
        print("\n".join("ABORT: " + p for p in problems), file=sys.stderr)
        return 3

    flags = kit.xctrace_flags()
    caf = subprocess.Popen(["caffeinate", "-d", "-i", "-m", "-s", "-u", "-w", str(os.getpid())])
    awake = threading.Event()

    def redeclare():
        # Re-declare user activity every few seconds so the idle lock never arms.
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
    if any(v > 10 for v in idle["Device Utilization %"]):
        print("WARNING: GPU utilisation is above 10% at idle; close other GPU apps", file=sys.stderr)
    meta = {"git_rev": kit.git("rev-parse", "HEAD"), "git_branch": kit.git("rev-parse", "--abbrev-ref", "HEAD"),
            "binary": str(binary),
            "binary_mtime": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(binary.stat().st_mtime)),
            "machine": " | ".join(filter(None, [
                subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
                subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip()])),
            "preset": args.preset, "pilot": args.pilot, "rounds": args.rounds, "warmup": args.warmup,
            "samples": args.samples, "engine_env": kit.ENGINE_ENV, "ext_res": RESOLUTIONS["ext"],
            "poses": {k: v for k, v in POSES.items()}, "xctrace_flags": flags, "idle": idle,
            "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "adapter": None}

    if not args.skip_smoke:
        # The arm setting the most SSR scene fields: a binary that lacks one
        # rejects the scene here (deny_unknown_fields), not mid-batch.
        probe = max(chosen, key=lambda a: sum(a[k] is not None for k in kit.SCENE_FIELDS))
        print(f"smoke capture ({probe['spec']}) ...", flush=True)
        ok, out = kit.smoke(probe)
        if not ok:
            print("smoke capture failed:\n" + out, file=sys.stderr)
            return 3
    for old in trace_dir.iterdir():
        shutil.rmtree(old, ignore_errors=True) if old.is_dir() else old.unlink()

    records, code = [], 0
    try:
        for rnd, a in plan:
            busy = kit.busy_processes()
            waited = 0
            # Another session's build: wait (never kill), up to 30 min.
            while busy and waited < 1800:
                print(f"waiting: {busy}", flush=True)
                time.sleep(30)
                waited += 30
                busy = kit.busy_processes()
            if busy:
                print(f"ABORT: busy for 30 min: {busy}", file=sys.stderr)
                code = 4
                break
            time.sleep(float(os.environ.get("IDLE_GAP", "15")))
            label = f"{args.tag}-{a['name']}-r{rnd}"
            print(f"run {label} ({a['spec']}) ...", flush=True)
            r = kit.run_one(label, a, rnd, args, trace_dir, flags)
            records.append(r)
            if r.get("adapter") and not meta["adapter"]:
                meta["adapter"] = r["adapter"]
            print(f"  reflection {refl_text(r)} fluid {kit.fmt(r.get('fluid_ms'))} "
                  f"forward {kit.fmt(r.get('forward_ms'))} frame {kit.fmt(r.get('gpu_frame_ms_complete'))} "
                  f"clock {kit.clock_text(r)} vram free {vram_text(r)} "
                  f"valid={r['valid']} {r['problems'] if not r['valid'] else ''}", flush=True)
            if args.pilot and not r["valid"]:
                print(f"ABORT: pilot stopped at {label}: {'; '.join(r['problems'] or ['no GPU data'])}",
                      file=sys.stderr)
                code = 5
                break
            # A machine-state problem invalidates the rest of the interleave too.
            if not r["machine_state_ok"]:
                print(f"ABORT: batch stopped at {label} (machine state)", file=sys.stderr)
                code = 4
                break
    finally:
        if records:
            summarize(records, args.tag, meta)
    return code


def split_specs(text):
    """Split a comma list of specs whose march segment also uses commas: a
    piece that parses as k=v with no ':' continues the previous spec."""
    out = []
    for piece in text.split(","):
        piece = piece.strip()
        if not piece:
            continue
        if out and ":" not in piece and "=" in piece:
            out[-1] += "," + piece
        else:
            out.append(piece)
    return out


# --- summary ---------------------------------------------------------------

def refl_text(r):
    if "Fluid Reflection Pass expected" in " ".join(r.get("anomalies", [])):
        return "ABSENT"
    v = r.get("fluid_reflection_ms")
    return "-" if v is None else f"{v:.3f}"


def vram_text(r):
    pre = (r.get("pre_launch") or {}).get("vramFreeBytes")
    during = ((r.get("clock") or {}).get("vram_free_mb") or {}).get("min")
    pre_text = "n/a" if pre is None else str(pre // (1 << 20))
    return f"{pre_text}/{'n/a' if during is None else during}"


METRICS = (("fluid_reflection", "fluid_reflection_ms"), ("fluid_surface", "fluid_ms"),
           ("forward", "forward_ms"), ("frame", "gpu_frame_ms_complete"))


def stat(values):
    values = [v for v in values if v is not None]
    if not values:
        return None
    return {"mean": sum(values) / len(values), "min": min(values), "max": max(values), "n": len(values),
            "half_range": (max(values) - min(values)) / 2}


def stat_text(s, absent=False):
    if absent:
        return "ABSENT"
    if s is None:
        return "-"
    return f"{s['mean']:.3f}" + (f" ±{s['half_range']:.3f}" if s["n"] > 1 else "")


def refl_cell(s, rs):
    """The arm's fluid_reflection cell: ABSENT when no valid round has it and
    some round was missing it, else the figure with the absent-round count."""
    n = sum(refl_text(r) == "ABSENT" for r in rs)
    if s is None:
        return "ABSENT" if n else "-"
    return stat_text(s) + (f" (ABSENT in {n})" if n else "")


def summarize(records, tag, meta):
    by = {}
    for r in records:
        by.setdefault(r["arm"], []).append(r)
    rows = []
    md = [f"# Fluid SSR reflections: Metal System Trace ({tag})", "",
          f"- Commit `{meta.get('git_rev')}` on `{meta.get('git_branch')}`; binary `{meta.get('binary')}` "
          f"(built {meta.get('binary_mtime')})",
          f"- Machine: {meta.get('machine')}; adapter: {meta.get('adapter')}",
          f"- Capture: {meta['warmup']} warmup + {meta['samples']} sampled frames per run, {meta['rounds']} "
          f"round(s) ABBA, engine env {json.dumps(meta['engine_env'])}, light_term_mask omitted (all terms)",
          f"- Idle before the batch (ioreg IOAccelerator): {json.dumps(meta.get('idle'))}",
          "- Each pass figure is its GPU time per complete steady frame (gpu_time.py `complete`): frames "
          "bounded by the cull-pass frame marker, holding exactly one Depth Pre-Pass, Textured Pass and Fluid "
          "Surface Pass encoder, and one Fluid Reflection Pass encoder when the arm records it. `frame` is "
          "the union of every labelled GPU interval per complete frame. `forward` is Textured Pass. Values are "
          "mean ± half the min-max range across valid rounds. `ABSENT`: the Fluid Reflection Pass was "
          "expected (medium and above) and missing, an anomaly. `-`: the arm records no such pass.",
          "- Clock: busy median core clock (min-max) per run. VRAM free MB: before launch / minimum during "
          "the run, per run.", "",
          "| pose | res | config | valid | fluid_reflection ms | fluid_surface ms | forward ms | "
          "whole-frame GPU ms | clock MHz per run | VRAM free MB per run |",
          "|---|---|---|---|---|---|---|---|---|---|"]
    for name, rs in by.items():
        good = [r for r in rs if r["valid"]]
        r0 = rs[0]
        stats = {m: stat([r.get(k) for r in good]) for m, k in METRICS}
        absent = any("Fluid Reflection Pass expected" in " ".join(r.get("anomalies", [])) for r in rs)
        row = {"arm": name, "spec": r0["spec"], "pose": r0["pose"], "res": r0["res"],
               "resolution": r0["resolution"], "config": r0["config"], "runs": len(rs), "valid": len(good),
               "reflection_absent": absent, **{f"{m}_ms": s for m, s in stats.items()},
               "clocks": [kit.clock_text(r) for r in rs], "vram_free_mb": [vram_text(r) for r in rs]}
        rows.append(row)
        md.append(f"| {r0['pose']} | {r0['res']} {r0['resolution'][0]}x{r0['resolution'][1]} | {r0['config']} | "
                  f"{len(good)}/{len(rs)} | {refl_cell(stats['fluid_reflection'], rs)} | "
                  f"{stat_text(stats['fluid_surface'])} | {stat_text(stats['forward'])} | "
                  f"{stat_text(stats['frame'])} | {'; '.join(row['clocks'])} | {'; '.join(row['vram_free_mb'])} |")
    md += ["", "## Per run", "",
           "| run | round | fluid_reflection ms | fluid_surface ms | forward ms | frame ms | complete/steady frames "
           "| clock MHz | VRAM free MB | valid |", "|---|---|---|---|---|---|---|---|---|---|"]
    for r in records:
        md.append(f"| {r['label']} | {r['round']} | {refl_text(r)} | {kit.fmt(r.get('fluid_ms'))} | "
                  f"{kit.fmt(r.get('forward_ms'))} | {kit.fmt(r.get('gpu_frame_ms_complete'))} | "
                  f"{r.get('complete_frames', 'n/a')}/{r.get('steady_frames', 'n/a')} | {kit.clock_text(r)} | "
                  f"{vram_text(r)} | {'yes' if r['valid'] else 'NO'} |")
    anomalous = [r for r in records if r.get("anomalies")]
    if anomalous:
        md += ["", "## Anomalies", ""] + [f"- `{r['label']}`: {'; '.join(r['anomalies'])}" for r in anomalous]
    bad = [r for r in records if not r["valid"]]
    if bad:
        md += ["", "## Invalid runs (rerun, do not report)", ""]
        md += [f"- `{r['label']}`: {'; '.join(r['problems'] or ['no GPU data'])}" for r in bad]
    out = {"meta": meta, "arms": rows,
           "runs": [{k: v for k, v in r.items() if k not in ("gpu_state_samples", "command")} for r in records]}
    (kit.RESULTS_DIR / f"results-{tag}.json").write_text(json.dumps(out, indent=2) + "\n")
    (kit.RESULTS_DIR / f"results-{tag}.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))


if __name__ == "__main__":
    sys.exit(main())
