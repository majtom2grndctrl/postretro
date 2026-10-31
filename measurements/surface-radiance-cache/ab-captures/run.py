"""Capture the `_bounce_fill` A/B at the poses in poses.json, diff them, and build contact sheets.

usage: <venv>/bin/python measurements/surface-radiance-cache/ab-captures/run.py [--skip-capture]
  --skip-capture   reuse raw/*.png; only redo diffs, stats and sheets
env:
  POSTRETRO_BIN    engine binary built with `--features capture` (default target/debug/postretro)

For every pose, fill and mode it writes scenes/<map>-<pose>-<mode>-fill-<f>.json (workspace-relative
paths), runs `<bin> --capture <scene>` from the workspace root with POSTRETRO_SH_STREAMING=sync-proof,
and stores raw/<same>.png. Modes: `full` (no light_term_mask: every term, default settings) and
`indirect` (light_term_mask 0x2: static SH indirect only). Fill pairs differ only in the PRL.
Diffs are fill 1 vs fill 0: per pixel the max over RGB of |fill1 - fill0| in 8-bit sRGB units.
diff/*.png shows it x4 on a heat ramp. stats.json holds mean, p99, max and the share > 2/255,
plus the mean signed luma change. contact-full.png / contact-indirect.png: one row per pose.
"""
import json, os, subprocess, sys
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
REL = HERE.relative_to(ROOT)
RES = [960, 540]
FOV = 100.0
MODES = {"full": None, "indirect": 0x2}
FILLS = {"campaign": ["0", "0.5", "1"], "whiteroom": ["0", "1"]}
AMP = 4
THUMB = (480, 270)


def name(mapkey, pose, mode, fill):
    return f"{mapkey}-{pose}-{mode}-fill-{fill}"


def scene(mapkey, pose, mode, fill):
    cam = pose["camera"]
    s = {"map": f"content/dev/maps/ab-{mapkey}-fill-{fill}.prl",
         "camera": {"position": cam["position"], "yaw_deg": cam["yaw_deg"],
                    "pitch_deg": cam["pitch_deg"], "fov_deg": FOV},
         "resolution": RES, "output": str(REL / "raw" / (name(mapkey, pose["name"], mode, fill) + ".png"))}
    if MODES[mode] is not None:
        s["light_term_mask"] = MODES[mode]
    return s


def capture(scene_path, log_path):
    binary = os.environ.get("POSTRETRO_BIN", str(ROOT / "target/debug/postretro"))
    env = dict(os.environ, POSTRETRO_SH_STREAMING="sync-proof", RUST_LOG="info")
    with open(log_path, "w") as log:
        r = subprocess.run([binary, "--capture", str(scene_path)], cwd=ROOT, env=env, stdout=log,
                           stderr=subprocess.STDOUT)
    if r.returncode != 0:
        raise SystemExit(f"capture failed ({r.returncode}): {scene_path}; see {log_path}")


def load(p):
    return np.asarray(Image.open(p).convert("RGB"), dtype=np.int16)


def heat(v):
    """0..255 -> black, red, yellow, white."""
    t = np.clip(v / 255.0, 0, 1)
    r = np.clip(t * 3, 0, 1); g = np.clip(t * 3 - 1, 0, 1); b = np.clip(t * 3 - 2, 0, 1)
    return (np.stack([r, g, b], -1) * 255).astype(np.uint8)


def diff_stats(a, b):
    d = np.abs(b - a).max(axis=-1)
    luma = lambda x: x @ np.array([0.2126, 0.7152, 0.0722])
    return d, {"mean": round(float(d.mean()), 3), "p99": float(np.percentile(d, 99)), "max": int(d.max()),
               "share_gt2": round(float((d > 2).mean()), 4),
               "mean_signed_luma": round(float((luma(b) - luma(a)).mean()), 3),
               "fill0_mean_luma": round(float(luma(a).mean()), 2)}


def font(size):
    for f in ("/System/Library/Fonts/Menlo.ttc", "/System/Library/Fonts/Supplemental/Arial.ttf"):
        try:
            return ImageFont.truetype(f, size)
        except OSError:
            pass
    return ImageFont.load_default()


