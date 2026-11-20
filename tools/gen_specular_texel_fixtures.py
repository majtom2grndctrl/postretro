#!/usr/bin/env python3
"""Generate the exact-value dev fixtures for per-texel specular sharpness and tint.

Writes three 16 x 16 materials into `content/dev/textures/test/` (or `--out`),
each a diffuse plus its slot siblings. The values are the plan of record's
(`context/plans/in-progress/specular-texel-sharpness/plan.md`, Delegated
answers); the map that shows them is `content/dev/maps/specular-texel-sharpness.map`.

* `metal_smoothness_ramp`: a neutral grey checker per texel (sRGB 118 and
  138), so texel edges read. `_s` is 255 everywhere. `_p` column x stores
  round(255 * x / 15): 0, 17, ..., 255, spanning the flat band (bytes 0-24,
  exponent 1), mid values and the cap band (bytes 224 and up, exponent 8192).
* `metal_tint_inlay`: a dark green field (sRGB 28 64 40) with a bronze inlay
  (sRGB 150 96 44): a one-texel square ring on texels 2-13 plus a centred 4 x 4
  block on texels 6-9. `_s` is 255 everywhere. `_t` is 0 on the field, 255 on
  the ring (the bake stores 254) and 128 on the block. No `_p`, so it shades
  at the `metal` prefix's exponent.
* `concrete_carved_gloss`: 4 x 4-texel cells, each a 3 x 3 tile with a
  one-texel mortar line on its low edges (mortar where x % 4 == 0 or
  y % 4 == 0). Diffuse: grey tiles (sRGB 150) and dark mortar (sRGB 56). `_h`
  tiles 200, mortar 56. `_p` tiles 235, mortar 30. `_s` 220 everywhere.

Rows run top to bottom and columns left to right, as PNG stores them. Diffuse
PNGs are 8-bit RGB; every sibling is 8-bit greyscale. No PNG carries an
`sRGB`, `gAMA` or `iCCP` chunk: the siblings must read as linear
(`texture_validation.rs`), and the diffuses need no tag. Exact-value fixtures
stay script-generated here, never hand-authored. Nothing draws randomness.
Standard library only.

Usage:
    python3 tools/gen_specular_texel_fixtures.py
    python3 tools/gen_specular_texel_fixtures.py --out <dir>

Tests: python3 -m unittest discover -s tools/tests -p test_gen_specular_texel_fixtures.py
"""

import argparse
import struct
import sys
import zlib
from pathlib import Path

SIZE = 16

DEFAULT_OUT = Path(__file__).resolve().parents[1] / "content" / "dev" / "textures" / "test"

# metal_smoothness_ramp
RAMP_CHECKER = (118, 138)  # sRGB grey on even and odd (x + y)
RAMP_SPECULAR = 255

# metal_tint_inlay
INLAY_FIELD = (28, 64, 40)
INLAY_BRONZE = (150, 96, 44)
INLAY_RING = (2, 13)  # inclusive bounds of the one-texel square ring
INLAY_BLOCK = (6, 9)  # inclusive bounds of the centred 4 x 4 block
INLAY_SPECULAR = 255
TINT_FIELD = 0
TINT_RING = 255
TINT_BLOCK = 128

# concrete_carved_gloss
CARVED_CELL = 4
CARVED_TILE_RGB = (150, 150, 150)
CARVED_MORTAR_RGB = (56, 56, 56)
CARVED_HEIGHT = {"tile": 200, "mortar": 56}
CARVED_SMOOTHNESS = {"tile": 235, "mortar": 30}
CARVED_SPECULAR = 220


def ramp_smoothness(x):
    """`_p` byte of ramp column x: round(255 * x / 15). 255 / 15 is 17, so the
    division is exact."""
    return 255 * x // 15


def inlay_region(x, y):
    """`field`, `ring` or `block` for one inlay texel."""
    lo, hi = INLAY_BLOCK
    if lo <= x <= hi and lo <= y <= hi:
        return "block"
    lo, hi = INLAY_RING
    if lo <= x <= hi and lo <= y <= hi and (x in (lo, hi) or y in (lo, hi)):
        return "ring"
    return "field"


def carved_region(x, y):
    """`mortar` or `tile` for one carved texel."""
    if x % CARVED_CELL == 0 or y % CARVED_CELL == 0:
        return "mortar"
    return "tile"


def grid(texel):
    """A SIZE x SIZE row-major grid of `texel(x, y)`."""
    return [[texel(x, y) for x in range(SIZE)] for y in range(SIZE)]


def fixtures():
    """Every fixture PNG as {file name: (kind, rows)}: kind `rgb` rows hold
    (r, g, b) tuples, kind `grey` rows hold single bytes."""
    tint = {"field": TINT_FIELD, "ring": TINT_RING, "block": TINT_BLOCK}
    return {
        "metal_smoothness_ramp.png": (
            "rgb",
            grid(lambda x, y: (RAMP_CHECKER[(x + y) % 2],) * 3),
        ),
        "metal_smoothness_ramp_s.png": ("grey", grid(lambda x, y: RAMP_SPECULAR)),
        "metal_smoothness_ramp_p.png": ("grey", grid(lambda x, y: ramp_smoothness(x))),
        "metal_tint_inlay.png": (
            "rgb",
            grid(lambda x, y: INLAY_FIELD if inlay_region(x, y) == "field" else INLAY_BRONZE),
        ),
        "metal_tint_inlay_s.png": ("grey", grid(lambda x, y: INLAY_SPECULAR)),
        "metal_tint_inlay_t.png": ("grey", grid(lambda x, y: tint[inlay_region(x, y)])),
        "concrete_carved_gloss.png": (
            "rgb",
            grid(
                lambda x, y: CARVED_TILE_RGB
                if carved_region(x, y) == "tile"
                else CARVED_MORTAR_RGB
            ),
        ),
        "concrete_carved_gloss_s.png": ("grey", grid(lambda x, y: CARVED_SPECULAR)),
        "concrete_carved_gloss_h.png": (
            "grey",
            grid(lambda x, y: CARVED_HEIGHT[carved_region(x, y)]),
        ),
        "concrete_carved_gloss_p.png": (
            "grey",
            grid(lambda x, y: CARVED_SMOOTHNESS[carved_region(x, y)]),
        ),
    }


def png_bytes(kind, rows):
    """An untagged 8-bit PNG: IHDR, one IDAT, IEND, filter 0 on every row."""
    color_type = {"grey": 0, "rgb": 2}[kind]
    raw = bytearray()
    for row in rows:
        raw.append(0)
        for texel in row:
            raw += bytes(texel) if kind == "rgb" else bytes((texel,))

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    height = len(rows)
    width = len(rows[0])
    header = struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def write_fixtures(out_dir):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    written = []
    for name, (kind, rows) in fixtures().items():
        path = out_dir / name
        path.write_bytes(png_bytes(kind, rows))
        written.append(path)
    return written


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUT,
        help=f"output directory (default: {DEFAULT_OUT})",
    )
    args = parser.parse_args(argv)
    for path in write_fixtures(args.out):
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
