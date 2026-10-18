"""Landing parity: per-vertex baked fluid light against today's per-fragment image.

usage: CELL_BIN=<hooked postretro> python3 measurements/fluid-base-pass-cost/metal/parity.py [W H]

For each parity view (near, probes, mid, under), captures three frames through
the engine's --capture path at the same frame count, so the water's motion
matches: `stock` (this branch's shader), `today` (variants/today.wgsl through
the experiment hook) and `today-flat` (the water mask). Reports per-channel max
and mean |today - stock| in 8-bit sRGB over water pixels, and again over water
pixels whose world point lies within one grid spacing of a pool wall. Each
pixel's world point is the view ray's hit on the pool's horizontal surface;
pool rectangles come from the maps' fluid brushes (engine metres). The grid
spacing is read from the engine's install log. Pure Python: no numpy.
Run variants first: python3 measurements/fluid-base-pass-cost/metal/landing_variants.py
"""
import json
import math
import os
import re
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run as kit  # noqa: E402

INCH = 0.0254


def pools(brushes, top_z):
    """Engine-space (x0, x1, z0, z1) of map brushes (x0, x1, y0, y1), at top_z."""
    return {"y": top_z * INCH,
            "rects": [(-y1 * INCH, -y0 * INCH, -x1 * INCH, -x0 * INCH) for x0, x1, y0, y1 in brushes]}


# The surface-level pools of each map (top z in map units); see the .map files.
E24 = pools([(1616, 1872, 64, 320), (1936, 2192, 64, 320), (2256, 2512, 64, 320),
             (1616, 1808, 448, 640), (1808, 2000, 448, 640), (2064, 2512, 448, 576),
             (2384, 2512, 576, 960), (1616, 1872, 768, 960), (1296, 1424, 576, 992),
             (1152, 1408, 128, 384), (640, 896, 128, 384), (128, 256, 128, 384),
             (256, 384, 128, 384)], -16)
PROBES = pools([(160, 352, 160, 352)], 0)
VIEWS = {"near": E24, "test": PROBES, "mid": E24, "under": E24}


def decode_png(path):
    data = Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos, idat, width = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and interlace == 0 and color in (2, 6), (depth, color, interlace)
            channels = 4 if color == 6 else 3
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        rows.append(bytes(line))
        prev = line
    return width, height, channels, rows


def wall_distance(rects, x, z):
    """Distance from (x, z) to the edge of the union of `rects` along the four
    axis directions (exact inside one rectangle); None outside every one."""
    if not any(x0 <= x <= x1 and z0 <= z <= z1 for x0, x1, z0, z1 in rects):
        return None
    best = math.inf
    for axis, sign in ((0, 1), (0, -1), (1, 1), (1, -1)):
        cur = x if axis == 0 else z
        while True:
            reach = None
            for x0, x1, z0, z1 in rects:
                lo, hi, other_lo, other_hi, other = ((x0, x1, z0, z1, z) if axis == 0 else (z0, z1, x0, x1, x))
                probe = cur + sign * 1e-6
                if lo <= probe <= hi and other_lo <= other <= other_hi:
                    edge = hi if sign > 0 else lo
                    if reach is None or (edge - cur) * sign > (reach - cur) * sign:
                        reach = edge
            if reach is None or (reach - cur) * sign <= 0:
                break
            cur = reach
        best = min(best, abs(cur - (x if axis == 0 else z)))
    return best


def capture(binary, pose_name, resolution, variant, out_dir):
    pose = kit.POSES[pose_name]
    png = out_dir / f"{pose_name}-{variant}.png"
    scene = {"map": pose["map"],
             "camera": {"position": pose["position"], "yaw_deg": pose["yaw"],
                        "pitch_deg": pose["pitch"], "fov_deg": kit.FOV},
             "resolution": resolution, "output": str(png),
             "measurement": {"report": str(out_dir / f"{pose_name}-{variant}.report.json"),
                             "warmup_frames": 4, "sample_frames": 4},
             "light_term_mask": 0x13F}
    scene_path = out_dir / f"{pose_name}-{variant}.scene.json"
    scene_path.write_text(json.dumps(scene, indent=2) + "\n")
    env = dict(os.environ, **kit.ENGINE_ENV)
    env.pop("POSTRETRO_FLUID_WGSL_EXPERIMENT", None)
    if variant != "stock":
        env["POSTRETRO_FLUID_WGSL_EXPERIMENT"] = str(HERE / "variants" / f"{variant}.wgsl")
    run = subprocess.run([str(binary), "--capture", str(scene_path)], cwd=kit.ROOT, env=env,
                         capture_output=True, text=True, timeout=600)
    if run.returncode != 0 or not png.is_file():
        raise SystemExit(f"{pose_name} {variant} capture failed:\n{run.stderr[-4000:]}")
    spacing = None
    match = re.search(r"Fluid surfaces installed: .* on a (?:DVec3)?[\[(]([^\])]+)[\])] m grid", run.stderr + run.stdout)
    if match:
        spacing = [float(v) for v in match.group(1).split(",")]
    return png, spacing


