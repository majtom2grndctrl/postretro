#!/usr/bin/env python3
"""Generate test-3.map: 6-room layout with L-shaped corridors.

Layout (top-down, X right, Y up in Quake coords):

    +------+     +------+     +------+
    |  R1  |---->|  R2  |---->|  R3  |
    |spawn |     | big  |     |      |
    +------+     +--+---+     +------+
                    |
                 +--+---+
                 |  R4  |
                 | tall |
                 +--+---+
                    |
    +------+     +--+---+
    |  R6  |<----|  R5  |
    |small |     |      |
    +------+     +------+

Corridors are narrow (64 wide, 96 tall) with lintels to create tight portals.
R1 and R6 should have minimal/no mutual visibility — separated by two right-angle turns.

All units in Quake coordinates (right-handed, Z-up).
"""

# Texture constants (stems of PNG files in assets/textures/50-free-textures/)
TEX_FLOOR = "concrete_pavement_036"
TEX_CEIL = "concrete_stone_021"
TEX_WALL = "concrete_stone_022"
TEX_CORR_FLOOR = "concrete_pavement_044"
TEX_CORR_CEIL = "concrete_stone_023"
TEX_CORR_WALL = "concrete_stone_024"
TEX_PILLAR = "wood_bark_046"
TEX_EMPTY = "__TB_empty"

TEXTURE = TEX_EMPTY


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
                opening_north=None, opening_south=None,
                tex_floor=TEX_FLOOR, tex_ceil=TEX_CEIL, tex_wall=TEX_WALL):
    """Build a sealed room. Openings are (lo, hi, z_top) on the relevant axis."""
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    ox1, oy1, oz1 = ix1 - wall, iy1 - wall, iz1 - wall
    ox2, oy2, oz2 = ix2 + wall, iy2 + wall, iz2 + wall
    brushes = []

    brushes.append(box_brush(ox1, oy1, oz1, ox2, oy2, iz1, tex=tex_floor, comment="floor"))
    brushes.append(box_brush(ox1, oy1, iz2, ox2, oy2, oz2, tex=tex_ceil, comment="ceiling"))

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
                                             tex=tex_wall, comment=f"{name} wall segment A"))
                if ohi < hi_axis:
                    brushes.append(box_brush(wx, ohi, iz1, wx2, hi_axis, iz2,
                                             tex=tex_wall, comment=f"{name} wall segment B"))
                if ztop < iz2:
                    brushes.append(box_brush(wx, olo, ztop, wx2, ohi, iz2,
                                             tex=tex_wall, comment=f"{name} wall lintel"))
            else:
                brushes.append(box_brush(wx, lo_axis, iz1, wx2, hi_axis, iz2,
                                         tex=tex_wall, comment=f"{name} wall"))
        else:  # south/north
            wy = oy1 if at_min else iy2
            wy2 = iy1 if at_min else oy2
            if opening:
                olo, ohi, ztop = opening
                if olo > lo_axis:
                    brushes.append(box_brush(lo_axis, wy, iz1, olo, wy2, iz2,
                                             tex=tex_wall, comment=f"{name} wall segment A"))
                if ohi < hi_axis:
                    brushes.append(box_brush(ohi, wy, iz1, hi_axis, wy2, iz2,
                                             tex=tex_wall, comment=f"{name} wall segment B"))
                if ztop < iz2:
                    brushes.append(box_brush(olo, wy, ztop, ohi, wy2, iz2,
                                             tex=tex_wall, comment=f"{name} wall lintel"))
            else:
                brushes.append(box_brush(lo_axis, wy, iz1, hi_axis, wy2, iz2,
                                         tex=tex_wall, comment=f"{name} wall"))
    return brushes


def corridor_x(interior, wall=16, tex_floor=TEX_CORR_FLOOR, tex_ceil=TEX_CORR_CEIL, tex_wall=TEX_CORR_WALL):
    """Corridor running along the X axis."""
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1, iy1 - wall, iz1 - wall, ix2, iy2 + wall, iz1, tex=tex_floor, comment="corr floor"),
        box_brush(ix1, iy1 - wall, iz2, ix2, iy2 + wall, iz2 + wall, tex=tex_ceil, comment="corr ceiling"),
        box_brush(ix1, iy1 - wall, iz1, ix2, iy1, iz2, tex=tex_wall, comment="corr south wall"),
        box_brush(ix1, iy2, iz1, ix2, iy2 + wall, iz2, tex=tex_wall, comment="corr north wall"),
    ]


