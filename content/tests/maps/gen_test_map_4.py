#!/usr/bin/env python3
"""Generate test_map_4.map: varied room sizes for visibility scaling tests.

Layout (top-down, X right, Y up in Quake coords):

                    +------------------+
                    |    Huge Room     |
                    |  2048x2048x512   |
                    +--------+---------+
                             |
                         (corridor N)
                             |
    +--------+     +---------+--------+     +-----------------+
    | Small  |---->|   Medium Room    |---->|   Large Room    |
    |256x256 |     |   512x512x256   |     | 1024x1024x384  |
    | spawn  |     |                  |     |                 |
    +--------+     +------------------+     +-----------------+

Corridors:
  - Small<->Medium: 96 wide, 96 tall, 128 long
  - Medium<->Large: 128 wide, 128 tall, 128 long
  - Medium<->Huge:  128 wide, 128 tall, 128 long

All corridors are straight segments. The transition from small-room cluster
scale to huge-room cluster scale is the interesting test case for the PVS
algorithm.

All units in Quake coordinates (right-handed, Z-up).
Wall thickness: 16 units throughout.
"""

TEXTURE = "__TB_empty"


def box_brush(x1, y1, z1, x2, y2, z2, tex=TEXTURE, comment=None):
    """Axis-aligned box brush with inward-pointing face normals."""
    lines = []
    if comment:
        lines.append(f"// {comment}")
    lines.append("{")
    lines.append(f"( {x2} {y1} {z1} ) ( {x2} {y2} {z2} ) ( {x2} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y2} {z1} ) ( {x1} {y1} {z2} ) ( {x1} {y1} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x2} {y2} {z1} ) ( {x1} {y2} {z2} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y1} {z2} ) ( {x2} {y1} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y2} {z2} ) ( {x2} {y1} {z2} ) ( {x1} {y1} {z2} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y2} {z1} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append("}")
    return "\n".join(lines)


def sealed_room(interior, wall=16, opening_east=None, opening_west=None,
                opening_north=None, opening_south=None):
    """Build a sealed room from six slabs with optional openings.

    Openings are (lo, hi, z_top) on the relevant axis, where the wall is
    split into segments around the opening.
    """
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    ox1, oy1, oz1 = ix1 - wall, iy1 - wall, iz1 - wall
    ox2, oy2, oz2 = ix2 + wall, iy2 + wall, iz2 + wall
    brushes = []

    brushes.append(box_brush(ox1, oy1, oz1, ox2, oy2, iz1, comment="floor"))
    brushes.append(box_brush(ox1, oy1, iz2, ox2, oy2, oz2, comment="ceiling"))

    for name, at_min, opening, lo_axis, hi_axis in [
        ("west",  True,  opening_west,  iy1, iy2),
        ("east",  False, opening_east,  iy1, iy2),
        ("south", True,  opening_south, ix1, ix2),
        ("north", False, opening_north, ix1, ix2),
    ]:
        if name in ("west", "east"):
            wx = ox1 if at_min else ix2
            wx2 = ix1 if at_min else ox2
            if opening:
                olo, ohi, ztop = opening
                if olo > lo_axis:
                    brushes.append(box_brush(wx, lo_axis, iz1, wx2, olo, iz2,
                                             comment=f"{name} wall segment A"))
                if ohi < hi_axis:
                    brushes.append(box_brush(wx, ohi, iz1, wx2, hi_axis, iz2,
                                             comment=f"{name} wall segment B"))
                if ztop < iz2:
                    brushes.append(box_brush(wx, olo, ztop, wx2, ohi, iz2,
                                             comment=f"{name} wall lintel"))
            else:
                brushes.append(box_brush(wx, lo_axis, iz1, wx2, hi_axis, iz2,
                                         comment=f"{name} wall"))
        else:  # south/north
            wy = oy1 if at_min else iy2
            wy2 = iy1 if at_min else oy2
            if opening:
                olo, ohi, ztop = opening
                if olo > lo_axis:
                    brushes.append(box_brush(lo_axis, wy, iz1, olo, wy2, iz2,
                                             comment=f"{name} wall segment A"))
                if ohi < hi_axis:
                    brushes.append(box_brush(ohi, wy, iz1, hi_axis, wy2, iz2,
                                             comment=f"{name} wall segment B"))
                if ztop < iz2:
                    brushes.append(box_brush(olo, wy, ztop, ohi, wy2, iz2,
                                             comment=f"{name} wall lintel"))
            else:
                brushes.append(box_brush(lo_axis, wy, iz1, hi_axis, wy2, iz2,
                                         comment=f"{name} wall"))
    return brushes


def corridor_x(interior, wall=16):
    """Corridor running along the X axis (floor, ceiling, south wall, north wall)."""
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1, iy1 - wall, iz1 - wall, ix2, iy2 + wall, iz1, comment="corr floor"),
        box_brush(ix1, iy1 - wall, iz2, ix2, iy2 + wall, iz2 + wall, comment="corr ceiling"),
        box_brush(ix1, iy1 - wall, iz1, ix2, iy1, iz2, comment="corr south wall"),
        box_brush(ix1, iy2, iz1, ix2, iy2 + wall, iz2, comment="corr north wall"),
    ]


