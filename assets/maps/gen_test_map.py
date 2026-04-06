#!/usr/bin/env python3
"""Generate test.map for Postretro Phase 1 BSP wireframe testing.

Layout (top-down, X right, Y up):

   Room 1 (spawn)  --Corridor 1-->  Room 2 (large, tall)  --Corridor 2-->  Room 3 (small)

Room 1: Deliberately non-square (384x256) so coordinate-transform mirroring is obvious.
Room 2: Large (384x384) and tall (192), with a raised platform + step for vertical variation.
Room 3: Small (128x192) for minimal draw-set testing.
Corridors: Narrow (96 wide, 112 tall) so vis separates rooms into distinct PVS clusters.

All units in Quake coordinates (right-handed, Z-up).
"""

TEXTURE = "__TB_empty"


def box_brush(x1, y1, z1, x2, y2, z2, tex=TEXTURE, comment=None):
    """Generate a 6-plane axis-aligned box brush.

    Points are ordered so normals point outward (into void).
    Brush interior is the intersection of the negative half-spaces.
    """
    lines = []
    if comment:
        lines.append(f"// {comment}")
    lines.append("{")
    # +X face (right)
    lines.append(f"( {x2} {y1} {z1} ) ( {x2} {y2} {z2} ) ( {x2} {y2} {z1} ) {tex} 0 0 0 1 1")
    # -X face (left)
    lines.append(f"( {x1} {y2} {z1} ) ( {x1} {y1} {z2} ) ( {x1} {y1} {z1} ) {tex} 0 0 0 1 1")
    # +Y face (front)
    lines.append(f"( {x2} {y2} {z1} ) ( {x1} {y2} {z2} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    # -Y face (back)
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y1} {z2} ) ( {x2} {y1} {z1} ) {tex} 0 0 0 1 1")
    # +Z face (top)
    lines.append(f"( {x1} {y2} {z2} ) ( {x2} {y1} {z2} ) ( {x1} {y1} {z2} ) {tex} 0 0 0 1 1")
    # -Z face (bottom)
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y2} {z1} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append("}")
    return "\n".join(lines)


def sealed_room(interior, wall=16, opening_east=None, opening_west=None,
                opening_north=None, opening_south=None):
    """Build a sealed room from an interior bounding box.

    interior: (x1, y1, z1, x2, y2, z2) — the air space.
    wall: wall/floor/ceiling thickness.
    opening_*: optional (y_lo, y_hi, z_hi) for doorways on that face.
               z_lo is always 0 (floor level). z_hi is the top of the opening.

    Returns a list of box_brush() strings.
    """
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    # Outer extents
    ox1, oy1, oz1 = ix1 - wall, iy1 - wall, iz1 - wall
    ox2, oy2, oz2 = ix2 + wall, iy2 + wall, iz2 + wall

    brushes = []

    # Floor and ceiling (span full outer extent in X and Y)
    brushes.append(box_brush(ox1, oy1, oz1, ox2, oy2, iz1, comment="floor"))
    brushes.append(box_brush(ox1, oy1, iz2, ox2, oy2, oz2, comment="ceiling"))

    # --- West wall (at ix1) ---
    if opening_west:
        olo, ohi, ztop = opening_west
        # Below opening — not needed, opening starts at floor
        if olo > iy1:
            brushes.append(box_brush(ox1, iy1, iz1, ix1, olo, iz2,
                                     comment="west wall south segment"))
        if ohi < iy2:
            brushes.append(box_brush(ox1, ohi, iz1, ix1, iy2, iz2,
                                     comment="west wall north segment"))
        if ztop < iz2:
            brushes.append(box_brush(ox1, olo, ztop, ix1, ohi, iz2,
                                     comment="west wall lintel"))
    else:
        brushes.append(box_brush(ox1, iy1, iz1, ix1, iy2, iz2,
                                 comment="west wall"))

    # --- East wall (at ix2) ---
    if opening_east:
        olo, ohi, ztop = opening_east
        if olo > iy1:
            brushes.append(box_brush(ix2, iy1, iz1, ox2, olo, iz2,
                                     comment="east wall south segment"))
        if ohi < iy2:
            brushes.append(box_brush(ix2, ohi, iz1, ox2, iy2, iz2,
                                     comment="east wall north segment"))
        if ztop < iz2:
            brushes.append(box_brush(ix2, olo, ztop, ox2, ohi, iz2,
                                     comment="east wall lintel"))
    else:
        brushes.append(box_brush(ix2, iy1, iz1, ox2, iy2, iz2,
                                 comment="east wall"))

    # --- South wall (at iy1) ---
    if opening_south:
        olo, ohi, ztop = opening_south  # x_lo, x_hi, z_hi
        if olo > ix1:
            brushes.append(box_brush(ix1, oy1, iz1, olo, iy1, iz2,
                                     comment="south wall west segment"))
        if ohi < ix2:
            brushes.append(box_brush(ohi, oy1, iz1, ix2, iy1, iz2,
                                     comment="south wall east segment"))
        if ztop < iz2:
            brushes.append(box_brush(olo, oy1, ztop, ohi, iy1, iz2,
                                     comment="south wall lintel"))
    else:
        brushes.append(box_brush(ix1, oy1, iz1, ix2, iy1, iz2,
                                 comment="south wall"))

    # --- North wall (at iy2) ---
    if opening_north:
        olo, ohi, ztop = opening_north  # x_lo, x_hi, z_hi
        if olo > ix1:
            brushes.append(box_brush(ix1, iy2, iz1, olo, oy2, iz2,
                                     comment="north wall west segment"))
        if ohi < ix2:
            brushes.append(box_brush(ohi, iy2, iz1, ix2, oy2, iz2,
                                     comment="north wall east segment"))
        if ztop < iz2:
            brushes.append(box_brush(olo, iy2, ztop, ohi, oy2, iz2,
                                     comment="north wall lintel"))
    else:
        brushes.append(box_brush(ix1, iy2, iz1, ix2, oy2, iz2,
                                 comment="north wall"))

    return brushes


