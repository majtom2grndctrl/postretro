#!/usr/bin/env python3
"""Generate a tiny, purpose-built SDF-shadow test fixture for Postretro.

Purpose
-------
The level-compiler SDF tests need a `.map` with a CLEAR, by-construction shadow
case: a floor receiver, an `_shadow_type "sdf"` light overhead, and a solid
occluder slab floating between PART of the floor and the light. The fixture is
deliberately small so an in-process SDF bake (and chunk-light-list bake) is fast.

Two tests load the committed map this script emits:
  * crates/level-compiler/src/sdf_bake.rs
        sdf_atlas_marks_occluder_between_floor_and_sdf_light
  * crates/level-compiler/src/chunk_light_list_bake.rs
        chunk_light_list_includes_sdf_light_for_shadowed_floor

The shadow case (why it is shadowed/lit BY CONSTRUCTION)
-------------------------------------------------------
All coordinates below are Quake units (Z-up); prl-build applies the
1 unit = 0.0254 m scale and the Z-up -> Y-up swizzle
(engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x).

* A sealed box room: a solid floor slab, four walls, and a ceiling, so the BSP
  flood-fill has a closed interior and does not delete the geometry (modelled on
  content/dev/maps/soft_shadow_test.map).
* A free-floating SOLID occluder brick high in the room, directly over ONE floor
  point. A solid brush's interior classifies as solid in the BSP, so the SDF bake
  marks it negative/surface. The brick is kept COMPACT (small footprint relative
  to the 8 m chunk-light-list cell) so it shadows the receiver's vertical ray
  without occluding the whole cell -- see the chunk-light-list test note.
* One `light` with `_shadow_type "sdf"` placed directly above the brick (its X/Y
  center), high under the ceiling.
* SHADOWED floor point: directly under the brick and the light. The straight
  floor->light segment is vertical and passes through the occluder brick.
* LIT floor point: far across the room in +X, in the open. Its floor->light
  segment is angled and clears the occluder entirely (the brick does not extend
  that far in X), so nothing occludes it.

The light's `_falloff_range` is generous enough to reach BOTH floor points so
the only difference between them is the occluder, not range attenuation.

Usage
-----
    python3 tools/gen_sdf_shadow_fixture.py
    python3 tools/gen_sdf_shadow_fixture.py -o content/dev/maps/sdf-shadow-test.map

The generated map is committed; the tests load the `.map`, they do NOT run this
script. Do not hand-edit the generated file — regenerate it from this script.
"""

import argparse
import sys

# Texture from the bundled "50-free-textures" collection (matches the other dev
# fixtures). The shadow case is about geometry, not materials, so one suffices.
TEX = "50-free-textures/concrete_stone_021"

# --- Fixture geometry (Quake units, Z-up). Round, human-legible numbers. ----
# Sealed room interior: X in [0,1024], Y in [0,512], Z in [0,384].
ROOM_X = (0, 1024)
ROOM_Y = (0, 512)
ROOM_Z = (0, 384)
SHELL_T = 32          # floor/wall/ceiling slab thickness (the sealing hull)

# Free-floating solid occluder brick, directly above the SHADOWED floor point
# and below the light. Kept COMPACT in footprint (~96 u = 2.44 m) relative to the
# 8 m chunk-light-list cell: the chunk bake keeps a light for a cell when ANY of
# the cell's light-facing sample points has a clear line to the light, so a small
# brick over the receiver shadows the receiver's vertical ray yet leaves the rest
# of the cell open -- the SDF light survives into the shadowed-floor cell (the
# property `chunk_light_list_includes_sdf_light_for_shadowed_floor` asserts) while
# the receiver itself is occluded (the property `sdf_atlas_marks_occluder...`
# asserts). It floats high in the room, well above the floor.
OCC_X = (368, 464)    # X span (96 u, centered on the shadowed floor point x=416)
OCC_Y = (208, 304)    # Y span (96 u, centered on y=256)
OCC_Z = (160, 224)    # Z span (floats high, between floor and light)

# SDF light: directly above the occluder center (X=416, Y=256), high under the
# ceiling, so the vertical floor->light segment under it pierces the brick.
LIGHT = (416, 256, 352)
LIGHT_INTENSITY = 300
LIGHT_COLOR = (255, 240, 220)
# Falloff in Quake units (inches). 800 u * 0.0254 = 20.32 m reaches both floor
# points (the farther LIT point is ~15 m from the light in engine space).
LIGHT_FALLOFF = 800

# Receiver floor points (just above the floor surface, in air) the tests probe.
# SHADOWED: directly under the occluder + light. LIT: far +X, in the open.
SHADOWED_FLOOR = (416, 256, 4)
LIT_FLOOR = (896, 256, 4)

# Player spawn (the other dev fixtures author one; keep the format complete).
SPAWN = (512, 256, 48)


def box_brush(x0, y0, z0, x1, y1, z1):
    """An axis-aligned solid box as a 6-plane Standard-format brush.

    Winding/point order mirrors the known-good box in gen_stress_map.py so the
    plane normals face outward (solid interior behind each plane).
    """
    t = TEX
    s = lambda: f"{t} 0 0 0 1 1"
    return (
        "{\n"
        f"( {x0} {y1} {z0} ) ( {x0} {y0} {z1} ) ( {x0} {y0} {z0} ) {s()}\n"   # -X
        f"( {x0} {y0} {z0} ) ( {x1} {y0} {z1} ) ( {x1} {y0} {z0} ) {s()}\n"   # -Y
        f"( {x0} {y0} {z0} ) ( {x1} {y1} {z0} ) ( {x0} {y1} {z0} ) {s()}\n"   # -Z
        f"( {x0} {y1} {z1} ) ( {x1} {y0} {z1} ) ( {x0} {y0} {z1} ) {s()}\n"   # +Z
        f"( {x1} {y1} {z0} ) ( {x0} {y1} {z1} ) ( {x0} {y1} {z0} ) {s()}\n"   # +Y
        f"( {x1} {y0} {z0} ) ( {x1} {y1} {z1} ) ( {x1} {y1} {z0} ) {s()}\n"   # +X
        "}\n"
    )


