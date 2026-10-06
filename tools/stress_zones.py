"""Zoned layout for the environment-volume stress map (`--preset env-volumes`).
Governing doc: `content/dev/maps/stress-env-volumes.README.md`.

The warren is split into zones so water can be checked alone and beside each
other engine feature, and a defect can be attributed to one pairing:

* `water`     -- water-only rooms: every volume shape, static baked lighting,
                 nothing live (no enemies, emitters, movers or crates).
* `lift`      -- (a) both rooms of each lift shaft; every car carries a dynamic
                 light, one settles into a pool, one rises through a block.
* `animated`  -- (b) animated lights above the water and inside a pool.
* `arena`     -- (c) the combat arena: enemies and weapon pickups among pools.
* `particles` -- (d) emitters rising out of pools and through floating blocks,
                 and falling across a zero-gravity layer boundary.
* `fog`       -- (e) fog volumes, a fog lamp and a fog tube overlapping fluid.
* `doors`     -- (f) action doors, their trigger volumes and a switch by water.
* `crates`    -- (g) baked and runtime spots with shadow-casting crates, a
                 crate standing in a pool.
* `lowgrav`   -- (h) a lunar-gravity room holding a pool and a floating block.
* `mixed`     -- everything at stress density: most volumes, enemies,
                 particles, doors, closets, crates, fog, props, a carousel.

Zone assignment is a pure function of the lattice (arena and lift cells are
fixed by the warren); the dedicated single-room zones take the top layer's
first free rooms, so they sit together, apart from the ground-floor horde.
Feature placement here draws from FEATURE_STREAM, never from the volume
stream, so the volume knob cannot move a feature.
"""

import math
import random

from stress_environment import prism_brush, rect_footprint, flat_top

FEATURE_STREAM = 0x20AE5

# Dedicated single-room zones, in assignment order.
DEDICATED_ZONES = ("water", "water", "animated", "particles", "fog", "doors",
                   "crates", "lowgrav")

# Per-zone placement policy. `lights` is the room's light mode (see
# gen_stress_map.emit_room_lights); the flags gate the warren's global
# placements (enemies, weapons, closets, maze doors, animated coverage);
# `spot_mix` guarantees the room both a baked and a runtime spot.
POLICY = {
    "water":     dict(lights="static", crates=0),
    "lift":      dict(lights="static", crates=0),
    "animated":  dict(lights="static", crates=0, animated=True),
    "arena":     dict(lights="static", crates=0, enemies=True, weapons=True),
    "particles": dict(lights="static", crates=0),
    "fog":       dict(lights="static", crates=0),
    "doors":     dict(lights="static", crates=0, doors=True),
    "crates":    dict(lights="mixed", crates=3, spot_mix=True),
    "lowgrav":   dict(lights="static", crates=0),
    "mixed":     dict(lights="mixed", crates=1, enemies=True, weapons=True,
                      closets=True, doors=True),
}

ZONE_LABELS = {
    "water": "water only",
    "lift": "(a) light-carrying lifts + pool / block",
    "animated": "(b) animated lights over and under water",
    "arena": "(c) arena: enemies + weapons among pools",
    "particles": "(d) emitters through pools, blocks, gravity boundary",
    "fog": "(e) fog volume / lamp / tube over fluid",
    "doors": "(f) action doors + triggers + switch by water",
    "crates": "(g) baked + runtime spots, crates in water",
    "lowgrav": "(h) low gravity, pool + floating block",
    "mixed": "everything at stress density",
}

ARENA_ENEMIES = 12        # of --enemies; the rest go to the mixed zone
PARTICLE_ZONE_EMITTERS = 4
MIXED_FOG = 4             # fog_volume brushes in mixed rooms (cap is 16 total)
MIXED_PROPS = 4
PROP_MODEL = "models/decraniated_low_poly_retro_pixel/scene.gltf"
FEATURE_TEX = "50-free-textures/concrete_stone_022"
UNDERWATER_LIGHT_OFFSET = 160
UNDERWATER_LIGHTS = 2     # animated lights in the animated zone's pool


