"""Emissive-volume GPU and CPU measurement fixture.

Fixture summary: rendering_pipeline.md §12, Emissive-volume fixture. The full
run procedure is context/plans/done/emissive-volumes/measurement.md.

One sealed room, two measurement views from one pedestal:

* North (yaw 90): a 16 x 16 grid of glowing panels (the bright panel texture)
  filling the view. Each panel is its own brush, so its front face is one
  authored face.
* South (yaw -90): the same grid in a texture with no `_e` map. Nothing there
  samples an emissive texture, so the view isolates the per-vertex volume-id
  stream every forward draw binds.

The volume count is the only thing that varies between variants, and volume
entities come after every other entity, so every other section of the
compiled maps matches:

* `--volumes 0`: no volume; every panel presents its texture strength.
* `--volumes 1`: one volume around every glowing panel.
* `--volumes 256`: one volume per glowing panel.
* `--animate N`: N of the 256 volumes (evenly spread) carry `stress_anim`,
  and a generated data script pulses them from level load. Without it the
  map names no data script, so the baseline compiler (before emissive
  volumes existed) can build the 0-volume variant.

Map axes (Quake units, Z up). Nothing here draws randomness.
"""

import argparse
import sys
from pathlib import Path

UNIT_M = 0.0254

GRID = 16              # panels per side
PANEL = 48             # panel edge, units
PANEL_DEPTH = 8
ROOM_X = GRID * PANEL  # 768
ROOM_Y = 496
ROOM_Z = 800
WALL = 16
PANEL_Z0 = 16          # bottom of the grid
PEDESTAL = (352, 416, 216, 280, 360)  # x0, x1, y0, y1, top
VOLUME_INSET = 4

GLOW_TEXTURE = "neon/glow_panel"
DARK_TEXTURE = "metal/metal_rough_046a"
WALL_TEXTURE = "concrete_stone_033"
FLOOR_TEXTURE = "debug_ceiling_grey"
VOLUME_TEXTURE = "trigger"

ANIM_TAG = "stress_anim"
STATIC_TAG = "stress_static"
PULSE_PERIOD_MS = 1500

DEFAULT_MAP = "content/dev/maps/emissive-volumes-stress-{variant}.map"
DEFAULT_SCRIPT = "content/dev/scripts/emissive-volumes-stress.ts"


def box(x0, x1, y0, y1, z0, z1, texture, faces=None):
    """A six-sided axis-aligned brush, plane order and winding as the dev maps
    write them. `faces` overrides the texture per side: `xmin`, `xmax`,
    `ymin`, `ymax`, `zmin`, `zmax`."""
    faces = faces or {}

    def tex(side):
        return faces.get(side, texture)

    return [
        "{",
        f"( {x1} {y0} {z0} ) ( {x1} {y1} {z1} ) ( {x1} {y1} {z0} ) {tex('xmax')} 0 0 0 1 1",
        f"( {x0} {y1} {z0} ) ( {x0} {y0} {z1} ) ( {x0} {y0} {z0} ) {tex('xmin')} 0 0 0 1 1",
        f"( {x1} {y1} {z0} ) ( {x0} {y1} {z1} ) ( {x0} {y1} {z0} ) {tex('ymax')} 0 0 0 1 1",
        f"( {x0} {y0} {z0} ) ( {x1} {y0} {z1} ) ( {x1} {y0} {z0} ) {tex('ymin')} 0 0 0 1 1",
        f"( {x0} {y1} {z1} ) ( {x1} {y0} {z1} ) ( {x0} {y0} {z1} ) {tex('zmax')} 0 0 0 1 1",
        f"( {x0} {y0} {z0} ) ( {x1} {y1} {z0} ) ( {x0} {y1} {z0} ) {tex('zmin')} 0 0 0 1 1",
        "}",
    ]


def panel_cells():
    """Each panel's (x0, z0), row-major from the bottom-left."""
    return [
        (col * PANEL, PANEL_Z0 + row * PANEL)
        for row in range(GRID)
        for col in range(GRID)
    ]


def world_brushes(glow_texture):
    hull = []
    w = WALL
    hull += box(-w, ROOM_X + w, -w, ROOM_Y + w, -w, 0, FLOOR_TEXTURE)            # floor
    hull += box(-w, ROOM_X + w, -w, ROOM_Y + w, ROOM_Z, ROOM_Z + w, FLOOR_TEXTURE)  # ceiling
    hull += box(-w, 0, 0, ROOM_Y, 0, ROOM_Z, WALL_TEXTURE)                       # west
    hull += box(ROOM_X, ROOM_X + w, 0, ROOM_Y, 0, ROOM_Z, WALL_TEXTURE)          # east
    hull += box(-w, ROOM_X + w, -w, 0, 0, ROOM_Z, WALL_TEXTURE)                  # south
    hull += box(-w, ROOM_X + w, ROOM_Y, ROOM_Y + w, 0, ROOM_Z, WALL_TEXTURE)     # north
    x0, x1, y0, y1, top = PEDESTAL
    hull += box(x0, x1, y0, y1, 0, top, DARK_TEXTURE)

    panels = []
    for x, z in panel_cells():
        # North grid: the glowing face points south, at the pedestal.
        panels += box(x, x + PANEL, ROOM_Y - PANEL_DEPTH, ROOM_Y, z, z + PANEL,
                      DARK_TEXTURE, {"ymin": glow_texture})
    for x, z in panel_cells():
        # South grid: the same panels with no `_e` map, facing north.
        panels += box(x, x + PANEL, 0, PANEL_DEPTH, z, z + PANEL, DARK_TEXTURE)
    return hull + panels


