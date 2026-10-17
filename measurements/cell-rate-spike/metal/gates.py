"""Capture-image honesty gates and per-pose stats for the cell-rate spike (Mac).

usage: python3 measurements/cell-rate-spike/metal/gates.py [--only gate,...] [--configs a,b]
env:
  CELL_BIN   spike engine binary (default target/release/postretro)
  MAIN_BIN   origin/main engine binary built with --features capture,dev-tools
             (required for the main-identity gates)

Untraced single-frame captures (no measurement block, so animation time is 0
and the output frame is deterministic), compared per channel after decoding
the PNG through `sips` to an uncompressed BMP. Gates:
- parity:  per-fragment pixel style vs cell buffer vs cell buffer + cull, at
           every config of run.py; reports the max channel difference and the
           pixels over 2/255.
- ratio1:  the same with the ratio forced to 1 in both paths.
- parity-t: per-fragment vs cell buffer at nonzero animation times (a
           measurement block before the output PNG, TIMED_PARITY).
- main:    spike binary, levers off, vs main's binary, pixel style: bit-identical.
- free:    reflection-free variant (light_term_mask without the specular bit,
           which shuts the reflecting gate), spike with every lever on vs main:
           bit-identical.
- smooth:  smooth style, cell buffer on vs off (spike): bit-identical.
- stale:   a capture that renders `pre_camera` first and then jumps to the pose
           vs a fresh capture at the pose, cells and cells + cull.
- stats:   fluid_reflection_cell_stats per config, cells and cells + cull.
Results: gates.json and gates.md beside this file; PNGs stay in gate-runs/.
"""
import argparse, json, os, struct, subprocess, sys, tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run as kit  # noqa: E402  (poses, configs)

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OUT = HERE / "gate-runs"
ENV = {"POSTRETRO_SH_STREAMING": "sync-proof", "RUST_LOG": "warn"}
# LightTermMask::ALL is 0x17F (fluid_surface.wgsl FLUID_REFLECTION_ALL_TERMS);
# dropping the specular bit (LightTermMask::SPECULAR, 1 << 6), which the fluid
# never reads, keeps the fluid's lit terms and shuts the reflecting gate, so
# every fluid draw takes the reflection-free variant.
MASK_NO_SPECULAR = 0x17F & ~0x40
TOLERANCE = 2
# (warmup, samples) frames before the output PNG for the parity-t gate; 4 + 4
# matches the GTX 1660 run's gate captures (claude/cell-rate-spike-1660).
TIMED_PARITY = ((4, 4), (30, 30), (120, 120))


def binary(name, default=None):
    v = os.environ.get(name)
    return Path(v) if v else default


def bmp_pixels(png):
    with tempfile.TemporaryDirectory() as d:
        bmp = Path(d) / "x.bmp"
        subprocess.run(["sips", "-s", "format", "bmp", str(png), "--out", str(bmp)],
                       check=True, capture_output=True)
        data = bmp.read_bytes()
    offset = struct.unpack_from("<I", data, 10)[0]
    width, height = struct.unpack_from("<ii", data, 18)
    bpp = struct.unpack_from("<H", data, 28)[0]
    return width, abs(height), bpp // 8, data[offset:]