def sheet(mode, rows, out):
    cols = 4
    label_h = 40
    head_h = 30
    W = cols * THUMB[0]
    H = head_h + len(rows) * (THUMB[1] + label_h)
    img = Image.new("RGB", (W, H), (24, 24, 24))
    dr = ImageDraw.Draw(img)
    f, fs = font(15), font(13)
    title = (f"_bounce_fill A/B, {'full lighting' if mode == 'full' else 'indirect only (light_term_mask 0x2)'}"
             f" -- columns: fill 0 | fill 0.5 (campaign only) | fill 1 | |fill1-fill0| x{AMP} (max RGB, 8-bit)")
    dr.text((8, 7), title, fill=(230, 230, 230), font=f)
    y = head_h
    for r in rows:
        dr.text((8, y + 4), r["label"], fill=(230, 230, 230), font=fs)
        dr.text((8, y + 21), r["stats_line"], fill=(255, 200, 120), font=fs)
        y += label_h
        for c, p in enumerate(r["cells"]):
            if p is None:
                continue
            im = Image.open(p).convert("RGB").resize(THUMB, Image.LANCZOS)
            img.paste(im, (c * THUMB[0], y))
        y += THUMB[1]
    img.save(out, optimize=True)


def main():
    skip = "--skip-capture" in sys.argv
    poses = json.loads((HERE / "poses.json").read_text())
    (HERE / "scenes").mkdir(exist_ok=True)
    (HERE / "raw").mkdir(exist_ok=True)
    (HERE / "diff").mkdir(exist_ok=True)
    logs = Path(os.environ.get("CAPTURE_LOG_DIR", "/tmp")) / "ab-capture-logs"
    logs.mkdir(parents=True, exist_ok=True)
    stats = {}
    for mode in MODES:
        rows = []
        for mapkey in ("campaign", "whiteroom"):
            for pose in poses[mapkey]["poses"]:
                pngs = {}
                for fill in FILLS[mapkey]:
                    n = name(mapkey, pose["name"], mode, fill)
                    sp = HERE / "scenes" / f"{n}.json"
                    sp.write_text(json.dumps(scene(mapkey, pose, mode, fill), indent=1) + "\n")
                    pngs[fill] = HERE / "raw" / f"{n}.png"
                    if not skip:
                        print("capture", n, flush=True)
                        capture(sp, logs / f"{n}.log")
                a, b = load(pngs["0"]), load(pngs["1"])
                d, st = diff_stats(a, b)
                if "0.5" in pngs:
                    _, st["half_vs_0"] = diff_stats(a, load(pngs["0.5"]))
                dp = HERE / "diff" / f"{mapkey}-{pose['name']}-{mode}-diff-x{AMP}.png"
                Image.fromarray(heat(np.clip(d * AMP, 0, 255))).save(dp, optimize=True)
                stats[f"{mapkey}/{pose['name']}/{mode}"] = st
                cam = pose["camera"]
                rows.append({
                    "label": f"{mapkey} {pose['name']}  cam {cam['position']} yaw {cam['yaw_deg']} pitch {cam['pitch_deg']}",
                    "stats_line": (f"|d| mean {st['mean']:.2f}  p99 {st['p99']:.0f}  max {st['max']}  "
                                   f">2/255 {st['share_gt2'] * 100:.1f}%  signed luma {st['mean_signed_luma']:+.2f}"
                                   f"  (fill0 mean luma {st['fill0_mean_luma']:.1f})"),
                    "cells": [pngs["0"], pngs.get("0.5"), pngs["1"], dp]})
        sheet(mode, rows, HERE / f"contact-{mode}.png")
    (HERE / "stats.json").write_text(json.dumps(stats, indent=1) + "\n")
    for k, v in stats.items():
        print(f"{k:45s} mean {v['mean']:6.2f} p99 {v['p99']:5.0f} max {v['max']:4d} >2: {v['share_gt2'] * 100:5.1f}%"
              f"  signed {v['mean_signed_luma']:+6.2f}  fill0 luma {v['fill0_mean_luma']:6.1f}")


if __name__ == "__main__":
    main()
