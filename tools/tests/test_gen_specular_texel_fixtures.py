"""Tests for gen_specular_texel_fixtures.py: every texel byte of every fixture.

Expected values are spelled out here from the fixture rules, not read from
the generator, and the PNGs are decoded by a reader independent of its writer.
Both the committed files under content/dev/textures/test/ and a fresh run are
checked.

Run with `python3 -m unittest discover -s tools/tests -p test_gen_specular_texel_fixtures.py`.
"""

import contextlib
import importlib.util
import io
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COMMITTED = REPO / "content" / "dev" / "textures" / "test"


def load_module():
    script = Path(__file__).parents[1] / "gen_specular_texel_fixtures.py"
    spec = importlib.util.spec_from_file_location("gen_specular_texel_fixtures_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GEN = load_module()

SIZE = 16

# --- Expected texels, written out by hand from the fixture rules in the generator's docstring. ---------

RAMP_P_ROW = [0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255]

# metal_tint_inlay regions: . field, R ring, B block.
INLAY_MAP = [
    "................",
    "................",
    "..RRRRRRRRRRRR..",
    "..R..........R..",
    "..R..........R..",
    "..R..........R..",
    "..R...BBBB...R..",
    "..R...BBBB...R..",
    "..R...BBBB...R..",
    "..R...BBBB...R..",
    "..R..........R..",
    "..R..........R..",
    "..R..........R..",
    "..RRRRRRRRRRRR..",
    "................",
    "................",
]

# concrete_carved_gloss regions: M mortar, T tile.
CARVED_ROW_MORTAR = "M" * 16
CARVED_ROW_TILE = "MTTTMTTTMTTTMTTT"
CARVED_MAP = [CARVED_ROW_MORTAR if y % 4 == 0 else CARVED_ROW_TILE for y in range(16)]

# glass_gloss_tiles `_p` regions: M came, S sharp tile, B broad tile. The
# tiles checker by 4-texel cell.
GLASS_P_MAP = [
    "MMMMMMMMMMMMMMMM",
    "MSSSMBBBMSSSMBBB",
    "MSSSMBBBMSSSMBBB",
    "MSSSMBBBMSSSMBBB",
    "MMMMMMMMMMMMMMMM",
    "MBBBMSSSMBBBMSSS",
    "MBBBMSSSMBBBMSSS",
    "MBBBMSSSMBBBMSSS",
    "MMMMMMMMMMMMMMMM",
    "MSSSMBBBMSSSMBBB",
    "MSSSMBBBMSSSMBBB",
    "MSSSMBBBMSSSMBBB",
    "MMMMMMMMMMMMMMMM",
    "MBBBMSSSMBBBMSSS",
    "MBBBMSSSMBBBMSSS",
    "MBBBMSSSMBBBMSSS",
]

# concrete_carved_tinted_gloss `_p` regions: M mortar, S sharp tile, B broad
# tile. Sharpness alternates by cell row.
TINTED_P_MAP = [
    "MMMMMMMMMMMMMMMM",
    "MSSSMSSSMSSSMSSS",
    "MSSSMSSSMSSSMSSS",
    "MSSSMSSSMSSSMSSS",
    "MMMMMMMMMMMMMMMM",
    "MBBBMBBBMBBBMBBB",
    "MBBBMBBBMBBBMBBB",
    "MBBBMBBBMBBBMBBB",
    "MMMMMMMMMMMMMMMM",
    "MSSSMSSSMSSSMSSS",
    "MSSSMSSSMSSSMSSS",
    "MSSSMSSSMSSSMSSS",
    "MMMMMMMMMMMMMMMM",
    "MBBBMBBBMBBBMBBB",
    "MBBBMBBBMBBBMBBB",
    "MBBBMBBBMBBBMBBB",
]

# concrete_carved_tinted_gloss `_t` regions: T tinted, U untinted. Tint
# alternates by cell column, mortar included.
TINTED_T_MAP = ["TTTTUUUUTTTTUUUU"] * 16

# concrete_satin regions: S seam, P panel. The seam runs along row 0 and
# column 0.
SATIN_MAP = ["S" * 16] + ["S" + "P" * 15] * 15

# metal_polished and metal_fade_band diffuse: A even cell, B odd cell, by
# 4-texel cell.
POLISHED_MAP = [
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "AAAABBBBAAAABBBB",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
    "BBBBAAAABBBBAAAA",
]

# concrete_tile_grout regions: G grout, T tile, by 8-texel cell.
TILE_ROW_GROUT = "G" * 16
TILE_ROW_TILE = "GTTTTTTTGTTTTTTT"
TILE_MAP = [TILE_ROW_GROUT if y % 8 == 0 else TILE_ROW_TILE for y in range(16)]


# metal_mirror_tinted and metal_satin_tinted diffuse: the polished checker's
# cells, neutral grey.
MIRROR_MAP = POLISHED_MAP

# concrete_signal_stripes: W white, K black, O orange, C cyan. Rows 0-3 a
# 2-texel checker, then orange, cyan and white bands each closed by black.
STRIPE_CHECKER_A = "WWKKWWKKWWKKWWKK"
STRIPE_CHECKER_B = "KKWWKKWWKKWWKKWW"
STRIPE_MAP = (
    [STRIPE_CHECKER_A] * 2
    + [STRIPE_CHECKER_B] * 2
    + ["O" * 16] * 3
    + ["K" * 16]
    + ["C" * 16] * 3
    + ["K" * 16]
    + ["W" * 16] * 3
    + ["K" * 16]
)


def from_map(region_map, values):
    return [[values[c] for c in row] for row in region_map]


def uniform(value):
    return [[value] * SIZE for _ in range(SIZE)]


EXPECTED = {
    "metal_smoothness_ramp.png": (
        "rgb",
        [[(118,) * 3 if (x + y) % 2 == 0 else (138,) * 3 for x in range(SIZE)] for y in range(SIZE)],
    ),
    "metal_smoothness_ramp_s.png": ("grey", uniform(255)),
    "metal_smoothness_ramp_p.png": ("grey", [list(RAMP_P_ROW) for _ in range(SIZE)]),
    "metal_tint_inlay.png": (
        "rgb",
        from_map(INLAY_MAP, {".": (28, 64, 40), "R": (150, 96, 44), "B": (150, 96, 44)}),
    ),
    "metal_tint_inlay_s.png": ("grey", uniform(255)),
    "metal_tint_inlay_t.png": ("grey", from_map(INLAY_MAP, {".": 0, "R": 255, "B": 128})),
    "concrete_carved_gloss.png": (
        "rgb",
        from_map(CARVED_MAP, {"M": (56, 56, 56), "T": (150, 150, 150)}),
    ),
    "concrete_carved_gloss_s.png": ("grey", uniform(220)),
    "concrete_carved_gloss_h.png": ("grey", from_map(CARVED_MAP, {"M": 56, "T": 200})),
    "concrete_carved_gloss_p.png": ("grey", from_map(CARVED_MAP, {"M": 30, "T": 235})),
    "glass_gloss_tiles.png": (
        "rgb",
        from_map(CARVED_MAP, {"M": (44, 48, 52), "T": (140, 180, 190)}),
    ),
    "glass_gloss_tiles_s.png": ("grey", uniform(255)),
    "glass_gloss_tiles_h.png": ("grey", from_map(CARVED_MAP, {"M": 56, "T": 200})),
    "glass_gloss_tiles_p.png": ("grey", from_map(GLASS_P_MAP, {"M": 30, "S": 235, "B": 120})),
    "concrete_carved_tinted_gloss.png": (
        "rgb",
        from_map(CARVED_MAP, {"M": (110, 60, 34), "T": (60, 140, 170)}),
    ),
    "concrete_carved_tinted_gloss_s.png": ("grey", uniform(220)),
    "concrete_carved_tinted_gloss_h.png": ("grey", from_map(CARVED_MAP, {"M": 56, "T": 200})),
    "concrete_carved_tinted_gloss_p.png": (
        "grey",
        from_map(TINTED_P_MAP, {"M": 30, "S": 235, "B": 120}),
    ),
    "concrete_carved_tinted_gloss_t.png": ("grey", from_map(TINTED_T_MAP, {"T": 255, "U": 0})),
    "concrete_satin.png": (
        "rgb",
        from_map(SATIN_MAP, {"S": (104, 98, 90), "P": (168, 158, 142)}),
    ),
    "concrete_satin_s.png": ("grey", uniform(255)),
    "concrete_satin_p.png": ("grey", uniform(64)),
    "metal_polished.png": (
        "rgb",
        from_map(POLISHED_MAP, {"A": (64, 68, 74), "B": (80, 84, 90)}),
    ),
    "metal_polished_s.png": ("grey", uniform(255)),
    "metal_polished_p.png": ("grey", uniform(200)),
    "metal_polished_t.png": ("grey", uniform(0)),
    "metal_fade_band.png": (
        "rgb",
        from_map(POLISHED_MAP, {"A": (64, 68, 74), "B": (80, 84, 90)}),
    ),
    "metal_fade_band_s.png": ("grey", uniform(255)),
    "metal_fade_band_p.png": ("grey", uniform(97)),
    "metal_fade_band_t.png": ("grey", uniform(0)),
    "concrete_tile_grout.png": (
        "rgb",
        from_map(TILE_MAP, {"G": (150, 146, 138), "T": (36, 44, 60)}),
    ),
    "concrete_tile_grout_s.png": ("grey", from_map(TILE_MAP, {"G": 0, "T": 255})),
    "concrete_tile_grout_p.png": ("grey", uniform(200)),
    "concrete_tile_grout_t.png": ("grey", uniform(0)),
    "metal_mirror_tinted.png": (
        "rgb",
        from_map(MIRROR_MAP, {"A": (70, 70, 70), "B": (96, 96, 96)}),
    ),
    "metal_mirror_tinted_s.png": ("grey", uniform(255)),
    "metal_mirror_tinted_p.png": ("grey", uniform(200)),
    "metal_mirror_tinted_t.png": ("grey", uniform(224)),
    "metal_satin_tinted.png": (
        "rgb",
        from_map(MIRROR_MAP, {"A": (70, 70, 70), "B": (96, 96, 96)}),
    ),
    "metal_satin_tinted_s.png": ("grey", uniform(255)),
    "metal_satin_tinted_p.png": ("grey", uniform(115)),
    "metal_satin_tinted_t.png": ("grey", uniform(224)),
    "concrete_signal_stripes.png": (
        "rgb",
        from_map(
            STRIPE_MAP,
            {
                "W": (236, 232, 220),
                "K": (18, 18, 22),
                "O": (236, 112, 20),
                "C": (24, 176, 216),
            },
        ),
    ),
}

# The reflection floor and the end of its fade band, as exponents
# (level-format `glossy_opt_in.rs`).
REFLECTION_FLOOR_EXPONENT = 8.0
FULL_WEIGHT_EXPONENT = 16.0


# --- An independent PNG reader. ---------------------------------------------


def read_png(data):
    """(chunk tags, width, height, kind, rows) of an 8-bit grey or RGB PNG,
    unfiltering every filter type."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "PNG signature"
    pos = 8
    tags = []
    header = None
    idat = bytearray()
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        (crc,) = struct.unpack(">I", data[pos + 8 + length : pos + 12 + length])
        assert crc == zlib.crc32(tag + body) & 0xFFFFFFFF, f"CRC of {tag!r}"
        tags.append(tag.decode("ascii"))
        if tag == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color_type, compression, filtering, interlace = header
    assert (depth, compression, filtering, interlace) == (8, 0, 0, 0)
    channels = {0: 1, 2: 3}[color_type]
    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    rows = []
    previous = bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        filter_type = raw[start]
        line = bytearray(raw[start + 1 : start + 1 + stride])
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up = previous[i]
            up_left = previous[i - channels] if i >= channels else 0
            if filter_type == 1:
                line[i] = (line[i] + left) & 0xFF
            elif filter_type == 2:
                line[i] = (line[i] + up) & 0xFF
            elif filter_type == 3:
                line[i] = (line[i] + (left + up) // 2) & 0xFF
            elif filter_type == 4:
                p = left + up - up_left
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - up_left)
                pred = left if pa <= pb and pa <= pc else up if pb <= pc else up_left
                line[i] = (line[i] + pred) & 0xFF
        previous = line
        if channels == 1:
            rows.append(list(line))
        else:
            rows.append([tuple(line[i : i + 3]) for i in range(0, stride, 3)])
    kind = "grey" if channels == 1 else "rgb"
    return tags, width, height, kind, rows


class GenSpecularTexelFixturesTest(unittest.TestCase):
    def check_directory(self, directory):
        for name, (kind, rows) in EXPECTED.items():
            with self.subTest(file=name):
                tags, width, height, got_kind, got_rows = read_png((directory / name).read_bytes())
                # No colour-space chunk of any kind: only the three critical ones.
                self.assertEqual(tags, ["IHDR", "IDAT", "IEND"])
                self.assertEqual((width, height), (SIZE, SIZE))
                self.assertEqual(got_kind, kind)
                self.assertEqual(got_rows, rows)

    def test_committed_fixtures_hold_every_expected_texel(self):
        self.check_directory(COMMITTED)

    def test_a_fresh_run_writes_every_expected_texel_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as tmp:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(GEN.main(["--out", tmp]), 0)
            self.assertEqual(sorted(p.name for p in Path(tmp).iterdir()), sorted(EXPECTED))
            self.check_directory(Path(tmp))

    def test_two_runs_write_identical_bytes(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            with contextlib.redirect_stdout(io.StringIO()):
                GEN.main(["--out", a])
                GEN.main(["--out", b])
            for name in EXPECTED:
                self.assertEqual((Path(a) / name).read_bytes(), (Path(b) / name).read_bytes())

    def test_committed_set_has_no_stray_sibling(self):
        # A stray `_p` or `_t` beside a fixture would widen its slot.
        stems = (
            "metal_smoothness_ramp",
            "metal_tint_inlay",
            "concrete_carved_gloss",
            "glass_gloss_tiles",
            "concrete_carved_tinted_gloss",
            "concrete_satin",
            "metal_polished",
            "metal_fade_band",
            "concrete_tile_grout",
            "metal_mirror_tinted",
            "metal_satin_tinted",
            "concrete_signal_stripes",
        )
        present = sorted(
            p.name for p in COMMITTED.iterdir() if p.name.startswith(stems)
        )
        self.assertEqual(present, sorted(EXPECTED))

    def test_ramp_spans_flat_mid_and_cap_bands(self):
        # Bytes 0-24 shade at exponent 1 and 224 and up at the 8192 cap.
        self.assertEqual([b for b in RAMP_P_ROW if b <= 24], [0, 17])
        self.assertEqual([b for b in RAMP_P_ROW if b >= 224], [238, 255])
        # Check the band edges against n = 2 / r^4 - 2 at r = 1 - p / 255.
        exponent = lambda p: 2 / (1 - p / 255) ** 4 - 2
        self.assertLessEqual(exponent(24), 1.0)
        self.assertGreaterEqual(exponent(224), 8192.0)

    def test_new_fixtures_carry_every_case_they_cover(self):
        exponent = lambda p: min(max(2 / (1 - p / 255) ** 4 - 2, 1.0), 8192.0)
        # Glass: `_p` on the one prefix with no Surface Depth tuning, beside a
        # height map with real relief, so a march that ran would show.
        _, _, _, _, glass_h = read_png((COMMITTED / "glass_gloss_tiles_h.png").read_bytes())
        self.assertEqual({b for row in glass_h for b in row}, {56, 200})
        # Each sharpness reaches a distinct exponent band: cap, mid and broad.
        self.assertEqual(exponent(235), 8192.0)
        self.assertTrue(10.0 < exponent(120) < 100.0)
        self.assertLess(exponent(30), 2.0)
        # Tinted: every sharpness appears in both a tinted and an untinted
        # column, so tint and sharpness vary independently.
        pairs = {
            (p, t)
            for p_row, t_row in zip(TINTED_P_MAP, TINTED_T_MAP)
            for p, t in zip(p_row, t_row)
        }
        self.assertEqual(pairs, {(p, t) for p in "MSB" for t in "TU"})

    def test_art_direction_fixtures_sit_where_their_rooms_need_them(self):
        exponent = lambda p: min(max(2 / (1 - p / 255) ** 4 - 2, 1.0), 8192.0)
        # Satin: highlights at a broad exponent, under the reflection floor.
        self.assertTrue(3.0 < exponent(64) < 5.0)
        self.assertLessEqual(exponent(64), REFLECTION_FLOOR_EXPONENT)
        # Polished: past the fade band, so full weight.
        self.assertGreater(exponent(200), FULL_WEIGHT_EXPONENT)
        # Fade band: inside the band, near its middle in log2.
        self.assertTrue(10.0 < exponent(97) < 13.0)
        self.assertGreater(exponent(97), REFLECTION_FLOOR_EXPONENT)
        self.assertLess(exponent(97), FULL_WEIGHT_EXPONENT)
        # The band's byte edges: 84 is the last byte at or under the floor,
        # 85 the first past it, 108 the first at full weight.
        self.assertLessEqual(exponent(84), REFLECTION_FLOOR_EXPONENT)
        self.assertGreater(exponent(85), REFLECTION_FLOOR_EXPONENT)
        self.assertLess(exponent(107), FULL_WEIGHT_EXPONENT)
        self.assertGreaterEqual(exponent(108), FULL_WEIGHT_EXPONENT)
        # Polished and fade band differ only in `_p`.
        for suffix in (".png", "_s.png", "_t.png"):
            self.assertEqual(
                EXPECTED["metal_polished" + suffix], EXPECTED["metal_fade_band" + suffix]
            )
        self.assertNotEqual(EXPECTED["metal_polished_p.png"], EXPECTED["metal_fade_band_p.png"])
        # Tile and grout: the grout is painted through `_s` 0 alone. Its `_p`
        # matches the tiles' and is past the floor, so `_p` never masks it.
        _, tile_s = EXPECTED["concrete_tile_grout_s.png"]
        _, tile_p = EXPECTED["concrete_tile_grout_p.png"]
        pairs = {
            (c, s, p)
            for map_row, s_row, p_row in zip(TILE_MAP, tile_s, tile_p)
            for c, s, p in zip(map_row, s_row, p_row)
        }
        self.assertEqual(pairs, {("G", 0, 200), ("T", 255, 200)})
        self.assertGreater(exponent(200), FULL_WEIGHT_EXPONENT)

    def test_showcase_fixtures_reflect_strongly_at_two_sharpnesses(self):
        exponent = lambda p: min(max(2 / (1 - p / 255) ** 4 - 2, 1.0), 8192.0)
        # Both tinted floors sit past the fade band, so each reflects at full
        # weight; the satin one is far rougher than the mirror one.
        self.assertGreater(exponent(200), FULL_WEIGHT_EXPONENT)
        self.assertGreater(exponent(115), FULL_WEIGHT_EXPONENT)
        self.assertTrue(18.0 < exponent(115) < 22.0)
        # They differ only in `_p`, and their tint is partial: reflectance
        # 0.02 x (2 + 48 x t) lands between the untinted 0.04 and 1.
        for suffix in (".png", "_s.png", "_t.png"):
            self.assertEqual(
                EXPECTED["metal_mirror_tinted" + suffix], EXPECTED["metal_satin_tinted" + suffix]
            )
        self.assertNotEqual(
            EXPECTED["metal_mirror_tinted_p.png"], EXPECTED["metal_satin_tinted_p.png"]
        )
        self.assertTrue(0.85 < 0.02 * (2 + 48 * 224 / 255) < 0.9)
        # The neutral albedo keeps the tinted reflection white.
        _, rows = EXPECTED["metal_mirror_tinted.png"]
        self.assertTrue(all(r == g == b for row in rows for (r, g, b) in row))
        # The striped subject never reflects: no slot sibling at all.
        self.assertFalse(
            any(name.startswith("concrete_signal_stripes_") for name in EXPECTED)
        )


if __name__ == "__main__":
    unittest.main()