def policy(zone, key):
    return POLICY[zone].get(key, False)


def assign_zones(nx, ny, nz, arena_cells, lift_cells):
    """Map every lattice cell `(i, j, k)` to its zone name."""
    zones = {cell: "arena" for cell in arena_cells}
    for cell in lift_cells:
        zones.setdefault(cell, "lift")
    top = nz - 1
    free = sorted(((i, j, top) for j in range(ny) for i in range(nx)
                   if (i, j, top) not in zones), key=lambda c: (c[1], c[0]))
    for zone, cell in zip(DEDICATED_ZONES, free):
        zones[cell] = zone
    for k in range(nz):
        for j in range(ny):
            for i in range(nx):
                zones.setdefault((i, j, k), "mixed")
    return zones


def engine_pose(rect, units_to_m=0.0254):
    """A `--start-pose` near a room corner facing its centre (engine meters).

    Map (x, y, z) -> engine (-y, z, -x); the pawn origin sits 32 u above the
    floor like `player_spawn`; engine yaw 0 faces -Z.
    """
    zf, _, x0, x1, y0, y1 = rect
    px, py = x0 + 160, y0 + 160
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    ex, ey, ez = -py * units_to_m, (zf + 32) * units_to_m, -px * units_to_m
    dx, dz = -(cy - py), -(cx - px)          # map delta -> engine delta
    yaw = math.degrees(math.atan2(-dx, -dz))
    return f"{ex:.2f},{ey:.2f},{ez:.2f},{yaw:.0f},0"


def _fog_volume(brush, density):
    return (["{", '"classname" "fog_volume"', f'"density" "{density}"',
             '"glow" "0.6"', '"tint" "180 220 255"', '"edge_softness" "24"']
            + brush + ["}"])