def diff(a_png, b_png):
    wa, ha, ca, a = bmp_pixels(a_png)
    wb, hb, cb, b = bmp_pixels(b_png)
    if (wa, ha, ca) != (wb, hb, cb):
        return {"comparable": False, "a": [wa, ha, ca], "b": [wb, hb, cb]}
    if a == b:
        return {"comparable": True, "identical": True, "max": 0, "pixels_over": 0, "pixels": wa * ha}
    stride = ((wa * ca + 3) // 4) * 4
    worst, over, differing = 0, 0, 0
    for y in range(ha):
        ra = a[y * stride:y * stride + wa * ca]
        rb = b[y * stride:y * stride + wa * ca]
        if ra == rb:
            continue
        for x in range(wa):
            pa = ra[x * ca:x * ca + 3]
            pb = rb[x * ca:x * ca + 3]
            if pa == pb:
                continue
            m = max(abs(pa[0] - pb[0]), abs(pa[1] - pb[1]), abs(pa[2] - pb[2]))
            differing += 1
            worst = max(worst, m)
            if m > TOLERANCE:
                over += 1
    return {"comparable": True, "identical": False, "max": worst, "pixels_differing": differing,
            "pixels_over": over, "pixels": wa * ha}


def capture(bin_path, label, pose, resolution, extra, stats=False, measure=None):
    OUT.mkdir(exist_ok=True)
    png = OUT / f"{label}.png"
    scene = {"map": pose["map"],
             "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                        "pitch_deg": pose["pitch"], "fov_deg": kit.FOV},
             "resolution": resolution, "output": str(png), **extra}
    report = None
    if stats or measure:
        # A measurement block renders the output PNG at the sequence's last
        # animation step (capture/prepared.rs capture_measurement_output_frame);
        # an ordinary capture renders at time 0.
        warmup, samples = measure or (8, 8)
        report = OUT / f"{label}.report.json"
        scene["measurement"] = {"report": str(report), "warmup_frames": warmup, "sample_frames": samples}
    if stats:
        scene["fluid_reflection_cell_stats"] = True
    scene_path = OUT / f"{label}.scene.json"
    scene_path.write_text(json.dumps(scene, indent=2) + "\n")
    png.unlink(missing_ok=True)
    env = {**os.environ, **ENV}
    env.pop("WGPU_VALIDATION_INDIRECT_CALL", None)
    r = subprocess.run([str(bin_path), "--capture", str(scene_path)], cwd=ROOT, env=env,
                       capture_output=True, text=True, timeout=900)
    if r.returncode != 0 or not png.exists():
        raise RuntimeError(f"capture {label} failed ({r.returncode}):\n{(r.stdout + r.stderr)[-3000:]}")
    return png, (json.loads(report.read_text()) if report else None)


def levers(cells=False, cull=False, ratio_one=False):
    return {"fluid_reflection_cells": {"cells": cells, "frustum_cull": cull, "ratio_one": ratio_one}}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only")
    ap.add_argument("--configs")
    args = ap.parse_args()
    only = set(args.only.split(",")) if args.only else None
    configs = [c for c in kit.CONFIGS if not args.configs or c[0] in args.configs.split(",")]
    spike = binary("CELL_BIN", ROOT / "target/release/postretro")
    main_bin = binary("MAIN_BIN")
    results = json.loads((HERE / "gates.json").read_text()) if (HERE / "gates.json").exists() else {}

    def want(g):
        return only is None or g in only

    for config, pose_name, res in configs:
        pose = kit.POSES[pose_name]
        out = results.setdefault(config, {"pose": pose, "resolution": res})
        print(f"== {config}", flush=True)
        if want("parity") or want("ratio1"):
            for ratio_one in ([False] if not want("ratio1") else [True] if not want("parity") else [False, True]):
                tag = "-r1" if ratio_one else ""
                frag, _ = capture(spike, f"{config}-frag{tag}", pose, res, levers(ratio_one=ratio_one))
                cells, _ = capture(spike, f"{config}-cells{tag}", pose, res, levers(True, False, ratio_one))
                cull, _ = capture(spike, f"{config}-cull{tag}", pose, res, levers(True, True, ratio_one))
                out[f"parity{tag}"] = {"frag_vs_cells": diff(frag, cells), "frag_vs_cull": diff(frag, cull),
                                       "cells_vs_cull": diff(cells, cull)}
                print(" parity" + tag, json.dumps(out[f"parity{tag}"]), flush=True)
        if want("parity-t"):
            # Parity away from animation time 0: the shimmer step moves which
            # cells per-fragment pixel style speckles inside.
            out["parity-t"] = {}
            for w, n in TIMED_PARITY:
                tag = f"t{w + n}"
                frag, _ = capture(spike, f"{config}-frag-{tag}", pose, res, levers(), measure=(w, n))
                cells, _ = capture(spike, f"{config}-cells-{tag}", pose, res, levers(True), measure=(w, n))
                out["parity-t"][tag] = diff(frag, cells)
            print(" parity-t", json.dumps(out["parity-t"]), flush=True)
        if want("main") and main_bin:
            a, _ = capture(spike, f"{config}-spike-default", pose, res, {})
            b, _ = capture(main_bin, f"{config}-main-default", pose, res, {})
            out["main"] = diff(a, b)
            print(" main", json.dumps(out["main"]), flush=True)
        if want("free") and main_bin:
            a, _ = capture(spike, f"{config}-spike-free", pose, res,
                           {"light_term_mask": MASK_NO_SPECULAR, **levers(True, True, True)})
            b, _ = capture(main_bin, f"{config}-main-free", pose, res, {"light_term_mask": MASK_NO_SPECULAR})
            out["free"] = diff(a, b)
            print(" free", json.dumps(out["free"]), flush=True)
        if want("smooth"):
            a, _ = capture(spike, f"{config}-smooth-frag", pose, res, {"fluid_reflection_style": "smooth"})
            b, _ = capture(spike, f"{config}-smooth-cells", pose, res,
                           {"fluid_reflection_style": "smooth", **levers(True, True)})
            out["smooth"] = diff(a, b)
            print(" smooth", json.dumps(out["smooth"]), flush=True)
        if want("stale"):
            # Jump from a pose turned 90 degrees and raised, so most cells were
            # off screen (cull) or seen differently before the move.
            pre = {"position": [pose["position"][0] + 1.0, pose["position"][1] + 0.5, pose["position"][2] - 1.0],
                   "yaw_deg": pose["yaw"] + 90.0, "pitch_deg": max(pose["pitch"] + 30.0, -89.0), "fov_deg": kit.FOV}
            out["stale"] = {}
            for name, lv in (("cells", levers(True)), ("cull", levers(True, True))):
                fresh, _ = capture(spike, f"{config}-fresh-{name}", pose, res, lv)
                moved, _ = capture(spike, f"{config}-moved-{name}", pose, res, {**lv, "pre_camera": pre})
                out["stale"][name] = diff(fresh, moved)
            print(" stale", json.dumps(out["stale"]), flush=True)
        if want("stats"):
            out["stats"] = {}
            for name, lv in (("cells", levers(True)), ("cull", levers(True, True))):
                _, rep = capture(spike, f"{config}-stats-{name}", pose, res, lv, stats=True)
                out["stats"][name] = rep.get("fluid_reflection_cells")
            print(" stats", json.dumps(out["stats"]), flush=True)
        (HERE / "gates.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
