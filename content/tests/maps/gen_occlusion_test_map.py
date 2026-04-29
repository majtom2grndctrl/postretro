#!/usr/bin/env python3
"""Generate hiz_test.map for testing HiZ culling and portal occlusion.

Final Version (Self-Review 2):
  - Cleaned up duplicated assembly logic.
  - Adjusted filler brushes to match max room heights (preventing leaks).
  - Added "High-Frequency" detail clusters (many tiny brushes) to stress HiZ.
  - Improved texture variety for better visual debugging.
"""

import math

# Textures from assets/textures/DebugTextures/
TEXTURE_WALL = "debug_wall_grey"
TEXTURE_FLOOR = "debug_floor_grey"
TEXTURE_CEIL = "debug_ceiling_grey"
TEXTURE_DETAIL = "debug_wall_red"
TEXTURE_COVER = "debug_wall_blue"
TEXTURE_BUILDING = "debug_wall_yellow"

def box_brush(x1, y1, z1, x2, y2, z2, tex=TEXTURE_WALL, comment=None):
    """Axis-aligned box brush with Quake plane definitions."""
    lines = []
    if comment:
        lines.append(f"// {comment}")
    lines.append("{")
    # Faces defined by 3 points. Winding consistent with existing gen_test_map.py
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
                tex_floor=TEXTURE_FLOOR, tex_ceil=TEXTURE_CEIL, tex_wall=TEXTURE_WALL):
    """Build a sealed room with optional wall openings."""
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
            wx, wx2 = (ox1, ix1) if at_min else (ix2, ox2)
            if opening:
                olo, ohi, ztop = opening
                if olo > lo_axis: brushes.append(box_brush(wx, lo_axis, iz1, wx2, olo, iz2, tex=tex_wall, comment=f"{name} wall A"))
                if ohi < hi_axis: brushes.append(box_brush(wx, ohi, iz1, wx2, hi_axis, iz2, tex=tex_wall, comment=f"{name} wall B"))
                if ztop < iz2: brushes.append(box_brush(wx, olo, ztop, wx2, ohi, iz2, tex=tex_wall, comment=f"{name} lintel"))
            else:
                brushes.append(box_brush(wx, lo_axis, iz1, wx2, hi_axis, iz2, tex=tex_wall, comment=f"{name} wall"))
        else:
            wy, wy2 = (oy1, iy1) if at_min else (iy2, oy2)
            if opening:
                olo, ohi, ztop = opening
                if olo > lo_axis: brushes.append(box_brush(lo_axis, wy, iz1, olo, wy2, iz2, tex=tex_wall, comment=f"{name} wall A"))
                if ohi < hi_axis: brushes.append(box_brush(ohi, wy, iz1, hi_axis, wy2, iz2, tex=tex_wall, comment=f"{name} wall B"))
                if ztop < iz2: brushes.append(box_brush(olo, wy, ztop, ohi, wy2, iz2, tex=tex_wall, comment=f"{name} lintel"))
            else:
                brushes.append(box_brush(lo_axis, wy, iz1, hi_axis, wy2, iz2, tex=tex_wall, comment=f"{name} wall"))
    return brushes

def corridor_x(interior, wall=16):
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1, iy1 - wall, iz1 - wall, ix2, iy2 + wall, iz1, tex=TEXTURE_FLOOR, comment="corr floor"),
        box_brush(ix1, iy1 - wall, iz2, ix2, iy2 + wall, iz2 + wall, tex=TEXTURE_CEIL, comment="corr ceiling"),
        box_brush(ix1, iy1 - wall, iz1, ix2, iy1, iz2, comment="corr south wall"),
        box_brush(ix1, iy2, iz1, ix2, iy2 + wall, iz2, comment="corr north wall"),
    ]

