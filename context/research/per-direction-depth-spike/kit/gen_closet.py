#!/usr/bin/env python3
"""Generate mover-closet.map, or its static-door reference with --static-door."""
import sys

CONC = "50-free-textures/concrete_stone_033"
FLOOR = "50-free-textures/concrete_pavement_036"
CEIL = "debug_ceiling_grey"
DOOR = "metal/metal_rough_046a"
BLUE = "DebugTextures/debug_floor_blue"


def box(name, lo, hi, tex):
    x0, y0, z0 = lo
    x1, y1, z1 = hi
    rows = [
        ((x0, y1, z0), (x0, y0, z1), (x0, y0, z0)),
        ((x0, y0, z1), (x1, y0, z0), (x0, y0, z0)),
        ((x0, y0, z0), (x1, y1, z0), (x0, y1, z0)),
        ((x0, y1, z1), (x1, y0, z1), (x0, y0, z1)),
        ((x1, y1, z1), (x0, y1, z1), (x0, y1, z0)),
        ((x1, y0, z0), (x1, y1, z1), (x1, y1, z0)),
    ]
    out = [f"// brush {name}", "{"]
    for a, b, c in rows:
        p = " ".join("( %d %d %d )" % v for v in (a, b, c))
        out.append(f"{p} {tex} 0 0 0 1 1")
    out.append("}")
    return out


NORTH = 760
for a in __import__("sys").argv:
    if a.startswith("--north="):
        NORTH = int(a.split("=")[1])

WORLD = [
    ("floor_bright", (-16, -16, -16), (656, 512, 0), FLOOR),
    ("ceiling_bright", (-16, -16, 320), (656, 512, 336), CEIL),
    ("wall_west", (-16, -16, 0), (0, 448, 320), CONC),
    ("wall_east", (640, -16, 0), (656, 448, 320), CONC),
    ("wall_south", (0, -16, 0), (640, 0, 320), CONC),
    ("divider_jamb_west", (-16, 448, 0), (272, 512, 320), CONC),
    ("divider_jamb_east", (368, 448, 0), (656, 512, 320), CONC),
    ("divider_lintel", (272, 448, 112), (368, 512, 320), CONC),
    ("closet_floor_south", (144, 512, -40), (496, 544, 0), FLOOR),
    ("closet_floor_north", (144, 688, -40), (496, NORTH + 16, 0), FLOOR),
    ("closet_floor_west", (144, 544, -40), (224, 688, 0), FLOOR),
    ("closet_floor_east", (416, 544, -40), (496, 688, 0), FLOOR),
    ("closet_pool_bottom", (224, 544, -40), (416, 688, -24), FLOOR),
    ("closet_wall_west", (144, 512, 0), (160, NORTH + 16, 128), CONC),
    ("closet_wall_east", (480, 512, 0), (496, NORTH + 16, 128), CONC),
    ("closet_wall_north", (160, NORTH, 0), (480, NORTH + 16, 128), CONC),
    ("closet_ceiling", (144, 512, 128), (496, NORTH + 16, 144), CEIL),
]
DOOR_LO, DOOR_HI = (272, 488, 0), (368, 504, 112)
if "--flush" in sys.argv:
    DOOR_LO, DOOR_HI = (272, 496, 0), (368, 512, 112)


def main():
    static_door = "--static-door" in sys.argv
    no_door = "--no-door" in sys.argv
    o = [
        "// Game: Postretro",
        "// Format: Standard",
    ]
    if static_door:
        o.append("// Static-door reference for mover-closet: door baked as a world brush.")
    else:
        o += [
            "// Mover closet: leak-diagnosis and proof surface for mover light flow.",
            "// See mover-closet.README.md for the layout, stations and the reference build.",
            "// Compile: cargo run -p postretro-level-compiler -- content/dev/maps/mover-closet.map -o content/dev/maps/mover-closet.prl",
        ]
    o += [
        "// Layout, TrenchBroom units, 1 unit = 0.0254 m, floor top at z 0.",
        "// Bright room: x 0 to 640, y 0 to 448, ceiling z 320.",
        "// Divider: y 448 to 512, 1.6 m thick, with a doorway at x 272 to 368, z 0 to 112.",
        "// Door: y 488 to 504, 16 units thick, 0.41 m against a 1 m probe spacing.",
        "// Closet: x 160 to 480, y 512 to 760, ceiling z 128, no lights.",
        "// Pool: pit x 224 to 416, y 544 to 688, bottom z -24, surface z -8.",
        "// The north wall sets the probe lattice: planes fall at y 484.4 and 523.8, either side of the door.",
        "// Every brush is an axis-aligned box. Keep comments free of braces and quotes.",
        "// entity 0",
        "{",
        '"classname" "worldspawn"',
        '"wad" ""',
        '"_tb_mod" "dev"',
        '"_tb_def" "external:../../../sdk/TrenchBroom/postretro.fgd"',
    ]
    for name, lo, hi, tex in WORLD:
        o += box(name, lo, hi, tex)
    if static_door:
        o += box("closet_door_static", DOOR_LO, DOOR_HI, DOOR)
    o.append("}")
    if not static_door and not no_door:
        o += [
            "// Door: closed at waypoint 0, slides up into the lintel.",
            "{",
            '"classname" "kinematic_mover"',
            '"name" "closet_door"',
            '"path" "closet_door_closed"',
            '"speed" "1"',
            '"wait_ms" "3000"',
            '"move_mode" "ping_pong"',
            '"start_on_spawn" "1"',
            '"_tags" "dev mover_closet closet_door"',
        ]
        o += box("door_slab", DOOR_LO, DOOR_HI, DOOR)
        o += [
            "}",
            "{",
            '"classname" "kinematic_waypoint"',
            '"name" "closet_door_closed"',
            '"next" "closet_door_open"',
            '"origin" "320 496 56"',
            "}",
            "{",
            '"classname" "kinematic_waypoint"',
            '"name" "closet_door_open"',
            '"origin" "320 496 168"',
            "}",
        ]
    o += [
        "// Shallow pool in the closet floor.",
        "{",
        '"classname" "fluid_volume"',
        '"fluid" "water"',
    ]
    o += box("closet_water", (224, 544, -24), (416, 688, -8), BLUE)
    o += [
        "}",
        "// Closet fog fills the room.",
        "{",
        '"classname" "fog_volume"',
        '"density" "0.3"',
        '"edge_softness" "0"',
        '"falloff" "1"',
        '"_tags" "dev mover_closet closet_fog"',
    ]
    o += box("closet_fog", (160, 512, -8), (480, NORTH, 128), BLUE)
    o += ["}", "// Bright-room lights hug the divider, high enough that the lintel cuts every line into the closet."]
    for x in (128, 320, 512):
        o += [
            "{",
            '"classname" "light"',
            f'"origin" "{x} 392 288"',
            '"light" "400"',
            '"_color" "255 244 230"',
            '"_falloff_range" "900"',
            "}",
        ]
    o += [
        "// Station 1: the closed door ahead. Yaw 90 faces +y.",
        "{",
        '"classname" "player_spawn"',
        '"origin" "320 200 48"',
        '"angles" "0 90 0"',
        "}",
    ]
    print("\n".join(o))


main()
