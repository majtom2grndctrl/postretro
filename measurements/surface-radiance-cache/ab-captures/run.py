"""Capture the `_bounce_fill` A/B at the poses in poses.json, diff them, and build contact sheets.

usage: <venv>/bin/python measurements/surface-radiance-cache/ab-captures/run.py [group ...] [--skip-capture]
           [--pair <map>/<pose> ...]
  group            main (campaign + white room), movement-feel, movers; default: all
  --skip-capture   reuse raw/*.png; only redo diffs, stats and sheets
  --pair M/P       also write pair-M-P.png: fill 0 | fill 1, full lighting, ~1600 px wide (repeatable)
env:
  POSTRETRO_BIN    engine binary built with `--features capture` (default target/release/postretro)

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
FILLS = {"campaign": ["0", "0.5", "1"], "whiteroom": ["0", "1"], "movement-feel": ["0", "1"],
         "mover-setpiece": ["0", "1"]}
GROUPS = {"main": (["campaign", "whiteroom"], "contact-{mode}.png"),
          "movement-feel": (["movement-feel"], "contact-movement-feel-{mode}.png"),
          "movers": (["mover-setpiece"], "contact-movers-{mode}.png")}
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
    binary = os.environ.get("POSTRETRO_BIN", str(ROOT / "target/release/postretro"))
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
    cols = 4 if any(r["cells"][1] is not None for r in rows) else 3
    label_h = 40
    head_h = 30
    W = cols * THUMB[0]
    H = head_h + len(rows) * (THUMB[1] + label_h)
    img = Image.new("RGB", (W, H), (24, 24, 24))
    dr = ImageDraw.Draw(img)
    f, fs = font(15), font(13)
    middle = "fill 0.5 (campaign only) | " if cols == 4 else ""
    title = (f"_bounce_fill A/B, {'full lighting' if mode == 'full' else 'indirect only (light_term_mask 0x2)'}"
             f" -- columns: fill 0 | {middle}fill 1 | |fill1-fill0| x{AMP} (max RGB, 8-bit)")
    dr.text((8, 7), title, fill=(230, 230, 230), font=f)
    y = head_h
    for r in rows:
        dr.text((8, y + 4), r["label"], fill=(230, 230, 230), font=fs)
        dr.text((8, y + 21), r["stats_line"], fill=(255, 200, 120), font=fs)
        y += label_h
        cells = r["cells"] if cols == 4 else [r["cells"][0], r["cells"][2], r["cells"][3]]
        for c, p in enumerate(cells):
            if p is None:
                continue
            im = Image.open(p).convert("RGB").resize(THUMB, Image.LANCZOS)
            img.paste(im, (c * THUMB[0], y))
        y += THUMB[1]
    img.save(out, optimize=True)


def pair(mapkey, pose_name, st):
    """fill 0 | fill 1 full lighting side by side, 800 px per frame, big labels for a phone."""
    w, h = 800, 450
    a = Image.open(HERE / "raw" / (name(mapkey, pose_name, "full", "0") + ".png")).convert("RGB").resize((w, h), Image.LANCZOS)
    b = Image.open(HERE / "raw" / (name(mapkey, pose_name, "full", "1") + ".png")).convert("RGB").resize((w, h), Image.LANCZOS)
    top, gap = 92, 8
    img = Image.new("RGB", (2 * w + gap, top + h), (20, 20, 20))
    img.paste(a, (0, top)); img.paste(b, (w + gap, top))
    dr = ImageDraw.Draw(img)
    dr.text((12, 8), f"{mapkey} {pose_name}: full lighting", fill=(235, 235, 235), font=font(28))
    dr.text((12, 46), (f"fill 1 vs 0: mean |d| {st['mean']:.1f}/255, p99 {st['p99']:.0f}, "
                       f"{st['share_gt2'] * 100:.0f}% of px > 2/255, mean luma {st['mean_signed_luma']:+.1f}"),
            fill=(255, 200, 120), font=font(24))
    for x, label in ((12, "_bounce_fill 0"), (w + gap + 12, "_bounce_fill 1")):
        dr.rectangle((x - 6, top + 6, x + 262, top + 46), fill=(0, 0, 0))
        dr.text((x, top + 10), label, fill=(255, 255, 255), font=font(28))
    out = HERE / f"pair-{mapkey}-{pose_name}.png"
    img.save(out, optimize=True)
    print("wrote", out)


def main():
    args = sys.argv[1:]
    skip = "--skip-capture" in args
    pairs = [args[i + 1] for i, a in enumerate(args) if a == "--pair"]
    groups = [a for i, a in enumerate(args) if not a.startswith("--") and (i == 0 or args[i - 1] != "--pair")]
    groups = groups or list(GROUPS)
    poses = json.loads((HERE / "poses.json").read_text())
    for sub in ("scenes", "raw", "diff"):
        (HERE / sub).mkdir(exist_ok=True)
    logs = Path(os.environ.get("CAPTURE_LOG_DIR", "/tmp")) / "ab-capture-logs"
    logs.mkdir(parents=True, exist_ok=True)
    stats_path = HERE / "stats.json"
    stats = json.loads(stats_path.read_text()) if stats_path.exists() else {}
    for group in groups:
        mapkeys, sheet_name = GROUPS[group]
        for mode in MODES:
            rows = []
            for mapkey in mapkeys:
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
                    print(f"{mapkey}/{pose['name']}/{mode:9s} mean {st['mean']:6.2f} p99 {st['p99']:5.0f} "
                          f"max {st['max']:4d} >2: {st['share_gt2'] * 100:5.1f}%  signed {st['mean_signed_luma']:+6.2f}"
                          f"  fill0 luma {st['fill0_mean_luma']:6.1f}", flush=True)
            sheet(mode, rows, HERE / sheet_name.format(mode=mode))
    stats_path.write_text(json.dumps(stats, indent=1) + "\n")
    for p in pairs:
        mapkey, pose_name = p.split("/")
        pair(mapkey, pose_name, stats[f"{mapkey}/{pose_name}/full"])


if __name__ == "__main__":
    main()