def view_dir(pose, w, h, i, j):
    yaw, pitch = math.radians(pose["yaw"]), math.radians(pose["pitch"])
    fwd = (-math.sin(yaw) * math.cos(pitch), math.sin(pitch), -math.cos(yaw) * math.cos(pitch))
    right = (math.cos(yaw), 0.0, -math.sin(yaw))
    up = (right[1] * fwd[2] - right[2] * fwd[1], right[2] * fwd[0] - right[0] * fwd[2],
          right[0] * fwd[1] - right[1] * fwd[0])
    th = math.tan(math.radians(kit.FOV) / 2)
    sx = (2 * (i + 0.5) / w - 1) * th
    sy = (1 - 2 * (j + 0.5) / h) * th * h / w
    return tuple(fwd[k] + sx * right[k] + sy * up[k] for k in range(3))


def compare(pose_name, today, stock, flat, spacing):
    pose, pool = kit.POSES[pose_name], VIEWS[pose_name]
    w, h, ch, rows_t = decode_png(today)
    _, _, ch_s, rows_s = decode_png(stock)
    _, _, ch_f, rows_f = decode_png(flat)
    stats = {"all": [0, [0] * 3, [0] * 3], "wall": [0, [0] * 3, [0] * 3]}
    in_pool = 0
    cam = pose["position"]
    for j in range(h):
        rt, rs, rf = rows_t[j], rows_s[j], rows_f[j]
        for i in range(w):
            f = rf[i * ch_f:i * ch_f + 3]
            if not (f[0] > 200 and f[1] < 80 and f[2] > 200):
                continue
            d = view_dir(pose, w, h, i, j)
            near_wall = False
            if abs(d[1]) > 1e-9:
                t = (pool["y"] - cam[1]) / d[1]
                if t > 0:
                    dist = wall_distance(pool["rects"], cam[0] + t * d[0], cam[2] + t * d[2])
                    if dist is not None:
                        in_pool += 1
                        near_wall = dist <= spacing
            a, b = rt[i * ch:i * ch + 3], rs[i * ch_s:i * ch_s + 3]
            for key in ("all", "wall") if near_wall else ("all",):
                s = stats[key]
                s[0] += 1
                for c in range(3):
                    diff = abs(a[c] - b[c])
                    s[1][c] = max(s[1][c], diff)
                    s[2][c] += diff
    out = {"water_pixels": stats["all"][0], "water_pixels_on_a_mapped_pool": in_pool,
           "image": [w, h]}
    for key, (n, mx, total) in stats.items():
        out[key] = {"pixels": n, "max": mx, "mean": [round(t / n, 3) if n else None for t in total]}
    return out


def main():
    resolution = [int(sys.argv[1]), int(sys.argv[2])] if len(sys.argv) > 2 else [960, 540]
    binary = Path(os.environ["CELL_BIN"])
    out_dir = Path(tempfile.mkdtemp(prefix="fluid-parity-"))
    results = {}
    for pose_name in VIEWS:
        today, _ = capture(binary, pose_name, resolution, "today", out_dir)
        stock, spacing = capture(binary, pose_name, resolution, "stock", out_dir)
        flat, _ = capture(binary, pose_name, resolution, "today-flat", out_dir)
        if spacing is None:
            raise SystemExit(f"{pose_name}: no grid spacing in the install log")
        results[pose_name] = compare(pose_name, today, stock, flat, max(spacing[0], spacing[2]))
        results[pose_name]["spacing_m"] = spacing
        print(pose_name, json.dumps(results[pose_name]), flush=True)
    target = HERE.parent / "results-landing-parity.json"
    target.write_text(json.dumps({"resolution": resolution, "views": results,
                                  "images": str(out_dir)}, indent=2) + "\n")
    print(f"wrote {target}; images in {out_dir}")


if __name__ == "__main__":
    main()