def volume_entity(x0, x1, z0, z1, tag):
    face_y = ROOM_Y - PANEL_DEPTH
    return (
        ["{", '"classname" "emissive_volume"', '"emissive_min" "0"',
         '"emissive_max" "1"', f'"_tags" "{tag}"']
        + box(x0, x1, face_y - 6, face_y + 2, z0, z1, VOLUME_TEXTURE)
        + ["}"]
    )


def animated_indices(volumes, animate):
    """`animate` volume indices spread over `volumes`: one per stride, stepping
    one column each time, so 16 of 256 run down the grid's diagonal."""
    if animate == 0:
        return set()
    stride = volumes / animate
    spread = {(int(i * stride) + i) % volumes for i in range(animate)}
    if len(spread) == animate:
        return spread
    return {int(i * stride) for i in range(animate)}


def volume_entities(volumes, animate):
    if volumes == 0:
        return []
    i = VOLUME_INSET
    if volumes == 1:
        tag = ANIM_TAG if animate else STATIC_TAG
        return volume_entity(i, ROOM_X - i, PANEL_Z0 + i, PANEL_Z0 + GRID * PANEL - i, tag)
    animated = animated_indices(volumes, animate)
    out = []
    for index, (x, z) in enumerate(panel_cells()):
        tag = ANIM_TAG if index in animated else STATIC_TAG
        out += volume_entity(x + i, x + PANEL - i, z + i, z + PANEL - i, tag)
    return out


def map_text(volumes, animate, glow_texture, script_kvp):
    x0, x1, y0, y1, top = PEDESTAL
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    worldspawn = ['"classname" "worldspawn"', '"wad" ""']
    if script_kvp:
        worldspawn.append(f'"data_script" "{script_kvp}"')
    worldspawn += ['"_tb_mod" "dev"',
                   '"_tb_def" "external:../../../sdk/TrenchBroom/postretro.fgd"']
    lines = [
        "// Game: Postretro",
        "// Format: Standard",
        "// Generated by tools/gen_emissive_stress.py; do not hand-edit.",
        f"// Emissive-volume measurement fixture: {volumes} volumes, {animate} animating.",
        "{",
        *worldspawn,
        *world_brushes(glow_texture),
        "}",
        "{",
        '"classname" "player_spawn"',
        f'"origin" "{cx} {cy} {top + 24}"',
        '"angles" "0 90 0"',
        "}",
    ]
    for ly in (ROOM_Y // 4, 3 * ROOM_Y // 4):
        lines += ["{", '"classname" "light"', f'"origin" "{cx} {ly} {ROOM_Z - 64}"',
                  '"light" "300"', '"_color" "255 255 255"', '"_falloff_range" "1200"', "}"]
    lines += volume_entities(volumes, animate)
    return "\n".join(lines) + "\n"


def script_text():
    return (
        "// Generated by tools/gen_emissive_stress.py; do not hand-edit.\n"
        "// Pulses every volume tagged `stress_anim` from level load.\n"
        'import { defineReaction, getMapEntities } from "postretro";\n'
        "\n"
        "export function setupLevel(_ctx: unknown) {\n"
        f'  const volumes = getMapEntities("emissive", {{ tag: "{ANIM_TAG}" }});\n'
        "  return {\n"
        "    reactions: [\n"
        '      defineReaction("levelLoad", {\n'
        "        sequence: volumes.flatMap((volume) =>\n"
        f"          volume.pulse({{ min: 0, max: 1, periodMs: {PULSE_PERIOD_MS} }}),\n"
        "        ),\n"
        "      }),\n"
        "    ],\n"
        "  };\n"
        "}\n"
    )


def engine_pose(facing_deg):
    """The pedestal pose (engine metres): map (x, y, z) -> engine (-y, z, -x),
    the pawn origin 32 u above the feet; yaw 90 faces map +Y."""
    x0, x1, y0, y1, top = PEDESTAL
    x, y = (x0 + x1) / 2, (y0 + y1) / 2
    return f"{-y * UNIT_M:.2f},{(top + 32) * UNIT_M:.2f},{-x * UNIT_M:.2f},{facing_deg},0"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--volumes", type=int, choices=(0, 1, 256), default=256)
    parser.add_argument("--animate", type=int, default=0,
                        help="volumes that pulse from load (needs --volumes 1 or 256)")
    parser.add_argument("--glow-texture", default=GLOW_TEXTURE,
                        help="the glowing panels' texture; the pre-brief baseline "
                             "names neon/neon_glow_panel")
    parser.add_argument("-o", "--output", help="map path (default: content/dev/maps/"
                        "emissive-volumes-stress-<variant>.map)")
    parser.add_argument("--script", default=DEFAULT_SCRIPT,
                        help="data script written beside an animating variant")
    args = parser.parse_args(argv)

    if args.animate < 0 or args.animate > args.volumes:
        parser.error("--animate must be between 0 and --volumes")
    if args.volumes == 1 and args.animate not in (0, 1):
        parser.error("--volumes 1 animates 0 or 1")

    variant = f"{args.volumes}" + (f"-anim{args.animate}" if args.animate else "")
    output = Path(args.output or DEFAULT_MAP.format(variant=variant))
    script_kvp = None
    if args.animate:
        script = Path(args.script)
        script.parent.mkdir(parents=True, exist_ok=True)
        script.write_text(script_text())
        script_kvp = Path("..", "scripts", script.name).as_posix()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(map_text(args.volumes, args.animate, args.glow_texture, script_kvp))
    print(f"wrote {output}: {args.volumes} volumes, {args.animate} animating")
    print(f"glow view    --start-pose={engine_pose(90)}")
    print(f"no-_e view   --start-pose={engine_pose(-90)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