def room_brushes():
    """The sealed hull: floor, ceiling, and four walls wrapping the interior.

    Each shell brush is SHELL_T thick and sits OUTSIDE the [ROOM_*] interior, so
    the interior air volume is exactly [ROOM_X] x [ROOM_Y] x [ROOM_Z].
    """
    x0, x1 = ROOM_X
    y0, y1 = ROOM_Y
    z0, z1 = ROOM_Z
    t = SHELL_T
    return [
        box_brush(x0 - t, y0 - t, z0 - t, x1 + t, y1 + t, z0),          # floor
        box_brush(x0 - t, y0 - t, z1, x1 + t, y1 + t, z1 + t),          # ceiling
        box_brush(x0 - t, y0 - t, z0, x0, y1 + t, z1),                  # -X wall
        box_brush(x1, y0 - t, z0, x1 + t, y1 + t, z1),                  # +X wall
        box_brush(x0, y0 - t, z0, x1, y0, z1),                         # -Y wall
        box_brush(x0, y1, z0, x1, y1 + t, z1),                         # +Y wall
    ]


def write_map(path):
    lines = []
    lines.append("// Game: Postretro")
    lines.append("// Format: Standard")
    lines.append("// SDF-shadow test fixture (generated by tools/gen_sdf_shadow_fixture.py")
    lines.append("// -- do not hand-edit; regenerate from the script).")
    # NOTE: never emit a bare `//` comment line — shalrath's .map comment grammar
    # rejects an empty comment and silently drops every entity ("no worldspawn").
    lines.append("// A sealed box room with a compact solid occluder brick floating over one")
    lines.append("// floor point, plus one `_shadow_type \"sdf\"` light directly above")
    lines.append("// the occluder. By construction the floor point under the occluder is")
    lines.append("// SHADOWED (its straight line to the light passes through the slab) and a")
    lines.append("// floor point out in the open is LIT (its line to the light is clear).")
    lines.append(f"// SHADOWED floor (quake units): {SHADOWED_FLOOR}")
    lines.append(f"// LIT floor (quake units):      {LIT_FLOOR}")
    lines.append(f"// SDF light (quake units):      {LIGHT}")
    lines.append(f"// Occluder slab X{OCC_X} Y{OCC_Y} Z{OCC_Z} (quake units).")
    lines.append("// entity 0")
    lines.append("{")
    lines.append('"classname" "worldspawn"')
    lines.append('"initialGravity" "-9.81"')
    lines.append('"ambient_color" "32 32 40"')
    lines.append('"wad" ""')
    lines.append('"_tb_mod" "dev"')

    brushes = room_brushes()
    # The free-floating solid occluder slab.
    brushes.append(box_brush(OCC_X[0], OCC_Y[0], OCC_Z[0],
                             OCC_X[1], OCC_Y[1], OCC_Z[1]))

    for n, b in enumerate(brushes):
        lines.append(f"// brush {n}")
        lines.append(b.rstrip("\n"))
    lines.append("}")

    lines.append("// entity 1")
    lines.append("{")
    lines.append('"classname" "player_spawn"')
    lines.append(f'"origin" "{SPAWN[0]} {SPAWN[1]} {SPAWN[2]}"')
    lines.append('"angle" "0"')
    lines.append("}")

    lines.append("// entity 2 -- the SDF-shadow light over the occluder.")
    lines.append("{")
    lines.append('"classname" "light"')
    lines.append('"_shadow_type" "sdf"')
    lines.append(f'"origin" "{LIGHT[0]} {LIGHT[1]} {LIGHT[2]}"')
    lines.append(f'"light" "{LIGHT_INTENSITY}"')
    lines.append(f'"_color" "{LIGHT_COLOR[0]} {LIGHT_COLOR[1]} {LIGHT_COLOR[2]}"')
    lines.append(f'"_falloff_range" "{LIGHT_FALLOFF}"')
    lines.append('"delay" "0"')
    lines.append('"style" "0"')
    lines.append("}")

    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def main(argv):
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-o", "--out",
                    default="content/dev/maps/sdf-shadow-test.map",
                    help="output .map path (default content/dev/maps/sdf-shadow-test.map)")
    args = ap.parse_args(argv)

    write_map(args.out)
    print(f"wrote {args.out}")
    print(f"  room interior (quake u): X{ROOM_X} Y{ROOM_Y} Z{ROOM_Z}")
    print(f"  occluder brick (quake u): X{OCC_X} Y{OCC_Y} Z{OCC_Z}")
    print(f"  sdf light (quake u): {LIGHT} falloff {LIGHT_FALLOFF}u "
          f"({LIGHT_FALLOFF * 0.0254:.2f} m)")
    print(f"  shadowed floor {SHADOWED_FLOOR}  lit floor {LIT_FLOOR}")


if __name__ == "__main__":
    main(sys.argv[1:])
