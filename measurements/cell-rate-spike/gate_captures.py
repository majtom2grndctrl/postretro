"""Honesty gates for the cell-rate spike, from real-map capture images.

usage: uv run --no-project --with pillow --with numpy python measurements/cell-rate-spike/gate_captures.py
       --main-bin <origin/main postretro built with capture,dev-tools> [--views near-4k,...]

For every view of run.py, captures (warmup 4 + 4 frames, so shimmer and rings
run at a fixed animation time) and compares PNGs:
- cell buffer vs per-fragment pixel style, ratio live and forced to 1: max
  channel difference and pixel counts above 0 and 2 (gate: within 2/255,
  every pixel past it explained);
- the cell buffer with and without its frustum cull: bit-identical;
- the cell buffer's two-draw form (reflection-free surface, then a
  premultiplied reflection overlay) vs its one-draw form: equal up to the
  blend's rounding;
- spike with every lever off vs origin/main, pixel style: bit-identical;
- smooth style, spike (levers off, and cell buffer on) vs origin/main:
  bit-identical. origin/main's capture cannot choose smooth, so its binary
  must carry the one-line local A/B hook `SPIKE_AB_SMOOTH` (findings.md);
- reflection-free variant (lighting mask without the depth-AO bit, which
  shuts every reflecting gate), spike (cell buffer on) vs origin/main:
  bit-identical.
Writes gates.json and gates.md beside this script; difference masks for
pixels past 2/255 go to runs/gate-*.png (local only).
"""
import argparse, json, os, subprocess, sys, tempfile
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run as kit  # noqa: E402

HERE = kit.HERE
# LightTermMask::ALL (0x17F) without bit 0x100 (depth AO): every reflecting
# gate shuts, so every draw takes the reflection-free variant.
REFLECTION_FREE_MASK = 0x17F & ~0x100


def capture(binary, view, extra, out_png, env_extra=None):
    pose, res = view
    p = kit.POSES[pose]
    scene = {
        "map": p["map"],
        "camera": {"position": p["position"], "yaw_deg": p["yaw"], "pitch_deg": p["pitch"], "fov_deg": kit.FOV},
        "resolution": kit.RESOLUTIONS[res],
        "output": str(out_png),
        "measurement": {"report": str(out_png.with_suffix(".report.json")), "warmup_frames": 4, "sample_frames": 4},
        **extra,
    }
    scene_path = out_png.with_suffix(".scene.json")
    scene_path.write_text(json.dumps(scene, indent=2))
    env = kit.engine_env()
    env.update(env_extra or {})
    r = subprocess.run([str(binary), "--capture", str(scene_path)], cwd=kit.ROOT, env=env,
                       capture_output=True, text=True, timeout=900)
    if r.returncode != 0 or not out_png.exists():
        raise RuntimeError(f"capture failed ({binary}, {view}, {extra}):\n{(r.stdout + r.stderr)[-1500:]}")
    return np.asarray(Image.open(out_png).convert("RGBA")).astype(np.int16)