def zone_feature_entities(seed, rooms, door_tags, light_entity, underwater_lights=True):
    """Zone-specific features, plus the volume anchors they imply.

    `rooms` is `[(zone, rect)]` in room order; `door_tags` maps a room index
    to the tags of maze doors on its walls; `light_entity` is the generator's
    light factory. `underwater_lights` is false when the map animates nothing
    (the generator reserves their animation budget only when true). Returns
    `(entities, anchors, script_lights)` where `anchors` is
    `[(room_index, anchor)]` for `stress_environment`.
    """
    rng = random.Random(seed ^ FEATURE_STREAM)
    entities, anchors = [], []
    script_lights = 0
    mixed = [n for n, (zone, _) in enumerate(rooms) if zone == "mixed"]

    for n, (zone, rect) in enumerate(rooms):
        zf, zc, x0, x1, y0, y1 = rect
        cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
        if zone == "animated":
            # Two animated lights inside a pool centred on the room: one KVP
            # curve, one script pulse (UNDERWATER_LIGHTS of the map's animated
            # budget). The light factory draws nothing, so omitting them moves
            # no other feature.
            if underwater_lights:
                off = UNDERWATER_LIGHT_OFFSET
                entities.append(light_entity("static", (cx - off, cy, zf + 24),
                                             (0, 200, 255), 600, 160, False, rng,
                                             animate="kvp"))
                entities.append(light_entity("static", (cx + off, cy, zf + 24),
                                             (0, 255, 160), 600, 160, False, rng,
                                             animate="script"))
                script_lights += 1
            anchors.append((n, ("pool", cx, cy, zf)))
        elif zone == "fog":
            # Axis-aligned fog box (ellipsoid fog) over a pool, a yawed
            # plane-bounded fog brush, a fog lamp and a fog tube.
            ax = cx - 224
            box = prism_brush(rect_footprint(ax, cy, 384, 384), zf, flat_top(zf + 384),
                              FEATURE_TEX)
            entities.append(_fog_volume(box, 0.5))
            anchors.append((n, ("fog_box", ax, cy, zf + 200)))
            yawed = prism_brush(rect_footprint(cx + 224, cy, 320, 256, 30), zf + 64,
                                flat_top(zf + 320), FEATURE_TEX)
            entities.append(_fog_volume(yawed, 0.4))
            anchors.append((n, ("pool", cx + 224, cy, zf)))
            entities.append(["{", '"classname" "fog_lamp"',
                             f'"origin" "{cx} {cy + 256} {zf + 64}"', '"radius" "128"',
                             '"density" "0.6"', "}"])
            entities.append(["{", '"classname" "fog_tube"',
                             f'"origin" "{cx} {cy - 256} {zf + 96}"', '"radius" "64"',
                             '"height" "256"', '"yaw" "45"', '"density" "0.4"', "}"])
        elif zone == "doors":
            # A switch pedestal that starts one of this room's doors, standing
            # beside a pool. The generator forces this room a doorway and
            # places its door first; the zone index flags a room left without.
            if door_tags.get(n):
                sx, sy = cx, cy - 256
                switch = prism_brush(rect_footprint(sx, sy, 48, 48), zf, flat_top(zf + 64),
                                     FEATURE_TEX)
                entities.append(["{", '"classname" "switch"',
                                 f'"name" "env_switch_{n}"',
                                 f'"target_tag" "{door_tags[n][0]}"', '"command" "start"',
                                 '"fire_mode" "multiple"', '"rearm_ms" "1000"']
                                + switch + ["}"])
                anchors.append((n, ("pool", sx, sy + 352, zf)))

    if mixed:
        # A carousel spinning in the upper storey carrying an orbiting dynamic
        # light. Its sweep (radius 160) stays on the room's +Y side, clear of a
        # monster closet's walls (which reach y0 + 640) and above crate stacks.
        n = mixed[rng.randrange(len(mixed))]
        zf, zc, x0, x1, y0, y1 = rooms[n][1]
        px, py, pz = (x0 + x1) // 2, (y0 + y1) // 2 + 300, zf + 384
        arm = prism_brush(rect_footprint(px, py, 320, 48), pz, flat_top(pz + 24),
                          FEATURE_TEX)
        entities.append(["{", '"classname" "kinematic_mover"', '"name" "env_carousel"',
                         '"path" "env_carousel_pivot"', '"speed" "1"', '"move_mode" "once"',
                         '"start_on_spawn" "1"', '"spin_axis" "0 0 1"', '"spin_speed" "45"',
                         '"spin_accel" "180"', '"_tags" "env_carousel"'] + arm + ["}"])
        entities.append(["{", '"classname" "kinematic_waypoint"',
                         '"name" "env_carousel_pivot"', f'"origin" "{px} {py} {pz + 12}"',
                         "}"])
        entities.append(light_entity("dynamic", (px + 144, py, pz - 32), (255, 120, 40),
                                     500, 300, False, rng, carrier="env_carousel"))
        # Fog volumes and static props scattered over mixed rooms.
        for f in range(MIXED_FOG):
            zf, zc, x0, x1, y0, y1 = rooms[mixed[rng.randrange(len(mixed))]][1]
            fx, fy = rng.randint(x0 + 256, x1 - 256), rng.randint(y0 + 256, y1 - 256)
            yaw = 0 if f % 2 == 0 else 30
            entities.append(_fog_volume(
                prism_brush(rect_footprint(fx, fy, 320, 320, yaw), zf, flat_top(zf + 256),
                            FEATURE_TEX), 0.3))
        for _ in range(MIXED_PROPS):
            zf, zc, x0, x1, y0, y1 = rooms[mixed[rng.randrange(len(mixed))]][1]
            entities.append(["{", '"classname" "prop_mesh"',
                             f'"origin" "{x1 - 128} {y1 - 128} {zf}"',
                             f'"model" "{PROP_MODEL}"', '"angles" "0 180 0"', "}"])
    return entities, anchors, script_lights