def corridor_y(interior, wall=16, tex_floor=TEX_CORR_FLOOR, tex_ceil=TEX_CORR_CEIL, tex_wall=TEX_CORR_WALL):
    """Corridor running along the Y axis."""
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1 - wall, iy1, iz1 - wall, ix2 + wall, iy2, iz1, tex=tex_floor, comment="corr floor"),
        box_brush(ix1 - wall, iy1, iz2, ix2 + wall, iy2, iz2 + wall, tex=tex_ceil, comment="corr ceiling"),
        box_brush(ix1 - wall, iy1, iz1, ix1, iy2, iz2, tex=tex_wall, comment="corr west wall"),
        box_brush(ix2, iy1, iz1, ix2 + wall, iy2, iz2, tex=tex_wall, comment="corr east wall"),
    ]


def filler(x1, y1, z1, x2, y2, z2, tex=TEX_EMPTY, comment="filler"):
    return box_brush(x1, y1, z1, x2, y2, z2, tex=tex, comment=comment)


def generate_map():
    W = 16
    DOOR_W = 64   # corridor width
    DOOR_H = 96   # corridor height (with lintel)
    ROOM_H = 128  # standard room height

    # --- Room definitions (interior air space) ---
    # Room 1: spawn, 256x256x128
    r1 = (0, 0, 0, 256, 256, ROOM_H)

    # Corridor 1: east from R1 to R2 (X-axis, 96 long)
    c1_y_lo = 96
    c1_y_hi = c1_y_lo + DOOR_W  # 160
    c1 = (r1[3] + W, c1_y_lo, 0, r1[3] + W + 96, c1_y_hi, DOOR_H)

    # Room 2: big, 384x384x160
    r2_x1 = c1[3] + W
    r2 = (r2_x1, -64, 0, r2_x1 + 384, -64 + 384, 160)

    # Corridor 2: east from R2 to R3 (X-axis, 96 long)
    c2_y_lo = 96
    c2_y_hi = c2_y_lo + DOOR_W
    c2 = (r2[3] + W, c2_y_lo, 0, r2[3] + W + 96, c2_y_hi, DOOR_H)

    # Room 3: 192x256x128
    r3_x1 = c2[3] + W
    r3 = (r3_x1, 0, 0, r3_x1 + 192, 256, ROOM_H)

    # Corridor 3: south from R2 (Y-axis, 96 long)
    c3_x_lo = r2[0] + 160
    c3_x_hi = c3_x_lo + DOOR_W
    c3 = (c3_x_lo, r2[1] - W - 96, 0, c3_x_hi, r2[1] - W, DOOR_H)

    # Room 4: tall, 256x256x192 (below R2)
    r4 = (r2[0] + 64, c3[1] - W - 256, 0, r2[0] + 64 + 256, c3[1] - W, 192)

    # Corridor 4: south from R4 (Y-axis, 96 long)
    c4_x_lo = r4[0] + 96
    c4_x_hi = c4_x_lo + DOOR_W
    c4 = (c4_x_lo, r4[1] - W - 96, 0, c4_x_hi, r4[1] - W, DOOR_H)

    # Room 5: 256x256x128 (below R4)
    r5 = (r4[0], c4[1] - W - 256, 0, r4[0] + 256, c4[1] - W, ROOM_H)

    # Corridor 5: west from R5 to R6 (X-axis, 96 long)
    c5_y_lo = r5[1] + 96
    c5_y_hi = c5_y_lo + DOOR_W
    c5 = (r5[0] - W - 96, c5_y_lo, 0, r5[0] - W, c5_y_hi, DOOR_H)

    # Room 6: small, 160x192x128
    r6 = (c5[0] - W - 160, r5[1] + 32, 0, c5[0] - W, r5[1] + 32 + 192, ROOM_H)

    # --- Opening specs: (lo, hi, z_top) ---
    c1_open = (c1_y_lo, c1_y_hi, DOOR_H)
    c2_open = (c2_y_lo, c2_y_hi, DOOR_H)
    c3_open_s = (c3_x_lo, c3_x_hi, DOOR_H)  # south opening on R2
    c3_open_n = (c3_x_lo, c3_x_hi, DOOR_H)  # north opening on R4
    c4_open_s = (c4_x_lo, c4_x_hi, DOOR_H)
    c4_open_n = (c4_x_lo, c4_x_hi, DOOR_H)
    c5_open = (c5_y_lo, c5_y_hi, DOOR_H)

    # --- Build brushes ---
    brushes = []

    # Room 1
    brushes.append("// ---- Room 1 (spawn, 256x256) ----")
    brushes.extend(sealed_room(r1, W, opening_east=c1_open))

    # Corridor 1
    brushes.append("// ---- Corridor 1 (R1 -> R2) ----")
    brushes.extend(corridor_x(c1, W))

    # Filler between R1 and R2 around corridor 1
    fx1, fx2 = r1[3] + W, r2[0] - W
    max_z = max(r1[5], r2[5])
    brushes.append(filler(fx1, r2[1] - W, -W, fx2, c1_y_lo, max_z + W,
                          comment="filler south of C1"))
    brushes.append(filler(fx1, c1_y_hi, -W, fx2, r2[4] + W, max_z + W,
                          comment="filler north of C1"))
    brushes.append(filler(fx1, c1_y_lo - W, DOOR_H, fx2, c1_y_hi + W, max_z + W,
                          comment="filler above C1"))

    # Room 2
    brushes.append("// ---- Room 2 (big, 384x384x160) ----")
    brushes.extend(sealed_room(r2, W,
                               opening_west=c1_open,
                               opening_east=c2_open,
                               opening_south=c3_open_s))

    # Pillar in center of Room 2 for extra geometry
    r2_cx = (r2[0] + r2[3]) // 2
    r2_cy = (r2[1] + r2[4]) // 2
    brushes.append("// ---- Room 2: center pillar ----")
    brushes.append(box_brush(r2_cx - 32, r2_cy - 32, 0, r2_cx + 32, r2_cy + 32, r2[5],
                             tex=TEX_PILLAR, comment="pillar"))

    # Corridor 2
    brushes.append("// ---- Corridor 2 (R2 -> R3) ----")
    brushes.extend(corridor_x(c2, W))

    # Filler between R2 and R3
    fx1, fx2 = r2[3] + W, r3[0] - W
    max_z = max(r2[5], r3[5])
    brushes.append(filler(fx1, r3[1] - W, -W, fx2, c2_y_lo, max_z + W,
                          comment="filler south of C2"))
    brushes.append(filler(fx1, c2_y_hi, -W, fx2, r3[4] + W, max_z + W,
                          comment="filler north of C2"))
    brushes.append(filler(fx1, c2_y_lo - W, DOOR_H, fx2, c2_y_hi + W, max_z + W,
                          comment="filler above C2"))

    # Room 3
    brushes.append("// ---- Room 3 (192x256) ----")
    brushes.extend(sealed_room(r3, W, opening_west=c2_open))

    # Corridor 3 (Y-axis, R2 south to R4 north)
    brushes.append("// ---- Corridor 3 (R2 -> R4) ----")
    brushes.extend(corridor_y(c3, W))

    # Filler around corridor 3
    # Use min/max of both rooms' X extents to seal the full gap.
    fy1, fy2 = r4[4] + W, r2[1] - W
    max_z = max(r2[5], r4[5])
    c3_fill_x_lo = min(r2[0], r4[0]) - W
    c3_fill_x_hi = max(r2[3], r4[3]) + W
    brushes.append(filler(c3_fill_x_lo, fy1, -W, c3_x_lo, fy2, max_z + W,
                          comment="filler west of C3"))
    brushes.append(filler(c3_x_hi, fy1, -W, c3_fill_x_hi, fy2, max_z + W,
                          comment="filler east of C3"))
    brushes.append(filler(c3_x_lo - W, fy1, DOOR_H, c3_x_hi + W, fy2, max_z + W,
                          comment="filler above C3"))

    # Room 4
    brushes.append("// ---- Room 4 (tall, 256x256x192) ----")
    brushes.extend(sealed_room(r4, W,
                               opening_north=c3_open_n,
                               opening_south=c4_open_s))

    # Corridor 4 (Y-axis, R4 south to R5 north)
    brushes.append("// ---- Corridor 4 (R4 -> R5) ----")
    brushes.extend(corridor_y(c4, W))

    # Filler around corridor 4
    fy1, fy2 = r5[4] + W, r4[1] - W
    max_z = max(r4[5], r5[5])
    brushes.append(filler(r5[0] - W, fy1, -W, c4_x_lo, fy2, max_z + W,
                          comment="filler west of C4"))
    brushes.append(filler(c4_x_hi, fy1, -W, r5[3] + W, fy2, max_z + W,
                          comment="filler east of C4"))
    brushes.append(filler(c4_x_lo - W, fy1, DOOR_H, c4_x_hi + W, fy2, max_z + W,
                          comment="filler above C4"))

    # Room 5
    brushes.append("// ---- Room 5 (256x256) ----")
    brushes.extend(sealed_room(r5, W,
                               opening_north=c4_open_n,
                               opening_west=c5_open))

    # Corridor 5 (X-axis, R5 west to R6 east)
    brushes.append("// ---- Corridor 5 (R5 -> R6) ----")
    brushes.extend(corridor_x(c5, W))

    # Filler around corridor 5
    fx1, fx2 = r6[3] + W, r5[0] - W
    max_z = max(r5[5], r6[5])
    brushes.append(filler(fx1, r6[1] - W, -W, fx2, c5_y_lo, max_z + W,
                          comment="filler south of C5"))
    brushes.append(filler(fx1, c5_y_hi, -W, fx2, r6[4] + W, max_z + W,
                          comment="filler north of C5"))
    brushes.append(filler(fx1, c5_y_lo - W, DOOR_H, fx2, c5_y_hi + W, max_z + W,
                          comment="filler above C5"))

    # Room 6
    brushes.append("// ---- Room 6 (small, 160x192) ----")
    brushes.extend(sealed_room(r6, W, opening_east=c5_open))

    # --- Player start: center of Room 1 ---
    spawn_x = (r1[0] + r1[3]) // 2
    spawn_y = (r1[1] + r1[4]) // 2
    spawn_z = 24

    # --- Assemble ---
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
        if item.startswith("//") and not item.startswith("// brush") and "{" not in item:
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

    # --- Lights ---
    # Exercise point + spot + directional per sub-plan 1 requirements.

    # Entity 2: point light in Room 2 (the big central room). Warm tint,
    # inverse-squared falloff. Origin centered in R2's interior air space.
    r2_light_x = (r2[0] + r2[3]) // 2
    r2_light_y = (r2[1] + r2[4]) // 2
    r2_light_z = r2[5] - 24
    lines.append("// entity 2 (point light)")
    lines.append("{")
    lines.append('"classname" "light"')
    lines.append(f'"origin" "{r2_light_x} {r2_light_y} {r2_light_z}"')
    lines.append('"light" "300"')
    lines.append('"_color" "255 190 120"')
    lines.append('"_fade" "4096"')
    lines.append('"delay" "2"')
    lines.append("}")

    # Entity 3: spotlight aimed down Corridor 4, exercising cone culling.
    # Place it on the Room 4 side above the corridor mouth, aimed south.
    c4_light_x = (c4[0] + c4[2]) // 2
    c4_light_y = r4[1] - 8
    c4_light_z = DOOR_H - 8
    lines.append("// entity 3 (spot light)")
    lines.append("{")
    lines.append('"classname" "light_spot"')
    lines.append(f'"origin" "{c4_light_x} {c4_light_y} {c4_light_z}"')
    lines.append('"light" "250"')
    lines.append('"_color" "180 210 255"')
    lines.append('"_fade" "2048"')
    lines.append('"delay" "1"')
    lines.append('"_cone" "25"')
    lines.append('"_cone2" "40"')
    # mangle: pitch=-10 (slight downward), yaw=270 (facing south in Quake),
    # roll=0. Quake yaw convention: 0=+X, 90=+Y, 180=-X, 270=-Y.
    lines.append('"mangle" "-10 270 0"')
    lines.append("}")

    # Entity 4: directional (sun) light — cool overhead ambient. Origin is
    # placed inside Room 1 for readability; directional lights ignore origin
    # for lighting math.
    lines.append("// entity 4 (directional / sun light)")
    lines.append("{")
    lines.append('"classname" "light_sun"')
    lines.append(f'"origin" "{spawn_x} {spawn_y} {r1[5] - 16}"')
    lines.append('"light" "180"')
    lines.append('"_color" "200 220 255"')
    lines.append('"delay" "0"')
    # Sun from roughly overhead, angled slightly forward.
    lines.append('"mangle" "-70 45 0"')
    lines.append("}")

    return "\n".join(lines)


if __name__ == "__main__":
    from pathlib import Path

    content = generate_map()
    out = Path(__file__).parent / "test-3.map"
    out.write_text(content + "\n")
    brush_count = sum(1 for line in content.split("\n") if line.strip().startswith("// brush "))
    print(f"Wrote {out} ({brush_count} brushes)")