def compare(a, b, mask_png=None):
    d = np.abs(a - b).max(axis=2)
    out = {"pixels": int(d.size), "max_diff": int(d.max()), "over_0": int((d > 0).sum()),
           "over_2": int((d > 2).sum()), "identical": bool((d == 0).all())}
    if mask_png is not None and out["over_2"]:
        Image.fromarray(((d > 2) * 255).astype(np.uint8)).save(mask_png)
        out["mask"] = str(mask_png.relative_to(HERE))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--main-bin", required=True)
    ap.add_argument("--views")
    args = ap.parse_args()
    spike, main_bin = kit.engine_binary(), Path(args.main_bin)
    views = [v for v in kit.views() if not args.views or f"{v[0]}-{v[1]}" in args.views.split(",")]
    kit.RUNS.mkdir(exist_ok=True)
    results = {"spike_bin": str(spike), "main_bin": str(main_bin),
               "spike_rev": kit.git("rev-parse", "HEAD"), "views": {}}
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        for view in views:
            key = f"{view[0]}-{view[1]}"
            print(f"view {key} ...", flush=True)
            shot = lambda name, binary, extra, env=None: capture(binary, view, extra, d / f"{key}-{name}.png", env)
            lever = lambda cells, cull, r1: {"fluid_cell_buffer": cells, "fluid_cell_cull": cull, "fluid_ratio_one": r1}
            img = {
                "frag_live": shot("frag_live", spike, lever(False, False, False)),
                "cell_live": shot("cell_live", spike, lever(True, False, False)),
                "cellcull_live": shot("cellcull_live", spike, lever(True, True, False)),
                "cell2_live": shot("cell2_live", spike, {**lever(True, False, False), "fluid_cell_overlay": True}),
                "frag2_live": shot("frag2_live", spike, {**lever(False, False, False), "fluid_frag_overlay": True}),
                "frag_r1": shot("frag_r1", spike, lever(False, False, True)),
                "cell_r1": shot("cell_r1", spike, lever(True, False, True)),
                "smooth": shot("smooth", spike, {**lever(False, False, False), "fluid_reflection_style": "smooth"}),
                "smooth_cell": shot("smooth_cell", spike, {**lever(True, True, False), "fluid_reflection_style": "smooth"}),
                "free_cell": shot("free_cell", spike, {**lever(True, True, False), "light_term_mask": REFLECTION_FREE_MASK}),
                "main_pixel": shot("main_pixel", main_bin, {}),
                "main_smooth": shot("main_smooth", main_bin, {}, {"SPIKE_AB_SMOOTH": "1"}),
                "main_free": shot("main_free", main_bin, {"light_term_mask": REFLECTION_FREE_MASK}),
            }
            mask = lambda n: kit.RUNS / f"gate-{key}-{n}.png"
            v = {
                "cell_vs_frag_live": compare(img["cell_live"], img["frag_live"], mask("live")),
                "cell_vs_frag_r1": compare(img["cell_r1"], img["frag_r1"], mask("r1")),
                "cellcull_vs_cell_live": compare(img["cellcull_live"], img["cell_live"]),
                "cell2_vs_cell_live": compare(img["cell2_live"], img["cell_live"], mask("cell2")),
                "cell2_vs_frag_live": compare(img["cell2_live"], img["frag_live"]),
                "frag2_vs_frag_live": compare(img["frag2_live"], img["frag_live"], mask("frag2")),
                "spike_frag_vs_main_pixel": compare(img["frag_live"], img["main_pixel"]),
                "spike_smooth_vs_main_smooth": compare(img["smooth"], img["main_smooth"]),
                "spike_smooth_cell_lever_vs_main_smooth": compare(img["smooth_cell"], img["main_smooth"]),
                "spike_free_cell_lever_vs_main_free": compare(img["free_cell"], img["main_free"]),
                "smooth_differs_from_pixel": not compare(img["smooth"], img["frag_live"])["identical"],
            }
            # Keep the round's frag and cell images to eyeball (local only).
            for n in ("frag_live", "cell_live"):
                Image.fromarray(img[n].astype(np.uint8)).save(kit.RUNS / f"gate-{key}-{n}.png")
            results["views"][key] = v
            print(json.dumps(v, indent=1), flush=True)
    (HERE / "gates.json").write_text(json.dumps(results, indent=2) + "\n")
    md = ["# Cell-rate spike: capture-image gates", "",
          f"Spike `{results['spike_rev']}` vs origin/main (`{main_bin.name}`, with the local `SPIKE_AB_SMOOTH` hook). "
          "Every capture renders warmup 4 + 4 frames. Differences are max channel |a - b| on the 8-bit PNG.", "",
          "| view | comparison | max | px > 0 | px > 2 | of px |", "|---|---|---|---|---|---|"]
    for key, v in results["views"].items():
        for name, c in v.items():
            if isinstance(c, dict):
                md.append(f"| {key} | {name} | {c['max_diff']} | {c['over_0']} | {c['over_2']} | {c['pixels']} |")
        md.append(f"| {key} | smooth differs from pixel (sanity) | {v['smooth_differs_from_pixel']} | | | |")
    (HERE / "gates.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))
    return 0


if __name__ == "__main__":
    sys.exit(main())