def corridor_y(interior, wall=16):
    ix1, iy1, iz1, ix2, iy2, iz2 = interior
    return [
        box_brush(ix1 - wall, iy1, iz1 - wall, ix2 + wall, iy2, iz1, tex=TEXTURE_FLOOR, comment="corr floor"),
        box_brush(ix1 - wall, iy1, iz2, ix2 + wall, iy2, iz2 + wall, tex=TEXTURE_CEIL, comment="corr ceiling"),
        box_brush(ix1 - wall, iy1, iz1, ix1, iy2, iz2, comment="corr west wall"),
        box_brush(ix2, iy1, iz1, ix2 + wall, iy2, iz2, comment="corr east wall"),
    ]

def generate_map():
    W = 16
    DOOR_H = 96
    brushes = []

    # --- Portal Zone ---
    r1_int = (0, 0, 0, 256, 256, 128)
    c1_int = (256 + W, 96, 0, 256 + W + 128, 160, DOOR_H)
    r2_int = (c1_int[3] + W, 0, 0, c1_int[3] + W + 256, 512, 128)
    c2_int = (r2_int[0] + 96, 512 + W, 0, r2_int[0] + 160, 512 + W + 128, DOOR_H)

    brushes.append("// ---- Room 1 ----")
    brushes.extend(sealed_room(r1_int, W, opening_east=(96, 160, DOOR_H)))
    brushes.append("// ---- Corridor 1 ----")
    brushes.extend(corridor_x(c1_int, W))
    brushes.append(box_brush(r1_int[3]+W, -W, -W, r2_int[0]-W, c1_int[1], 128+W, tex=TEXTURE_WALL, comment="filler S C1"))
    brushes.append(box_brush(r1_int[3]+W, c1_int[4], -W, r2_int[0]-W, r1_int[4]+W, 128+W, tex=TEXTURE_WALL, comment="filler N C1"))
    brushes.append(box_brush(r1_int[3]+W, c1_int[1]-W, DOOR_H, r2_int[0]-W, c1_int[4]+W, 128+W, tex=TEXTURE_WALL, comment="filler Top C1"))

    brushes.append("// ---- Room 2 ----")
    brushes.extend(sealed_room(r2_int, W, opening_west=(96, 160, DOOR_H), opening_north=(96, 160, DOOR_H)))
    brushes.append("// ---- Corridor 2 ----")
    brushes.extend(corridor_y(c2_int, W))

    # --- Arena Zone ---
    arena_y1 = c2_int[4] + W
    arena_int = (-512, arena_y1, 0, 1536, arena_y1 + 2048, 512)
    
    brushes.append(box_brush(r2_int[0]-W, r2_int[4]+W, -W, c2_int[0], arena_int[1]-W, 512+W, tex=TEXTURE_WALL, comment="filler W C2"))
    brushes.append(box_brush(c2_int[3], r2_int[4]+W, -W, r2_int[3]+W, arena_int[1]-W, 512+W, tex=TEXTURE_WALL, comment="filler E C2"))
    brushes.append(box_brush(c2_int[0]-W, r2_int[4]+W, DOOR_H, c2_int[3]+W, arena_int[1]-W, 512+W, tex=TEXTURE_WALL, comment="filler Top C2"))

    brushes.append("// ---- Huge Arena ----")
    brushes.extend(sealed_room(arena_int, W, opening_south=(c2_int[0], c2_int[3], DOOR_H)))

    # --- HiZ Stress Features ---
    wall_x, wall_y = arena_int[0] + 512, arena_int[1] + 512
    brushes.append(box_brush(wall_x, wall_y, 0, wall_x + 1024, wall_y + 32, 256, comment="Primary Occluder"))

    # Cluster of small objects (HiZ culling target)
    for i in range(8):
        for j in range(8):
            x, y = wall_x + 128 + i * 80, wall_y + 128 + j * 80
            brushes.append(box_brush(x, y, 0, x + 40, y + 40, 32 + (i + j) * 8, tex=TEXTURE_DETAIL, comment="Cluster Obj"))

    # Secondary Occluder (High Verticality)
    brushes.append(box_brush(wall_x - 400, wall_y + 800, 0, wall_x - 300, wall_y + 1800, 512, tex=TEXTURE_WALL, comment="Vertical Occluder"))

    # Scattered Cover
    for k in range(10):
        cx, cy = arena_int[0] + 100 + (k % 4) * 400, arena_int[1] + 200 + (k // 4) * 600
        brushes.append(box_brush(cx, cy, 0, cx+64, cy+64, 96, tex=TEXTURE_COVER, comment="Cover"))

    # --- Nested Building ---
    b_x1, b_y1 = arena_int[0] + 300, arena_int[1] + 1400
    brushes.append("// ---- Nested Building ----")
    brushes.extend(sealed_room((b_x1, b_y1, 0, b_x1 + 200, b_y1 + 200, 128), W, opening_west=(b_y1+64, b_y1+136, DOOR_H), opening_east=(b_y1+64, b_y1+136, DOOR_H), tex_wall=TEXTURE_BUILDING))
    brushes.extend(sealed_room((b_x1+200+W, b_y1, 0, b_x1+400+W, b_y1+200, 128), W, opening_west=(b_y1+64, b_y1+136, DOOR_H), tex_wall=TEXTURE_BUILDING))

    # --- Assemble ---
    lines = ["// Game: Generic", "// Format: Standard", "{", '"classname" "worldspawn"', '"wad" ""']
    brush_num = 0
    for item in brushes:
        if item.startswith("//"): lines.append(item)
        else:
            lines.append(f"// brush {brush_num}")
            lines.append(item)
            brush_num += 1
    lines.append("}")

    spawn_x, spawn_y = (r1_int[0] + r1_int[3]) // 2, (r1_int[1] + r1_int[4]) // 2
    lines.append("// entity 1 (player start)")
    lines.append("{")
    lines.append('"classname" "info_player_start"')
    lines.append(f'"origin" "{spawn_x} {spawn_y} 24"')
    lines.append('"angle" "0"')
    lines.append("}")

    # --- Lights (sub-plan 1 of Lighting Foundation) ---
    # Directional sun for the open arena — steep downward aim.
    arena_cx = (arena_int[0] + arena_int[2]) // 2
    arena_cy = (arena_int[1] + arena_int[3]) // 2
    lines.append("// entity 2 (directional / sun light)")
    lines.append("{")
    lines.append('"classname" "light_sun"')
    lines.append(f'"origin" "{arena_cx} {arena_cy} {arena_int[5] - 32}"')
    lines.append('"light" "200"')
    lines.append('"_color" "220 230 255"')
    lines.append('"delay" "0"')
    lines.append('"angles" "-75 0 0"')
    lines.append("}")

    # Point light in the portal corridor (Corridor 1) — exercises falloff in
    # a tight space. Warm, inverse-squared.
    c1_cx = (c1_int[0] + c1_int[2]) // 2
    c1_cy = (c1_int[1] + c1_int[3]) // 2
    c1_cz = DOOR_H - 16
    lines.append("// entity 3 (point light, corridor)")
    lines.append("{")
    lines.append('"classname" "light"')
    lines.append(f'"origin" "{c1_cx} {c1_cy} {c1_cz}"')
    lines.append('"light" "220"')
    lines.append('"_color" "255 170 90"')
    lines.append('"_fade" "1024"')
    lines.append('"delay" "2"')
    lines.append("}")

    # Point light scattered among the arena detail cluster — fills shadow
    # pockets between the small occluders. Cooler tint, linear falloff.
    cluster_cx = wall_x + 128 + 4 * 80
    cluster_cy = wall_y + 128 + 4 * 80
    lines.append("// entity 4 (point light, arena cluster)")
    lines.append("{")
    lines.append('"classname" "light"')
    lines.append(f'"origin" "{cluster_cx} {cluster_cy} 160"')
    lines.append('"light" "180"')
    lines.append('"_color" "160 200 255"')
    lines.append('"_fade" "1536"')
    lines.append('"delay" "0"')
    lines.append("}")

    return "\n".join(lines)

if __name__ == "__main__":
    from pathlib import Path
    Path("assets/maps/occlusion-test.map").write_text(generate_map() + "\n")
    print("Generated occlusion-test.map")
