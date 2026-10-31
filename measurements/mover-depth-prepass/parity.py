"""Pixel parity: before vs after binaries at each pose, through untraced --capture runs.

usage: BIN_BEFORE=<..>/postretro BIN_AFTER=<..>/postretro MAP_PRL=<mover-setpiece.prl> \
       python3 measurements/mover-depth-prepass/parity.py <out_dir> [pose ...] [--compare-only]

For each pose (run.py's POSES) captures one PNG per binary at 3840x2160 with a
4 + 4 frame measurement (the same frame count for both, so animation matches), then
compares them per pixel in 8-bit RGBA: differing-pixel count, max channel delta, a
delta histogram, the differing pixels' bounding box, and a diff image
(<out_dir>/<pose>-diff.png, grey = 0, brighter = larger max channel delta x 16,
clamped). Writes <out_dir>/parity.json and prints it. Pure Python (no PIL/numpy),
as measurements/fluid-base-pass-cost/metal/parity.py.
"""
import json, os, struct, subprocess, sys, zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run as kit  # noqa: E402


def png_raw(path):
    data = Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos, idat = 8, b""
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
    return width, height, channels, zlib.decompress(idat)


def unfilter(width, height, channels, raw):
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        if f == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 255
        elif f == 2:
            line = bytearray((a + b) & 255 for a, b in zip(line, prev))
        elif f == 3:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 255
        elif f == 4:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        rows.append(bytes(line))
        prev = line
    return rows


def write_gray_png(path, width, height, rows):
    raw = b"".join(b"\x00" + r for r in rows)
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xffffffff)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    Path(path).write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9))
                           + chunk(b"IEND", b""))


def capture(binary, pose, out_dir, tag):
    png = out_dir / f"{pose}-{tag}.png"
    report = out_dir / f"{pose}-{tag}.report.json"
    scene = kit.scene_for(pose, kit.map_path(), png, report, 4, 4)
    sp = out_dir / f"{pose}-{tag}.scene.json"
    sp.write_text(json.dumps(scene, indent=2) + "\n")
    r = subprocess.run([str(binary), "--capture", str(sp)], cwd=kit.ROOT, env=kit.engine_env(),
                       capture_output=True, text=True, timeout=900)
    (out_dir / f"{pose}-{tag}.log").write_text(r.stdout + r.stderr)
    if r.returncode != 0 or not png.is_file():
        raise SystemExit(f"{pose} {tag} capture failed:\n{(r.stdout + r.stderr)[-4000:]}")
    return png


def compare(before, after, diff_path):
    wb, hb, cb, rawb = png_raw(before)
    wa, ha, ca, rawa = png_raw(after)
    assert (wb, hb, cb) == (wa, ha, ca), "image shapes differ"
    out = {"image": [wb, hb], "channels": cb}
    if rawb == rawa:
        out.update(identical=True, differing_pixels=0, max_channel_delta=0)
        return out
    rb, ra = unfilter(wb, hb, cb, rawb), unfilter(wa, ha, ca, rawa)
    hist = {"1": 0, "2-4": 0, "5-16": 0, "17-64": 0, ">64": 0}
    n = maxd = 0
    box = [wb, hb, -1, -1]
    diff_rows = []
    for y in range(hb):
        a, b = ra[y], rb[y]
        if a == b:
            diff_rows.append(bytes(wb))
            continue
        line = bytearray(wb)
        for x in range(wb):
            i = x * cb
            d = max(abs(a[i + k] - b[i + k]) for k in range(cb))
            if d:
                n += 1
                maxd = max(maxd, d)
                line[x] = min(255, 32 + d * 16)
                box = [min(box[0], x), min(box[1], y), max(box[2], x), max(box[3], y)]
                key = "1" if d == 1 else "2-4" if d <= 4 else "5-16" if d <= 16 else "17-64" if d <= 64 else ">64"
                hist[key] += 1
        diff_rows.append(bytes(line))
    write_gray_png(diff_path, wb, hb, diff_rows)
    out.update(identical=False, differing_pixels=n, differing_fraction=n / (wb * hb), max_channel_delta=maxd,
               histogram=hist, bbox_xyxy=box, diff_image=str(diff_path))
    return out


def main():
    args = sys.argv[1:]
    compare_only = "--compare-only" in args
    args = [a for a in args if a != "--compare-only"]
    out_dir = Path(args[0])
    out_dir.mkdir(parents=True, exist_ok=True)
    poses = args[1:] or list(kit.POSES)
    results = {}
    for pose in poses:
        pngs = {}
        for tag, env in kit.BINARIES.items():
            pngs[tag] = (out_dir / f"{pose}-{tag}.png") if compare_only else capture(Path(os.environ[env]), pose, out_dir, tag)
        results[pose] = compare(pngs["before"], pngs["after"], out_dir / f"{pose}-diff.png")
        print(pose, json.dumps(results[pose]), flush=True)
    (out_dir / "parity.json").write_text(json.dumps({"poses": kit.POSES, "results": results}, indent=2) + "\n")


if __name__ == "__main__":
    main()