def corridor(interior, wall=16):
    """Build a corridor with floor, ceiling, and two side walls.

    Corridor runs along X axis. Rooms seal the ends via their wall openings.
    interior: (x1, y1, z1, x2, y2, z2)
    """
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    brushes = []
    brushes.append(box_brush(ix1, iy1 - wall, iz1 - wall, ix2, iy2 + wall, iz1,
                             comment="corridor floor"))
    brushes.append(box_brush(ix1, iy1 - wall, iz2, ix2, iy2 + wall, iz2 + wall,
                             comment="corridor ceiling"))
    brushes.append(box_brush(ix1, iy1 - wall, iz1, ix2, iy1, iz2,
                             comment="corridor south wall"))
    brushes.append(box_brush(ix1, iy2, iz1, ix2, iy2 + wall, iz2,
                             comment="corridor north wall"))
    return brushes


def filler_block(x1, y1, z1, x2, y2, z2, comment="filler"):
    """Solid block to seal gaps between rooms outside corridor openings."""
    return box_brush(x1, y1, z1, x2, y2, z2, comment=comment)


def generate_map():
    W = 16  # wall thickness
    DOOR_H = 112  # corridor/doorway height

    # =========================================================
    # Interior bounding boxes (air space)
    # =========================================================

    # Room 1 — spawn. 384 wide x 256 deep x 128 tall (non-square = asymmetry).
    room1 = (0, 0, 0, 384, 256, 128)

    # Corridor 1 — 128 long x 96 wide x 112 tall.
    # Connects Room 1 east wall to Room 2 west wall.
    # Corridor Y centered at Y=128 (Room 1's midpoint).
    corr1 = (384 + W, 80, 0, 384 + W + 128, 176, DOOR_H)
    # That puts corridor interior at X=[400,528], Y=[80,176].

    # Room 2 — large. 384 wide x 384 deep x 192 tall.
    room2_x1 = corr1[3] + W  # 528 + 16 = 544
    room2 = (room2_x1, -64, 0, room2_x1 + 384, -64 + 384, 192)
    # Interior: (544, -64, 0, 928, 320, 192)

    # Corridor 2 — 128 long x 96 wide x 112 tall.
    corr2_x1 = room2[3] + W  # 928 + 16 = 944
    corr2 = (corr2_x1, 80, 0, corr2_x1 + 128, 176, DOOR_H)
    # Interior: (944, 80, 0, 1072, 176, 112)

    # Room 3 — small. 128 wide x 160 deep x 128 tall.
    room3_x1 = corr2[3] + W  # 1072 + 16 = 1088
    room3 = (room3_x1, 48, 0, room3_x1 + 128, 48 + 160, 128)
    # Interior: (1088, 48, 0, 1216, 208, 128)

    # =========================================================
    # Openings — (y_lo, y_hi, z_top) for east/west walls
    # =========================================================
    corr1_opening = (80, 176, DOOR_H)
    corr2_opening = (80, 176, DOOR_H)

    # =========================================================
    # Build brushes
    # =========================================================
    brushes = []

    # --- Room 1 ---
    brushes.append("// ---- Room 1 (spawn, 384x256x128) ----")
    brushes.extend(sealed_room(room1, W, opening_east=corr1_opening))

    # --- Corridor 1 ---
    brushes.append("// ---- Corridor 1 ----")
    brushes.extend(corridor(corr1, W))

    # Filler blocks to seal gaps between Room 1 and Room 2 outside the corridor
    r1_ox2 = room1[3] + W  # 400
    r2_ox1 = room2[0] - W  # 528
    r1_oz2 = room1[5] + W  # 144
    brushes.append(filler_block(r1_ox2, -W - 64, -W, r2_ox1, 80, r1_oz2,
                                comment="filler south of corridor 1"))
    brushes.append(filler_block(r1_ox2, 176, -W, r2_ox1, 320 + W, 192 + W,
                                comment="filler north of corridor 1"))
    # Also fill above corridor between rooms (corridor is 112 tall, rooms are taller)
    brushes.append(filler_block(r1_ox2, 80 - W, DOOR_H, r2_ox1, 176 + W, 192 + W,
                                comment="filler above corridor 1"))

    # --- Room 2 ---
    brushes.append("// ---- Room 2 (large, 384x384x192) ----")
    brushes.extend(sealed_room(room2, W,
                               opening_west=corr1_opening,
                               opening_east=corr2_opening))

    # Raised platform + step in SE quadrant of Room 2
    px1, py1, pz1 = room2[0] + 192, room2[1], 0  # starts 192 units into the room
    px2, py2, pz2 = room2[3], room2[1] + 192, 32  # half the room's Y depth, 32 units tall
    brushes.append("// ---- Room 2: raised platform + step ----")
    brushes.append(box_brush(px1 - 32, py1, 0, px1, py2, 16,
                             comment="step (16 units tall)"))
    brushes.append(box_brush(px1, py1, 0, px2, py2, pz2,
                             comment="raised platform (32 units tall)"))

    # --- Corridor 2 ---
    brushes.append("// ---- Corridor 2 ----")
    brushes.extend(corridor(corr2, W))

    # Filler blocks between Room 2 and Room 3
    r2_ox2 = room2[3] + W  # 944
    r3_ox1 = room3[0] - W  # 1072
    brushes.append(filler_block(r2_ox2, -64 - W, -W, r3_ox1, 80, 192 + W,
                                comment="filler south of corridor 2"))
    brushes.append(filler_block(r2_ox2, 176, -W, r3_ox1, 320 + W, 192 + W,
                                comment="filler north of corridor 2"))
    brushes.append(filler_block(r2_ox2, 80 - W, DOOR_H, r3_ox1, 176 + W, 192 + W,
                                comment="filler above corridor 2"))

    # --- Room 3 ---
    brushes.append("// ---- Room 3 (small, 128x160x128) ----")
    brushes.extend(sealed_room(room3, W, opening_west=corr2_opening))

    # =========================================================
    # Player start — center of Room 1, slightly above floor
    # =========================================================
    spawn_x = (room1[0] + room1[3]) // 2  # 192
    spawn_y = (room1[1] + room1[4]) // 2  # 128
    spawn_z = 24

    # =========================================================
    # Assemble .map file
    # =========================================================
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
        if item.startswith("//"):
            lines.append(item)
        else:
            lines.append(f"// brush {brush_num}")
            lines.append(item)
            brush_num += 1

    lines.append("}")

    # Player start entity
    lines.append("// entity 1")
    lines.append("{")
    lines.append('"classname" "info_player_start"')
    lines.append(f'"origin" "{spawn_x} {spawn_y} {spawn_z}"')
    lines.append('"angle" "0"')
    lines.append("}")

    # Light entity in Room 2 (so light tool doesn't complain if we ever run it)
    r2_cx = (room2[0] + room2[3]) // 2
    r2_cy = (room2[1] + room2[4]) // 2
    lines.append("// entity 2")
    lines.append("{")
    lines.append('"classname" "light"')
    lines.append(f'"origin" "{r2_cx} {r2_cy} 160"')
    lines.append('"light" "300"')
    lines.append("}")

    return "\n".join(lines)


if __name__ == "__main__":
    import sys
    from pathlib import Path

    content = generate_map()
    out = Path(__file__).parent / "test.map"
    out.write_text(content + "\n")
    print(f"Wrote {out} ({content.count(chr(123)) - 2} brushes)")  # subtract entity braces
    # Count for verification
    brush_count = content.count("// brush ")
    print(f"Brush count: {brush_count}")