def corridor_y(interior, wall=16):
    """Corridor running along the Y axis (floor, ceiling, west wall, east wall)."""
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1 - wall, iy1, iz1 - wall, ix2 + wall, iy2, iz1, comment="corr floor"),
        box_brush(ix1 - wall, iy1, iz2, ix2 + wall, iy2, iz2 + wall, comment="corr ceiling"),
        box_brush(ix1 - wall, iy1, iz1, ix1, iy2, iz2, comment="corr west wall"),
        box_brush(ix2, iy1, iz1, ix2 + wall, iy2, iz2, comment="corr east wall"),
    ]


def filler(x1, y1, z1, x2, y2, z2, comment="filler"):
    """Solid brush that fills a gap between rooms around a corridor."""
    return box_brush(x1, y1, z1, x2, y2, z2, comment=comment)


def generate_map():
    W = 16  # wall thickness

    # --- Corridor dimensions ---
    # Small<->Medium corridor: 96 wide, 96 tall
    C_SM_W = 96
    C_SM_H = 96
    C_SM_LEN = 128  # length of corridor between wall surfaces

    # Medium<->Large corridor: 128 wide, 128 tall
    C_ML_W = 128
    C_ML_H = 128
    C_ML_LEN = 128

    # Medium<->Huge corridor (Y-axis): 128 wide, 128 tall
    C_MH_W = 128
    C_MH_H = 128
    C_MH_LEN = 128

    # --- Room interiors (air space) ---
    # All rooms share floor at Z=0.

    # Medium Room: 512x512x256, centered at origin for easy layout
    med = (0, 0, 0, 512, 512, 256)

    # Small Room: 256x256x128, west of Medium
    # Corridor connects at Medium's west wall, so Small's east edge + wall +
    # corridor + wall = Medium's west edge.
    small_x2 = med[0] - W - C_SM_LEN - W
    small = (small_x2 - 256, 128, 0, small_x2, 128 + 256, 128)
    # Small room Y centered on corridor: corridor at med Y center (256),
    # so small room Y from 128..384 centers on 256.

    # Large Room: 1024x1024x384, east of Medium
    large_x1 = med[3] + W + C_ML_LEN + W
    large = (large_x1, -256, 0, large_x1 + 1024, -256 + 1024, 384)
    # Large room Y centered on corridor at med Y center (256),
    # so large room Y from -256..768 centers on 256.

    # Huge Room: 2048x2048x512, north of Medium
    huge_y1 = med[4] + W + C_MH_LEN + W
    huge = (-768, huge_y1, 0, -768 + 2048, huge_y1 + 2048, 512)
    # Huge room X centered on corridor at med X center (256),
    # so huge room X from -768..1280 centers on 256.

    # --- Corridor interiors ---
    # Corridor 1: Small <-> Medium (X-axis)
    # Centered on Y=256 (center of both rooms' Y overlap)
    c1_y_lo = 256 - C_SM_W // 2   # 208
    c1_y_hi = 256 + C_SM_W // 2   # 304
    c1 = (small[3] + W, c1_y_lo, 0, med[0] - W, c1_y_hi, C_SM_H)

    # Corridor 2: Medium <-> Large (X-axis)
    # Centered on Y=256
    c2_y_lo = 256 - C_ML_W // 2   # 192
    c2_y_hi = 256 + C_ML_W // 2   # 320
    c2 = (med[3] + W, c2_y_lo, 0, large[0] - W, c2_y_hi, C_ML_H)

    # Corridor 3: Medium <-> Huge (Y-axis)
    # Centered on X=256
    c3_x_lo = 256 - C_MH_W // 2   # 192
    c3_x_hi = 256 + C_MH_W // 2   # 320
    c3 = (c3_x_lo, med[4] + W, 0, c3_x_hi, huge[1] - W, C_MH_H)

    # --- Opening specs: (lo, hi, z_top) ---
    # Corridor 1 openings (on Y axis for east/west walls)
    c1_open_sm = (c1_y_lo, c1_y_hi, C_SM_H)  # opening in small's east / med's west

    # Corridor 2 openings (on Y axis for east/west walls)
    c2_open_ml = (c2_y_lo, c2_y_hi, C_ML_H)

    # Corridor 3 openings (on X axis for north/south walls)
    c3_open_mh = (c3_x_lo, c3_x_hi, C_MH_H)

    # --- Build brushes ---
    brushes = []

    # ---- Small Room (256x256x128, spawn) ----
    brushes.append("// ---- Small Room (256x256x128, spawn) ----")
    brushes.extend(sealed_room(small, W, opening_east=c1_open_sm))

    # ---- Corridor 1: Small <-> Medium (X-axis) ----
    brushes.append("// ---- Corridor 1: Small <-> Medium ----")
    brushes.extend(corridor_x(c1, W))

    # Filler around Corridor 1
    # The gap between small room's outer east wall and medium room's outer
    # west wall, excluding the corridor itself.
    fx1 = small[3] + W   # corridor X start
    fx2 = med[0] - W     # corridor X end
    # Vertical extent: from below floor to above the taller room's ceiling
    max_z = max(small[5], med[5])
    # South filler: from the southernmost room extent up to corridor south edge
    fill_y_lo = min(small[1], med[1]) - W
    fill_y_hi = max(small[4], med[4]) + W
    brushes.append(filler(fx1, fill_y_lo, -W, fx2, c1_y_lo, max_z + W,
                          comment="C1 filler south"))
    # North filler: from corridor north edge to northernmost room extent
    brushes.append(filler(fx1, c1_y_hi, -W, fx2, fill_y_hi, max_z + W,
                          comment="C1 filler north"))
    # Above filler: over the corridor opening
    brushes.append(filler(fx1, c1_y_lo - W, C_SM_H, fx2, c1_y_hi + W, max_z + W,
                          comment="C1 filler above"))

    # ---- Medium Room (512x512x256, central hub) ----
    brushes.append("// ---- Medium Room (512x512x256) ----")
    brushes.extend(sealed_room(med, W,
                               opening_west=c1_open_sm,
                               opening_east=c2_open_ml,
                               opening_north=c3_open_mh))

    # ---- Corridor 2: Medium <-> Large (X-axis) ----
    brushes.append("// ---- Corridor 2: Medium <-> Large ----")
    brushes.extend(corridor_x(c2, W))

    # Filler around Corridor 2
    fx1 = med[3] + W
    fx2 = large[0] - W
    max_z = max(med[5], large[5])
    fill_y_lo = min(med[1], large[1]) - W
    fill_y_hi = max(med[4], large[4]) + W
    brushes.append(filler(fx1, fill_y_lo, -W, fx2, c2_y_lo, max_z + W,
                          comment="C2 filler south"))
    brushes.append(filler(fx1, c2_y_hi, -W, fx2, fill_y_hi, max_z + W,
                          comment="C2 filler north"))
    brushes.append(filler(fx1, c2_y_lo - W, C_ML_H, fx2, c2_y_hi + W, max_z + W,
                          comment="C2 filler above"))

    # ---- Large Room (1024x1024x384) ----
    brushes.append("// ---- Large Room (1024x1024x384) ----")
    brushes.extend(sealed_room(large, W, opening_west=c2_open_ml))

    # ---- Corridor 3: Medium <-> Huge (Y-axis) ----
    brushes.append("// ---- Corridor 3: Medium <-> Huge ----")
    brushes.extend(corridor_y(c3, W))

    # Filler around Corridor 3
    fy1 = med[4] + W
    fy2 = huge[1] - W
    max_z = max(med[5], huge[5])
    fill_x_lo = min(med[0], huge[0]) - W
    fill_x_hi = max(med[3], huge[3]) + W
    brushes.append(filler(fill_x_lo, fy1, -W, c3_x_lo, fy2, max_z + W,
                          comment="C3 filler west"))
    brushes.append(filler(c3_x_hi, fy1, -W, fill_x_hi, fy2, max_z + W,
                          comment="C3 filler east"))
    brushes.append(filler(c3_x_lo - W, fy1, C_MH_H, c3_x_hi + W, fy2, max_z + W,
                          comment="C3 filler above"))

    # ---- Huge Room (2048x2048x512) ----
    brushes.append("// ---- Huge Room (2048x2048x512) ----")
    brushes.extend(sealed_room(huge, W, opening_south=c3_open_mh))

    # --- Player start: center of Small Room, 24 units above floor ---
    spawn_x = (small[0] + small[3]) // 2
    spawn_y = (small[1] + small[4]) // 2
    spawn_z = 24

    # --- Assemble .map output ---
    lines = [
        "// Game: Generic",
        "// Format: Standard",
        "// entity 0",
        "{",
        '"classname" "worldspawn"',
        '"wad" ""',
    ]

    brush_num = 0
    for item in brushes:
        if "{" not in item:
            # Section comment (no brush geometry)
            lines.append(item)
        else:
            lines.append(f"// brush {brush_num}")
            lines.append(item)
            brush_num += 1

    lines.append("}")

    lines.append("// entity 1")
    lines.append("{")
    lines.append('"classname" "info_player_start"')
    lines.append(f'"origin" "{spawn_x} {spawn_y} {spawn_z}"')
    lines.append('"angle" "0"')
    lines.append("}")

    return "\n".join(lines)


if __name__ == "__main__":
    from pathlib import Path

    content = generate_map()
    out = Path(__file__).parent / "test_map_4.map"
    out.write_text(content + "\n")
    brush_count = sum(1 for line in content.split("\n") if line.strip().startswith("// brush "))
    print(f"Wrote {out} ({brush_count} brushes)")
