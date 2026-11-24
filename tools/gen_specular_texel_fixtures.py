#!/usr/bin/env python3
"""Generate the exact-value dev fixtures for per-texel specular sharpness and tint.

Writes twelve 16 x 16 materials into `content/dev/textures/test/` (or `--out`),
each a diffuse plus any slot siblings. The first five cover per-texel
sharpness and tint; of them, the last two cover `_p` on a prefix with no
Surface Depth tuning, and `_p`, `_t` and `_h` together. The map that shows
them is `content/dev/maps/specular-texel-sharpness.map`. The next four
art-direct glossy world reflections through `_p` and `_s` alone, in the
side rooms of `content/dev/maps/glossy-reflections-test.map`. The last three
dress `content/dev/maps/glossy-reflections-showcase.map`: a sharp and a rough
tinted floor that reflect strongly, and a matte striped subject. Exponents
below are n = 2 / r^4 - 2 at r = 1 - p / 255; a texel reflects only where
n > 8 and `_s` > 0, at a weight fading in over n 8-16 (bytes 85-107).

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
* `glass_gloss_tiles`: the carved cell lattice on `glass`, the one prefix whose
  Surface Depth resolves flat. Diffuse: pale glass tiles (sRGB 140 180 190)
  and dark came (sRGB 44 48 52). `_s` 255. `_h` tiles 200, came 56: a real
  relief the march must still ignore. `_p` checkers the cells: tiles in cells
  whose (x // 4 + y // 4) is even store 235 (the 8192 cap), odd 120 (about
  exponent 23); came 30 (near exponent 1). Without `_p` the prefix would shade
  at 96.
* `concrete_carved_tinted_gloss`: the carved cell lattice with `_p`, `_t` and
  `_h` together. Diffuse: cyan tiles (sRGB 60 140 170) and copper mortar
  (sRGB 110 60 34). `_s` 220. `_h` tiles 200, mortar 56. `_p` by cell row:
  tiles in cell rows 0 and 2 (y // 4 even) store 235, rows 1 and 3 store 120;
  mortar 30. `_t` by cell column, mortar included: columns 0 and 2 (x // 4
  even) store 255 (the bake stores 254), columns 1 and 3 store 0. So one
  albedo glints in its own hue on the 255 columns and white on the 0 columns
  (t = 0 is white, not the raw-albedo multiply of a slot without `_t`), at
  each sharpness.
* `concrete_satin`: warm grey 16 x 16 panels (sRGB 168 158 142) with a
  one-texel darker seam (sRGB 104 98 90) on the low edges (x == 0 or
  y == 0). `_s` 255 and `_p` 64 (about exponent 4.4) everywhere: a broad
  highlight under the reflection floor, so it never reflects.
* `metal_polished`: a dark steel checker by 4-texel cell (sRGB 64 68 74 where
  x // 4 + y // 4 is even, 80 84 90 where odd). `_s` 255, `_p` 200 (about
  exponent 922, past the fade band: full weight, minimum blur) and `_t` 0
  (white highlight and reflection) everywhere.
* `metal_fade_band`: `metal_polished` with `_p` 97 everywhere (about exponent
  11.6, mid fade band: about half weight, blurrier).
* `concrete_tile_grout`: 8-texel cells, each a 7 x 7 dark blue tile (sRGB 36
  44 60) with a one-texel pale grout line (sRGB 150 146 138) on its low edges
  (grout where x % 8 == 0 or y % 8 == 0). `_s` tiles 255, grout 0. `_p` 200
  and `_t` 0 everywhere, so only `_s` keeps the grout from reflecting.
* `metal_mirror_tinted`: a neutral grey checker by 4-texel cell (sRGB 70
  where x // 4 + y // 4 is even, 96 where odd). `_s` 255, `_p` 200 (about
  exponent 922: full weight, minimum blur) and `_t` 224 everywhere. The tint
  raises the head-on reflectance from 0.04 to about 0.88, and on a neutral
  albedo it leaves the reflection's colour white.
* `metal_satin_tinted`: `metal_mirror_tinted` with `_p` 115 everywhere (about
  exponent 20: past the fade band, so full weight, but blurrier than the
  minimum blur, about mip 3.4).
* `concrete_signal_stripes`: a matte striped subject, diffuse only (no `_s`,
  no `_p`, so it never reflects). Rows 0-3 a 2-texel white and black checker
  (sRGB 236 232 220 and 18 18 22); rows 4-6 orange (236 112 20); rows 8-10
  cyan (24 176 216); rows 12-14 white; rows 7, 11 and 15 black.

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

# glass_gloss_tiles
GLASS_TILE_RGB = (140, 180, 190)
GLASS_CAME_RGB = (44, 48, 52)
GLASS_SPECULAR = 255
GLASS_HEIGHT = {"tile": 200, "mortar": 56}
GLASS_SMOOTHNESS = {"sharp": 235, "broad": 120, "mortar": 30}

# concrete_carved_tinted_gloss
TINTED_TILE_RGB = (60, 140, 170)
TINTED_MORTAR_RGB = (110, 60, 34)
TINTED_SPECULAR = 220
TINTED_HEIGHT = {"tile": 200, "mortar": 56}
TINTED_SMOOTHNESS = {"sharp": 235, "broad": 120, "mortar": 30}
TINTED_TINT = {"tinted": 255, "untinted": 0}

# concrete_satin
SATIN_PANEL_RGB = (168, 158, 142)
SATIN_SEAM_RGB = (104, 98, 90)
SATIN_SPECULAR = 255
SATIN_SMOOTHNESS = 64  # about exponent 4.4, under the reflection floor

# metal_polished and metal_fade_band: one diffuse, two sharpnesses
POLISHED_CHECKER = ((64, 68, 74), (80, 84, 90))  # even and odd (x // 4 + y // 4)
POLISHED_SPECULAR = 255
POLISHED_SMOOTHNESS = 200  # about exponent 922: full weight
FADE_BAND_SMOOTHNESS = 97  # about exponent 11.6: mid fade band
POLISHED_TINT = 0

# concrete_tile_grout
TILE_CELL = 8
TILE_RGB = (36, 44, 60)
GROUT_RGB = (150, 146, 138)
TILE_SPECULAR = {"tile": 255, "grout": 0}
TILE_SMOOTHNESS = 200
TILE_TINT = 0

# metal_mirror_tinted and metal_satin_tinted: one diffuse, two sharpnesses
MIRROR_CHECKER = ((70, 70, 70), (96, 96, 96))  # even and odd (x // 4 + y // 4)
MIRROR_SPECULAR = 255
MIRROR_SMOOTHNESS = 200  # about exponent 922: full weight, minimum blur
SATIN_TINTED_SMOOTHNESS = 115  # about exponent 20: full weight, about mip 3.4
MIRROR_TINT = 224  # head-on reflectance about 0.88

# concrete_signal_stripes
STRIPE_WHITE = (236, 232, 220)
STRIPE_BLACK = (18, 18, 22)
STRIPE_ORANGE = (236, 112, 20)
STRIPE_CYAN = (24, 176, 216)


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


def glass_smoothness_region(x, y):
    """`mortar`, `sharp` or `broad` for one glass texel: tiles checker by cell."""
    if carved_region(x, y) == "mortar":
        return "mortar"
    return "sharp" if (x // CARVED_CELL + y // CARVED_CELL) % 2 == 0 else "broad"


def tinted_smoothness_region(x, y):
    """`mortar`, `sharp` or `broad` for one tinted texel: tiles by cell row."""
    if carved_region(x, y) == "mortar":
        return "mortar"
    return "sharp" if (y // CARVED_CELL) % 2 == 0 else "broad"


def tinted_tint_region(x, y):
    """`tinted` or `untinted` for one tinted texel: by cell column."""
    return "tinted" if (x // CARVED_CELL) % 2 == 0 else "untinted"


def satin_region(x, y):
    """`seam` or `panel` for one satin texel."""
    return "seam" if x == 0 or y == 0 else "panel"


def polished_rgb(x, y):
    """The polished and fade-band diffuse: a checker by 4-texel cell."""
    return POLISHED_CHECKER[(x // 4 + y // 4) % 2]


def tile_region(x, y):
    """`grout` or `tile` for one tile-and-grout texel."""
    if x % TILE_CELL == 0 or y % TILE_CELL == 0:
        return "grout"
    return "tile"


def mirror_rgb(x, y):
    """The mirror and satin tinted diffuse: a neutral checker by 4-texel cell."""
    return MIRROR_CHECKER[(x // 4 + y // 4) % 2]


def stripe_rgb(x, y):
    """The signal-stripes diffuse: a checker band, then orange, cyan and
    white bands, each closed by a black row."""
    if y < 4:
        return STRIPE_WHITE if (x // 2 + y // 2) % 2 == 0 else STRIPE_BLACK
    if y % 4 == 3:
        return STRIPE_BLACK
    return {1: STRIPE_ORANGE, 2: STRIPE_CYAN, 3: STRIPE_WHITE}[y // 4]


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
        "glass_gloss_tiles.png": (
            "rgb",
            grid(
                lambda x, y: GLASS_TILE_RGB
                if carved_region(x, y) == "tile"
                else GLASS_CAME_RGB
            ),
        ),
        "glass_gloss_tiles_s.png": ("grey", grid(lambda x, y: GLASS_SPECULAR)),
        "glass_gloss_tiles_h.png": (
            "grey",
            grid(lambda x, y: GLASS_HEIGHT[carved_region(x, y)]),
        ),
        "glass_gloss_tiles_p.png": (
            "grey",
            grid(lambda x, y: GLASS_SMOOTHNESS[glass_smoothness_region(x, y)]),
        ),
        "concrete_carved_tinted_gloss.png": (
            "rgb",
            grid(
                lambda x, y: TINTED_TILE_RGB
                if carved_region(x, y) == "tile"
                else TINTED_MORTAR_RGB
            ),
        ),
        "concrete_carved_tinted_gloss_s.png": ("grey", grid(lambda x, y: TINTED_SPECULAR)),
        "concrete_carved_tinted_gloss_h.png": (
            "grey",
            grid(lambda x, y: TINTED_HEIGHT[carved_region(x, y)]),
        ),
        "concrete_carved_tinted_gloss_p.png": (
            "grey",
            grid(lambda x, y: TINTED_SMOOTHNESS[tinted_smoothness_region(x, y)]),
        ),
        "concrete_carved_tinted_gloss_t.png": (
            "grey",
            grid(lambda x, y: TINTED_TINT[tinted_tint_region(x, y)]),
        ),
        "concrete_satin.png": (
            "rgb",
            grid(
                lambda x, y: SATIN_SEAM_RGB
                if satin_region(x, y) == "seam"
                else SATIN_PANEL_RGB
            ),
        ),
        "concrete_satin_s.png": ("grey", grid(lambda x, y: SATIN_SPECULAR)),
        "concrete_satin_p.png": ("grey", grid(lambda x, y: SATIN_SMOOTHNESS)),
        "metal_polished.png": ("rgb", grid(polished_rgb)),
        "metal_polished_s.png": ("grey", grid(lambda x, y: POLISHED_SPECULAR)),
        "metal_polished_p.png": ("grey", grid(lambda x, y: POLISHED_SMOOTHNESS)),
        "metal_polished_t.png": ("grey", grid(lambda x, y: POLISHED_TINT)),
        "metal_fade_band.png": ("rgb", grid(polished_rgb)),
        "metal_fade_band_s.png": ("grey", grid(lambda x, y: POLISHED_SPECULAR)),
        "metal_fade_band_p.png": ("grey", grid(lambda x, y: FADE_BAND_SMOOTHNESS)),
        "metal_fade_band_t.png": ("grey", grid(lambda x, y: POLISHED_TINT)),
        "concrete_tile_grout.png": (
            "rgb",
            grid(lambda x, y: TILE_RGB if tile_region(x, y) == "tile" else GROUT_RGB),
        ),
        "concrete_tile_grout_s.png": (
            "grey",
            grid(lambda x, y: TILE_SPECULAR[tile_region(x, y)]),
        ),
        "concrete_tile_grout_p.png": ("grey", grid(lambda x, y: TILE_SMOOTHNESS)),
        "concrete_tile_grout_t.png": ("grey", grid(lambda x, y: TILE_TINT)),
        "metal_mirror_tinted.png": ("rgb", grid(mirror_rgb)),
        "metal_mirror_tinted_s.png": ("grey", grid(lambda x, y: MIRROR_SPECULAR)),
        "metal_mirror_tinted_p.png": ("grey", grid(lambda x, y: MIRROR_SMOOTHNESS)),
        "metal_mirror_tinted_t.png": ("grey", grid(lambda x, y: MIRROR_TINT)),
        "metal_satin_tinted.png": ("rgb", grid(mirror_rgb)),
        "metal_satin_tinted_s.png": ("grey", grid(lambda x, y: MIRROR_SPECULAR)),
        "metal_satin_tinted_p.png": ("grey", grid(lambda x, y: SATIN_TINTED_SMOOTHNESS)),
        "metal_satin_tinted_t.png": ("grey", grid(lambda x, y: MIRROR_TINT)),
        "concrete_signal_stripes.png": ("rgb", grid(stripe_rgb)),
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
