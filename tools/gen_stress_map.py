#!/usr/bin/env python3
"""Generate a dense, multi-layer "warren" stress map for Postretro.

Purpose
-------
The runtime currently walks the whole geometry/BVH every frame. To find out
whether that is a real bottleneck (and to later validate BVH node-visibility
masks) we need a map that pushes the *room/node count* as high as possible
while staying inside the engine's real size envelope. On top of that raw stress
skeleton the generator can layer *gameplay* content -- arenas, enemies, weapon
pickups, doors, lifts, and animated lights -- so a single map also exercises the
entity, mover, trigger, and animated-lighting paths under load.

The binding engine constraint
-----------------------------
prl-build rejects any map with more than 131072 geometry-bearing BSP leaves
(`MAX_CELL_ID_EXCLUSIVE` in `bvh_build.rs`): the runtime visible-cell bitmask
is a fixed 131072-bit structure. This -- not coordinates or buffer widths -- is
what caps room count. Every doorway and shaft fragments the empty space into
extra leaves, so the trade-off is direct: more connectivity per room => fewer
rooms fit under the 131072 cap. `--door-prob` / `--shaft-prob` expose that knob.
Watch the "geometry leaves" line prl-build prints and keep it under 131072.
Arenas also spend leaves (an arena is an open multi-storey volume with a
mezzanine ledge and stairs), so a map with `--arenas` needs a smaller grid.

Design (why it compiles and does not leak)
------------------------------------------
* A uniform axis-aligned 3D lattice of isolated room footprints separated by
  reserved corridor bands. Axis-aligned grids are the best case for the brush
  BSP splitter (clean axis-aligned cuts, almost no spanning brushes), so the
  tree stays shallow well under the MAX_RECURSION_DEPTH=256 guard in
  `partition/brush_bsp.rs`.
* Coordinates stay inside the classic Quake +/-16384-unit envelope. Input is
  parsed as f32 (`parse.rs`), whose integers are exact to 16.7M, so every
  vertex here is represented exactly.
* No static `light` entities *by default*. With zero baked lights the lightmap
  is a placeholder (no 8192^2 atlas cap, no multi-hour bake), so the per-frame
  geometry/BVH walk is the dominant cost -- exactly what we want to measure.
  Lights are opt-in (`--lights`); enabling them opts the map into the bake.
* The complex is fully sealed: solid edge walls and solid top/bottom slabs
  wrap the whole grid, so the exterior flood-fill (leaf culling) cannot reach
  the interior and delete geometry. Arenas keep their footprint fully interior
  (never on the grid edge) so every one of their four entryways opens into a
  neighbouring room rather than the void.

Reserved corridor layout
------------------------
Every ordinary lattice cell is one room. Adjacent room footprints have a 384 u
band of dedicated, sealed corridor airspace between them; the corridor floor
and low roof are emitted separately from room floors and walls. A chosen maze
edge opens matching doorways from both rooms into that band, never directly
from one room into another. Corridor-grid intersections provide corners, and
sparse diagonal guide fins add an angled treatment entirely within allocated
junction space. Arenas are the deliberate multi-cell exception and consume the
bands inside their own footprint. Vertical shafts punch holes through interior
room slabs to portal-connect the stacked layers; the shaft pattern shifts every
layer so shafts never stack into a vertical sightline.

All coordinates are emitted in Quake units (Z-up); prl-build applies the
1 unit = 0.0254 m scale and the Z-up -> Y-up swizzle.

Gameplay content (opt-in)
-------------------------
The stress skeleton doubles as a gameplay playground. All of the following are
off by default so the bare `stress-warren.map` stays a pure geometry/BVH probe;
`--preset warren` turns the whole set on at a conservative grid size for a
feature-heavy map.

* `--arenas N` carves N large open-area rooms. Each arena is a multi-cell block
  sized so its interior is never smaller than a regulation NFL football field
  (360 ft x 160 ft incl. end zones == 4320 x 1920 world units; at the stock
  PITCH_XY that is a 4x2-cell footprint -- see `arena_cell_span`), two storeys
  tall, with solid perimeter walls holding four ground-level entryways (one per
  side, into the neighbouring rooms), a walkable staircase from the floor up to
  a mezzanine ledge, and a wide central gap in that ledge the player can jump
  down through to the lower floor. Arena cells are reserved out of the maze
  lattice, so they replace rooms rather than adding to them.
* `--enemies N` pre-places N `reference_enemy` AI enemies (globally registered
  by the dev mod) across the rooms.
* `--weapons N` pre-places N wieldable weapon pickups (the dev mod's reference
  pistol/shotgun and the two fixture wieldables), touchable world items.
* `--doors N` upgrades N maze doorways to automatic sliding `kinematic_mover`
  doors. `--door-activation use` makes their `trigger_volume`s require the
  action button; `auto_close_ms` shuts them afterwards.
* `--monster-closets N` adds sealed closet pods. Their one-shot touch trigger
  starts the door and calls a generated data-script reaction that fires only
  that pod's `entity_spawner`.
* `--lifts N` replaces the jump-stairs in N shafts with a ping-pong lift
  platform that carries the player between the two layers.
* Animated lights. When lights are on, a fraction (`--animated-frac`) of the
  scattered baked lights animate: half are KVP-driven (an authored
  `brightness_curve`, baked entirely at compile time) and half are
  script-driven (tagged `warren_script_pulse` and pulsed by the companion data
  script `content/dev/scripts/stress-warren.ts` via `setLightAnimation`). Bake a
  map with animated lights at `--lightmap-density 0.25`: the warren's large
  faces collapse animated chunks into overlapping weight-map atlas rects at the
  coarser 0.5 and the packer aborts (0.25 clears it, under the atlas cap).
* Static room-lighting contract. When room lights are enabled, the normal
  contract is four baked spotlights plus one dim bake-only point light per room.
  `--preset warren` uses that contract with static lights only. Animated
  coverage lights are an explicit opt-in and remain promotable because a script
  must be able to address them. Dynamic lighting remains available through the
  explicit `--lights dynamic` / `mixed` stress modes; lift-car lights are always
  dynamic because they move with their carrier.

Usage
-----
    python3 tools/gen_stress_map.py            # committed default, fits the cap
    python3 tools/gen_stress_map.py --grid 8 8 4 --door-prob 0.2
    python3 tools/gen_stress_map.py --preset warren   # full gameplay showcase
    # crate stacks (shadow-casting occluders) + spot-heavy dynamic lights:
    python3 tools/gen_stress_map.py --grid 7 6 3 --lights dynamic \
        --crates 2 --spot-frac 0.5

Then compile with a COARSE SH probe spacing (the SH irradiance volume bakes a
probe grid over the whole world AABB regardless of lights; at the default 1.0 m
spacing a map this large would bake millions of probes and gigabytes):

    prl-build content/dev/maps/stress-warren.map \\
        -o content/dev/maps/stress-warren.prl --sh-probe-spacing 10.0 --no-cache

Push the room count up by enlarging the grid and/or lowering --door-prob, and
watch both compile cost and the 131072 BSP-leaf cap (see below).

Lightmap-array overflow preset
------------------------------
`--preset overflow` sets a known-good knob combination whose bake spills the
lightmap atlas into >=2 `texture_2d_array` layers at BOUNDED memory, so the
multi-layer atlas path can be exercised and crate shadows verified across
layers. How it works:

  prl-build charts EVERY world face into the lightmap atlas (chart size
  ~= face_size_m / density texels/side), then packs charts per BVH leaf. The
  per-layer atlas square is sized to fit the LARGEST single leaf; a leaf that
  no longer fits the current layer rolls onto a NEW array layer. So overflow is
  driven by TOTAL surface area, while the per-layer dimension (and thus the
  bake's peak memory -- one full-res float layer is ~1.9 GB at 8192^2) is driven
  by the largest single leaf.

  The cheap, memory-safe way to overflow is therefore: MANY small crate brushes
  + a MODEST grid at a MODERATE density. Lots of separate crates => lots of
  small per-leaf charts => the largest leaf stays small (small per-layer dim,
  low memory) while the combined chart area still spills past one layer. Do NOT
  crank density fine -- that inflates one huge layer and OOMs.

The preset sets: --grid 4 4 2, --crates 4, --lights static, --spot-frac 0.3,
--lights-per-room 3, --light-every 1, --door-prob 0.4, --shaft-prob 1.0.

Navigability. The map is meant to be WALKED to inspect lighting on the upper
layers, so the preset is tuned to be traversable, not just dense:
  * STAIRS. Every shaft gets a spiral of jumpable platforms threaded up through
    the ceiling gap (see emit_shaft_stairs). A bare shaft is only a hole, and
    with ~12 u of auto-step a 384 u storey cannot be climbed; the spiral both
    makes the gap reachable on foot and visibly MARKS it so it is not missed.
  * SCATTERED LIGHTS. Each room gets 3 lights jittered across the room (not one
    central fixture), so coverage is even and you are never wandering a dim
    SH-only space looking for the way up.
  * MORE DOORS. --door-prob 0.4 braids the maze with loop doors so a shaft is
    not buried at the end of a long dead-end branch.

The light mix is a POINT+SPOT BLEND (coverage + crate shadows): ~70% point
(`light`) and ~30% spot (`light_spot`). WHY the blend -- point lights are
omnidirectional, so they fully light each room AND directly light crate faces on
EVERY atlas layer, the clean confirmation that the multi-layer atlas renders
direct light correctly. Spotlights are narrow cones that leave faces angled away
from the cone dark, so a pure-spot map reads as broken; we keep ~30% spots only
because static spots (`_shadow_type static_light_map`) are what bake crate
SHADOWS into the lightmap, which we still want to verify. Crate count -- not
light type -- drives the atlas overflow. Every preset value is overridable.

  Recommended bake (writes the .prl to /tmp; do NOT commit it):

    python3 tools/gen_stress_map.py --preset overflow \\
        -o content/dev/maps/stress-warren-overflow.map
    RUST_LOG=info ./target/release/prl-build --no-cache \\
        --lightmap-density 0.06 --sh-probe-spacing 10.0 \\
        content/dev/maps/stress-warren-overflow.map -o /tmp/stress-overflow.prl

  Look for `[PRL] Lightmap: WxH atlas, N layer(s)` with N >= 2. With only 4
  crates/room the chart area is lower than the old 10-crate preset, so the bake
  density is dropped to 0.06 to keep the overflow; if N == 1, lower it another
  notch (0.05) or raise --crates. If the bake OOMs or reports ChartTooLarge,
  RAISE the density (coarser) so the per-layer dim drops.
"""

import argparse
import math
import os
import random
import sys

# --- Lattice geometry (Quake units) ---------------------------------------
PITCH_XY = 1280   # isolated room outer width/depth; interior = 1024 x 1024
WALL_T = 256      # wall thickness -> horizontal room interior 1024 x 1024 (>= 1024)
# A lattice step deliberately includes a dedicated band of corridor airspace.
# Rooms never occupy this band: a doorway now joins a room wall to a corridor,
# rather than cutting a shared wall between two room interiors.
CORRIDOR_BAND = 384
LATTICE_PITCH_XY = PITCH_XY + CORRIDOR_BAND
STORY_H = 256     # one navigable storey (6.50 m at the map's inch scale)
ROOM_STORIES = 2  # lattice rooms are deliberately at least two storeys tall
SLAB_T = 128      # floor/ceiling slab thickness
PITCH_Z = STORY_H * ROOM_STORIES + SLAB_T  # room interior = 512 u / 13.0 m

DOOR_W = 256      # doorway opening width
DOOR_H = STORY_H  # doorway/hall clearance; rooms remain open above it
# Half-depth of a door's touch trigger along the passage normal. The shut leaf
# occupies +/-WALL_T/2 (128 u) of the wall, so the trigger must reach well past
# that into BOTH rooms or only one approach touches it. 384 leaves ~256 u of
# touch zone inside each room -- comfortably less than the 1024 u interior, so it
# never crosses to the far wall -- and the player opens the door from either side.
DOOR_TRIGGER_REACH = 384
SHAFT = 384       # vertical shaft opening (square hole in interior slabs)

# Every room is a tall, two-storey volume. The reserved corridor grid occupies
# only the lower storey, with its own floor and low roof. Its intersections are
# where routes turn; small diagonal guide fins give selected junctions an angled
# treatment without ever entering a room footprint.
CORRIDOR_CEIL_T = 32
CORRIDOR_GUIDE_T = 40
CORRIDOR_GUIDE_LEN = 176
HALLWAY_SPOT_SPACING = 768   # ~19.5 m: at least one baked spot every 20 m
HALLWAY_SPOT_FALLOFF = 1024
HALLWAY_SPOT_INTENSITY = 180

# Textures from the bundled "50-free-textures" collection. Each diffuse has a
# `_n` (normal) and `_s` (specular) sibling, which prl-build auto-resolves into
# the per-texture .prm bundle (build_pipeline.md §Texture name resolution), so
# these maps also stress the normal-map + specular material pipeline. Using
# several per surface class spreads geometry across more material buckets => more
# indirect draw calls per frame, another axis of realistic stress.
_C = "50-free-textures/"
WALL_TEX = [_C + n for n in (
    "concrete_stone_021", "concrete_stone_023", "concrete_stone_025",
    "concrete_stone_027", "concrete_stone_029")]
FLOOR_TEX = [_C + n for n in (
    "concrete_pavement_036", "concrete_pavement_038", "concrete_pavement_040",
    "concrete_pavement_042")]
CEIL_TEX = [_C + n for n in (
    "concrete_stone_031", "concrete_stone_033", "concrete_stone_035")]
CRATE_TEX = [_C + n for n in ("wood_bark_046", "wood_bark_047", "wood_bark_048")]
# Movers (doors/lifts) reuse the concrete-stone pool so they always resolve to a
# real .prm; a distinct index keeps them visually separable from the walls.
MOVER_TEX = _C + "concrete_stone_022"


def pick(pool, key):
    """Deterministically pick a texture from a pool by an integer key."""
    return pool[key % len(pool)]

# Crate stacks: small solid box-brushes piled on the room floor. They are world
# geometry, so they (a) add to the per-frame geometry/BVH walk, (b) cast real
# dynamic shadows under both dynamic light types (the spot-shadow depth pass and
# the point cube-shadow face passes both rasterize cone-culled world geometry --
# see rendering_pipeline.md §4, §7.1), and (c) carve the room's empty leaf into
# several BSP leaves, so they spend the BSP-leaf budget and lower room count.
CRATE_EDGE = 112      # crate cube edge (Quake units, ~2.8 m)
CRATE_MARGIN = 192    # keep stacks this far from interior walls (clear of doors)

# Cyberpunk-ish palette (0-255 RGB) so lights vary in color.
LIGHT_COLORS = [
    (0, 255, 200), (255, 0, 200), (255, 160, 40), (40, 160, 255),
    (180, 0, 255), (0, 255, 120), (255, 60, 60), (120, 220, 255),
]

# Tag the script-driven animated lights carry; the companion data script
# (content/dev/scripts/stress-warren.ts) queries this tag and drives a pulse.
SCRIPT_LIGHT_TAG = "warren_script_pulse"
SCRIPT_DATA_SCRIPT = "content/dev/scripts/stress-warren.ts"

# Hard cap on the number of ANIMATED baked lights per map. Every animated baked
# light forces the animated-light weight-map bake, whose per-face chunk packer
# (`animated_light_weight_maps.rs::assert_no_overlapping_rects_per_face`) aborts
# when the warren's very large faces subdivide into chunks that collapse into the
# same 1-texel atlas rect -- a compiler-side packer limitation (the assertion
# itself says "fix the packer, not this baker"). The collision risk scales with
# the total animated-chart area, so bounding the animated set to a small number
# keeps the bake inside the range that compiles. `--animated-frac` still chooses
# *which* baked lights animate, but never more than this many total.
ANIMATED_LIGHT_CAP = 6

# Wieldable pickups the dev mod registers globally (start-script.ts). Placing
# these classnames directly spawns touchable weapon world-items at level load.
WEAPON_CLASSES = [
    "reference_pistol", "reference_shotgun",
    "wieldable_fixture_auto", "wieldable_fixture_press",
]
# AI enemy archetype the dev mod registers globally; directly map-placeable
# because it carries health + mesh components (sdk/behaviors/reference/entities).
ENEMY_CLASS = "reference_enemy"


def box_brush(x0, y0, z0, x1, y1, z1, tex_side, tex_top, tex_bottom):
    """An axis-aligned solid box as a 6-plane Standard-format brush.

    Winding/point order mirrors a known-good box from occlusion-test.map so the
    plane normals face outward (interior behind each plane).
    """
    s = lambda t: f"{t} 0 0 0 1 1"
    return (
        "{\n"
        f"( {x0} {y1} {z0} ) ( {x0} {y0} {z1} ) ( {x0} {y0} {z0} ) {s(tex_side)}\n"   # -X
        f"( {x0} {y0} {z0} ) ( {x1} {y0} {z1} ) ( {x1} {y0} {z0} ) {s(tex_side)}\n"   # -Y
        f"( {x0} {y0} {z0} ) ( {x1} {y1} {z0} ) ( {x0} {y1} {z0} ) {s(tex_bottom)}\n" # -Z
        f"( {x0} {y1} {z1} ) ( {x1} {y0} {z1} ) ( {x0} {y0} {z1} ) {s(tex_top)}\n"     # +Z
        f"( {x1} {y1} {z0} ) ( {x0} {y1} {z1} ) ( {x0} {y1} {z0} ) {s(tex_side)}\n"   # +Y
        f"( {x1} {y0} {z0} ) ( {x1} {y1} {z1} ) ( {x1} {y1} {z0} ) {s(tex_side)}\n"   # +X
        "}\n"
    )


def oriented_box_brush(cx, cy, dx, dy, length, width, z0, z1,
                       tex_side, tex_top, tex_bottom):
    """An XY-rotated rectangular prism using Standard-map brush planes.

    ``(dx, dy)`` is the longitudinal direction.  Keeping the prism convex lets
    corridor legs be diagonal without changing the BSP-facing source format.
    """
    norm = math.hypot(dx, dy)
    if norm == 0:
        raise ValueError("oriented box direction must be non-zero")
    ux, uy = dx / norm, dy / norm
    vx, vy = -uy, ux
    half_l, half_w = length / 2, width / 2

    def point(s, t, z):
        return (cx + ux * s + vx * t, cy + uy * s + vy * t, z)

    def fmt(point):
        return "( {:.3f} {:.3f} {:.3f} )".format(*point)

    p = point
    s = lambda t: f"{t} 0 0 0 1 1"
    return (
        "{\n"
        f"{fmt(p(-half_l, half_w, z0))} {fmt(p(-half_l, -half_w, z1))} {fmt(p(-half_l, -half_w, z0))} {s(tex_side)}\n"
        f"{fmt(p(-half_l, -half_w, z0))} {fmt(p(half_l, -half_w, z1))} {fmt(p(half_l, -half_w, z0))} {s(tex_side)}\n"
        f"{fmt(p(-half_l, -half_w, z0))} {fmt(p(half_l, half_w, z0))} {fmt(p(-half_l, half_w, z0))} {s(tex_bottom)}\n"
        f"{fmt(p(-half_l, half_w, z1))} {fmt(p(half_l, -half_w, z1))} {fmt(p(-half_l, -half_w, z1))} {s(tex_top)}\n"
        f"{fmt(p(half_l, half_w, z0))} {fmt(p(-half_l, half_w, z1))} {fmt(p(-half_l, half_w, z0))} {s(tex_side)}\n"
        f"{fmt(p(half_l, -half_w, z0))} {fmt(p(half_l, half_w, z1))} {fmt(p(half_l, half_w, z0))} {s(tex_side)}\n"
        "}\n"
    )


def room_bounds(starts, index):
    """Return the solid outer span of one room on a lattice axis."""
    return starts[index], starts[index] + PITCH_XY


def emit_reserved_corridor_grid(brushes, x0s, y0s, zf, tex, junction_seed,
                                arena_at=None, hallway_lights=None):
    """Emit the one-storey corridor network that occupies reserved XY bands.

    The vertical bands span a whole map row; horizontal bands fill only the
    room columns, so their floor/roof brushes meet at faces rather than overlap
    at every junction.  Room walls bound these bands on both sides.  Therefore
    every door opens from a room into allocated corridor airspace, never into a
    neighbouring room or a wall that the corridor also occupies.
    """
    nx, ny = len(x0s), len(y0s)
    arena_at = arena_at or (lambda _i, _j: False)
    z0, z1 = zf - SLAB_T // 2, zf + SLAB_T // 2
    roof0 = zf + STORY_H
    # The playable corridor remains one-storey high (its visible underside is
    # roof0), but the solid roof continues through the unused upper-storey band
    # and overlaps the next room slab. That makes each reserved band a true
    # solid separator above the hallway rather than a thin ceiling merely
    # touching tall-room walls at T-junctions.
    roof1 = zf + PITCH_Z
    def floor_and_roof(ax0, ay0, ax1, ay1):
        brushes.append(box_brush(ax0, ay0, z0, ax1, ay1, z1, tex, tex, tex))
        brushes.append(box_brush(ax0, ay0, roof0, ax1, ay1, roof1, tex, tex, tex))
        if hallway_lights is None:
            return
        length = max(ax1 - ax0, ay1 - ay0)
        count = max(1, math.ceil(length / HALLWAY_SPOT_SPACING))
        for n in range(count):
            fraction = (n + 0.5) / count
            px = round(ax0 + (ax1 - ax0) * fraction)
            py = round(ay0 + (ay1 - ay0) * fraction)
            hallway_lights.append(light_entity(
                "static", (px, py, roof0 - 24), (255, 244, 214),
                HALLWAY_SPOT_FALLOFF, HALLWAY_SPOT_INTENSITY, True, None,
                bake_only=False))

    # Split the north/south bands at every room/gap boundary. An arena consumes
    # a band only when it occupies both cells on the two sides; a band bordering
    # an arena is still a real corridor serving that arena's perimeter doorway.
    for i in range(nx - 1):
        for j in range(ny):
            if not (arena_at(i, j) and arena_at(i + 1, j)):
                floor_and_roof(x0s[i] + PITCH_XY, y0s[j],
                               x0s[i + 1], y0s[j] + PITCH_XY)
        for j in range(ny - 1):
            if not all(arena_at(ii, jj)
                       for ii in (i, i + 1) for jj in (j, j + 1)):
                floor_and_roof(x0s[i] + PITCH_XY, y0s[j] + PITCH_XY,
                               x0s[i + 1], y0s[j + 1])
    # East/west bands only fill room columns, avoiding coplanar overlap with
    # north/south bands while retaining connected cross-junction airspace.
    for j in range(ny - 1):
        for i in range(nx):
            if not (arena_at(i, j) and arena_at(i, j + 1)):
                floor_and_roof(x0s[i], y0s[j] + PITCH_XY,
                               x0s[i] + PITCH_XY, y0s[j + 1])

    # A sparse 45-degree guide fin makes selected cross-junctions visibly
    # angled. It stays entirely inside the reserved intersection and leaves
    # generous passage on both sides, so it cannot cut into a room or seal the
    # corridor network.
    for j in range(ny - 1):
        for i in range(nx - 1):
            if ((i * 11 + j * 7 + junction_seed) % 3 != 0
                    or all(arena_at(ii, jj)
                           for ii in (i, i + 1) for jj in (j, j + 1))):
                continue
            cx = (x0s[i] + PITCH_XY + x0s[i + 1]) / 2
            cy = (y0s[j] + PITCH_XY + y0s[j + 1]) / 2
            brushes.append(oriented_box_brush(
                cx, cy, 1, 1 if (i + j + junction_seed) % 2 else -1,
                CORRIDOR_GUIDE_LEN, CORRIDOR_GUIDE_T, zf, roof0,
                tex, tex, tex))


def wall_box(brushes, x0, y0, x1, y1, zf, zc, tex):
    """Solid wall slab spanning [x0,x1]x[y0,y1] over interior height [zf,zc]."""
    if x1 - x0 < 1 or y1 - y0 < 1:
        return
    brushes.append(box_brush(x0, y0, zf, x1, y1, zc, tex, tex, tex))


DOOR_SLOT_MARGIN = 96   # solid jamb kept at the wall ends when a door hugs a slot


def slot_center(lo, hi, slot):
    """Doorway center for one of two disjoint end-slots of a wall segment.

    slot 0 hugs the `lo` end, slot 1 hugs the `hi` end. The two slots never
    overlap (for our 1280-unit walls the openings are ~580 units apart), so if
    a room's two opposite-wall doorways use different slots there is no straight
    line of sight through that room. Maze edges alternate slots (see generate),
    which keeps any straight sightline to at most the two rooms a single doorway
    joins -- never a third.
    """
    off = DOOR_W // 2 + DOOR_SLOT_MARGIN
    if hi - lo <= 2 * off:                 # too short to offset: fall back to center
        return (lo + hi) // 2
    return lo + off if slot == 0 else hi - off


def uf_find(parent, a):
    """Union-find root with path compression (used to build the per-layer maze)."""
    while parent[a] != a:
        parent[a] = parent[parent[a]]
        a = parent[a]
    return a


def emit_wall(brushes, axis, line, lo, hi, zf, zc, dcenter, tex):
    """Emit a wall on an interior/edge boundary.

    `dcenter` is None for a solid wall, or the coordinate along [lo,hi] where a
    doorway opening is cut. axis 'x': wall on plane X=line, spanning Y in
    [lo,hi]. axis 'y': wall on plane Y=line, spanning X in [lo,hi]. The wall is
    WALL_T thick, centered on `line`. Interior height [zf,zc].
    """
    h = WALL_T // 2
    if dcenter is None:
        if axis == "x":
            wall_box(brushes, line - h, lo, line + h, hi, zf, zc, tex)
        else:
            wall_box(brushes, lo, line - h, hi, line + h, zf, zc, tex)
        return

    # Full-thickness doorway at `dcenter`: split into two jambs + a lintel.
    d0, d1 = dcenter - DOOR_W // 2, dcenter + DOOR_W // 2
    ztop = zf + DOOR_H
    if axis == "x":
        wall_box(brushes, line - h, lo, line + h, d0, zf, zc, tex)       # jamb low
        wall_box(brushes, line - h, d1, line + h, hi, zf, zc, tex)       # jamb high
        wall_box(brushes, line - h, d0, line + h, d1, ztop, zc, tex)     # lintel
    else:
        wall_box(brushes, lo, line - h, d0, line + h, zf, zc, tex)
        wall_box(brushes, d1, line - h, hi, line + h, zf, zc, tex)
        wall_box(brushes, d0, line - h, d1, line + h, ztop, zc, tex)


def emit_slab(brushes, x0, y0, x1, y1, zc, holed, ftex, ctex):
    """Horizontal slab centered on Z=zc over footprint [x0,x1]x[y0,y1].

    When `holed`, a centered square shaft is carved (slab split into 4 rims)
    to portal-connect the room below to the room above.
    """
    h = SLAB_T // 2
    z0, z1 = zc - h, zc + h
    if not holed:
        brushes.append(box_brush(x0, y0, z0, x1, y1, z1, ftex, ftex, ctex))
        return
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    a0, a1 = cx - SHAFT // 2, cx + SHAFT // 2
    b0, b1 = cy - SHAFT // 2, cy + SHAFT // 2
    # four rims around the hole
    brushes.append(box_brush(x0, y0, z0, x1, b0, z1, ftex, ftex, ctex))
    brushes.append(box_brush(x0, b1, z0, x1, y1, z1, ftex, ftex, ctex))
    brushes.append(box_brush(x0, b0, z0, a0, b1, z1, ftex, ftex, ctex))
    brushes.append(box_brush(a1, b0, z0, x1, b1, z1, ftex, ftex, ctex))


def tile_layer(nx, ny, rng, blocked):
    """Assign one isolated room footprint to every unblocked lattice cell.

    The old 1x1..2x2 greedy tiling merged cells. That is incompatible with
    reserved corridor bands: a merged room would either consume a band or leave
    a corridor passing through its interior. Ordinary rooms are consequently
    uniform here; arenas remain the intentional multi-cell room type.
    """
    room = {}
    rid = 0
    for j in range(ny):
        for i in range(nx):
            if (i, j) in blocked:
                continue
            room[(i, j)] = rid
            rid += 1
    return room


def emit_crate_stack(brushes, x0i, y0i, x1i, y1i, zf, zc, tex, rng):
    """Pile crate cubes on the floor inside the room interior rect.

    The base is placed clear of the walls by CRATE_MARGIN; upper crates jitter
    slightly for a messy-pile silhouette (better shadow shapes). Stack height is
    capped so the top crate stays under the ceiling `zc` -- otherwise a tall
    stack pokes through the ceiling slab and can engulf the ceiling light (which
    then bakes "inside a solid leaf"). Boxes are solid world brushes; minor
    overlaps between stacked crates are harmless (the BSP unions solids).
    """
    e = CRATE_EDGE
    # interior rect the base may occupy (so the whole crate stays off the walls)
    bx0, bx1 = x0i + CRATE_MARGIN, x1i - CRATE_MARGIN - e
    by0, by1 = y0i + CRATE_MARGIN, y1i - CRATE_MARGIN - e
    if bx1 <= bx0 or by1 <= by0:
        return
    px = rng.randint(bx0, bx1)
    py = rng.randint(by0, by1)
    max_h = max(1, (zc - zf - 32) // e)             # fit under the ceiling
    height = rng.randint(1, min(3, max_h))
    for n in range(height):
        jx = rng.randint(-e // 4, e // 4) if n else 0
        jy = rng.randint(-e // 4, e // 4) if n else 0
        cx0 = max(x0i + 8, min(px + jx, x1i - e - 8))
        cy0 = max(y0i + 8, min(py + jy, y1i - e - 8))
        cz0 = zf + n * e
        brushes.append(box_brush(cx0, cy0, cz0, cx0 + e, cy0 + e, cz0 + e,
                                 tex, tex, tex))
    return (px, py)                                 # base xy, for light avoidance


# --- Vertical traversal: jump-stair spirals through shafts -----------------
# A shaft is only a HOLE in the slab. With the player's ~12-unit auto-step
# (step_height 0.3 m) against a 384-unit floor-to-floor rise, a bare hole cannot
# be climbed -- you can fall DOWN one but never up. There is no ladder entity in
# the engine, so vertical traversal has to be geometric: we thread a compact
# spiral of *jumpable* platforms up through each shaft (or, with `--lifts`, a
# ping-pong lift platform -- see emit_lift). Each step rises STAIR_RISE < the
# player's jump apex, so it is reachable in one hop, and the rising spiral is a
# visible structure that MARKS the otherwise easy-to-miss ceiling gap. The
# shaft's centre column is kept clear so the air portal that connects the two
# layers (and stops the flood-fill from sealing the upper layer) stays open.
STAIR_RISE = 48      # vertical gain per step (Quake u). Jump apex ~= 60 u
# (jump_velocity 5.5 m/s vs g 9.81 m/s^2), so 48 u clears
                     # with margin while still climbing a 640 u layer in 14 hops.
STAIR_R = 96         # spiral ring radius (platform centre from shaft centre)
STAIR_HALF = 56      # platform half-extent -> 112 u square (player dia ~= 31 u)
STAIR_THICK = 24     # platform slab thickness


def emit_shaft_stairs(brushes, cx, cy, zf_low, climb, tex):
    """Spiral of jumpable platforms climbing `climb` units from floor `zf_low`,
    threaded up through a shaft hole centred at (cx, cy). Cycles the four sides
    of the hole (+X, +Y, -X, -Y) so the player corkscrews up; a final landing
    bridges the top step out to the solid upper-floor rim. Returns step count.
    """
    n = max(1, climb // STAIR_RISE)                 # 384 / 48 = 8 steps / storey
    ring = [(STAIR_R, 0), (0, STAIR_R), (-STAIR_R, 0), (0, -STAIR_R)]
    last_side = 0
    for s in range(n):
        last_side = s % 4
        dx, dy = ring[last_side]
        px, py = cx + dx, cy + dy
        ztop = zf_low + (s + 1) * STAIR_RISE
        brushes.append(box_brush(px - STAIR_HALF, py - STAIR_HALF, ztop - STAIR_THICK,
                                 px + STAIR_HALF, py + STAIR_HALF, ztop, tex, tex, tex))
    # Landing at the top, flush with the upper floor, bridging the last step out
    # past the hole rim (half-width SHAFT//2 = 192) so the player can step off.
    ztop = zf_low + climb
    inner = STAIR_R - STAIR_HALF                     # last step's inner edge
    reach = SHAFT // 2 + 28                          # just past the rim
    dx, dy = ring[last_side]
    if dx:                                           # +/-X side: extend along X
        x_lo, x_hi = (cx + inner, cx + reach) if dx > 0 else (cx - reach, cx - inner)
        brushes.append(box_brush(x_lo, cy - 80, ztop - STAIR_THICK,
                                 x_hi, cy + 80, ztop, tex, tex, tex))
    else:                                            # +/-Y side: extend along Y
        y_lo, y_hi = (cy + inner, cy + reach) if dy > 0 else (cy - reach, cy - inner)
        brushes.append(box_brush(cx - 80, y_lo, ztop - STAIR_THICK,
                                 cx + 80, y_hi, ztop, tex, tex, tex))
    return n


# --- Walkable staircases (arenas) ------------------------------------------
# A run of solid stepped boxes climbing `climb` units. Rise per step is <= the
# player's ~12 u auto-step so the run is WALKED (no jumping), which is what an
# arena's "stairway to move between levels" needs. Each step is a full-height
# block from the floor to its own top, so the run reads as a real staircase.
STAIR_WALK_RISE = 12   # <= step_height (0.3 m ~= 11.8 u): walkable without a jump
STAIR_WALK_DEPTH = 28  # tread depth per step


def emit_walk_stairs(brushes, base_x, base_y, axis, sign, cross_half, zf, climb, tex):
    """Straight walkable staircase from floor `zf` climbing `climb`.

    Ascends along `axis` ('x'/'y') in direction `sign` (+1/-1) starting at
    (base_x, base_y); each tread is `2*cross_half` wide across the run. Returns
    (end_x, end_y) of the top tread's far edge so the caller can confirm the run
    lands on the mezzanine ledge.
    """
    n = max(1, int(round(climb / STAIR_WALK_RISE)))
    for s in range(n):
        z_top = zf + (s + 1) * STAIR_WALK_RISE
        if axis == "x":
            a0 = base_x + sign * s * STAIR_WALK_DEPTH
            a1 = a0 + sign * STAIR_WALK_DEPTH
            x0, x1 = (a0, a1) if sign > 0 else (a1, a0)
            brushes.append(box_brush(x0, base_y - cross_half, zf,
                                     x1, base_y + cross_half, z_top, tex, tex, tex))
        else:
            a0 = base_y + sign * s * STAIR_WALK_DEPTH
            a1 = a0 + sign * STAIR_WALK_DEPTH
            y0, y1 = (a0, a1) if sign > 0 else (a1, a0)
            brushes.append(box_brush(base_x - cross_half, y0, zf,
                                     base_x + cross_half, y1, z_top, tex, tex, tex))
    run = n * STAIR_WALK_DEPTH
    if axis == "x":
        return base_x + sign * run, base_y
    return base_x, base_y + sign * run


# --- Arenas ----------------------------------------------------------------
ARENA_MEZZ_WALK = 384   # width of the mezzanine walkway ring around the gap
ARENA_STAIR_HALF = 128  # half-width of the arena staircase run

# Arena minimum footprint: a regulation NFL football field. The engine's world
# unit is one inch (1 map unit = 0.0254 m, exact; build_pipeline.md "Unit
# scale"), and this generator emits map/Quake units, so 1 emitted unit == 1 inch.
# An NFL field including both end zones is 360 ft x 160 ft (120 yd x 53.33 yd)
# == 4320 in x 1920 in. An arena must be no smaller than that; we measure the
# floor on the arena's INTERIOR (playable) footprint -- the walkable area inside
# the perimeter walls -- so the player really does get a field-sized space.
NFL_FIELD_LONG_U = 360 * 12    # 4320 world units (inches): length incl. end zones
NFL_FIELD_SHORT_U = 160 * 12   # 1920 world units (inches): width


def arena_cell_span():
    """(aw, ah) cells an arena must span so its INTERIOR clears an NFL field.

    An arena consumes the corridor bands inside its own footprint, so its outer
    extent is `span * LATTICE_PITCH_XY - CORRIDOR_BAND`. Subtracting WALL_T for
    the perimeter walls gives its playable interior. Solving that expression
    against the field dimensions gives the minimum span. At the stock sizes this
    is still 4 x 2 cells, comfortably above 4320 x 1920 u. The floor of 2 keeps
    the two-storey mezzanine/staircase geometry well-formed.
    """
    def span_for(length):
        return math.ceil((length + WALL_T + CORRIDOR_BAND) / LATTICE_PITCH_XY)

    aw = max(2, span_for(NFL_FIELD_LONG_U))
    ah = max(2, span_for(NFL_FIELD_SHORT_U))
    return aw, ah


def plan_arenas(nx, ny, nz, n_arenas, spawn_cell):
    """Reserve up to `n_arenas` non-overlapping arena footprints.

    Each footprint is `aw x ah` cells (see `arena_cell_span`) sized so the
    arena interior is never smaller than a regulation NFL football field. Every
    arena stays fully interior (its footprint and a one-cell frame around it are
    inside the grid) so all four side walls face a neighbouring room -- never
    the exterior -- and never covers the player spawn cell. A lattice cell is
    already two storeys tall, so arenas occupy one such cell layer. Returns a
    list of arena dicts. Grids too small to seat one
    field-sized arena (plus its frame) yield an empty list -- the caller warns.
    """
    if n_arenas <= 0:
        return []
    aw, ah = arena_cell_span()
    alayers = 1
    arenas = []
    reserved = set()
    # Candidate lower-left corners keeping the aw x ah footprint one cell off
    # every grid edge (interior neighbours on all four sides).
    cands = [(i0, j0)
             for j0 in range(1, ny - ah)
             for i0 in range(1, nx - aw)]
    rng = random.Random(0x5710 + nx * 131 + ny)
    rng.shuffle(cands)
    for (i0, j0) in cands:
        if len(arenas) >= n_arenas:
            break
        i1, j1 = i0 + aw, j0 + ah
        cells = {(i, j) for j in range(j0, j1) for i in range(i0, i1)}
        # Keep arenas apart (a one-cell gap) and clear of the spawn column.
        halo = {(i, j) for j in range(j0 - 1, j1 + 1) for i in range(i0 - 1, i1 + 1)}
        if halo & reserved:
            continue
        if (spawn_cell[0], spawn_cell[1]) in cells:
            continue
        reserved |= cells
        arenas.append(dict(i0=i0, j0=j0, i1=i1, j1=j1, k0=0, k1=alayers))
    return arenas


def emit_arena(brushes, X, X_END, Y, Y_END, Z, arena, rng):
    """Emit one arena's shell + mezzanine + staircase and return placement info.

    Geometry: four full-height perimeter walls each holding one ground-level
    entryway, a solid floor and ceiling, one mezzanine ledge (footprint minus a
    wide central gap) at the interior layer boundary, and a walkable staircase
    from the floor up to that ledge. Interior vertical walls and the interior
    slab are omitted, so the whole block is one open two-storey volume.

    Returns a dict describing the arena's lightable/placeable space (interior
    rect, floor and ceiling Z, mezzanine top Z) for the content phase.
    """
    i0, j0, i1, j1 = arena["i0"], arena["j0"], arena["i1"], arena["j1"]
    k0, k1 = arena["k0"], arena["k1"]
    # An arena is the intentional exception to ordinary one-cell rooms: it
    # consumes the bands inside its reserved multi-cell footprint, leaving its
    # outer perimeter walls facing the surrounding corridor grid.
    x0, x1 = X[i0], X_END[i1 - 1]
    y0, y1 = Y[j0], Y_END[j1 - 1]
    zf = Z[k0] + SLAB_T // 2             # interior floor top
    zc = Z[k1] - SLAB_T // 2             # interior ceiling bottom
    wtex = pick(WALL_TEX, i0 + j0)
    ftex = pick(FLOOR_TEX, i0 + j0)
    ctex = pick(CEIL_TEX, i0 + j0)

    # Perimeter walls, each with a single ground-level entryway at its midpoint.
    emit_wall(brushes, "x", x0, y0, y1, zf, zc, (y0 + y1) // 2, wtex)   # -X side
    emit_wall(brushes, "x", x1, y0, y1, zf, zc, (y0 + y1) // 2, wtex)   # +X side
    emit_wall(brushes, "y", y0, x0, x1, zf, zc, (x0 + x1) // 2, wtex)   # -Y side
    emit_wall(brushes, "y", y1, x0, x1, zf, zc, (x0 + x1) // 2, wtex)   # +Y side

    # Solid floor and ceiling over the whole footprint (the shell's seal).
    zfl = Z[k0]
    brushes.append(box_brush(x0, y0, zfl - SLAB_T // 2, x1, y1, zfl + SLAB_T // 2,
                             ftex, ftex, ctex))
    zcl = Z[k1]
    brushes.append(box_brush(x0, y0, zcl - SLAB_T // 2, x1, y1, zcl + SLAB_T // 2,
                             ftex, ftex, ctex))

    # Mezzanine at the first-storey height: footprint minus a wide centred gap,
    # so an upper-level player can jump down through it.  A lattice cell now
    # contains both storeys, rather than borrowing a neighbouring cell layer.
    zm = zf + STORY_H
    mz0, mz1 = zm - SLAB_T // 2, zm + SLAB_T // 2
    g0x, g1x = x0 + ARENA_MEZZ_WALK, x1 - ARENA_MEZZ_WALK
    g0y, g1y = y0 + ARENA_MEZZ_WALK, y1 - ARENA_MEZZ_WALK
    brushes.append(box_brush(x0, y0, mz0, x1, g0y, mz1, ftex, ftex, ctex))       # -Y rim
    brushes.append(box_brush(x0, g1y, mz0, x1, y1, mz1, ftex, ftex, ctex))       # +Y rim
    brushes.append(box_brush(x0, g0y, mz0, g0x, g1y, mz1, ftex, ftex, ctex))     # -X rim
    brushes.append(box_brush(g1x, g0y, mz0, x1, g1y, mz1, ftex, ftex, ctex))     # +X rim

    # Walkable staircase from the floor up to the mezzanine top, hugging the -Y
    # rim (which is solid, so the run lands on walkable ledge). Climb = one
    # storey; the run ascends in +X from just inside the -X wall.
    mtop = mz1                                       # mezzanine walking surface
    climb = mtop - zf
    base_x = x0 + WALL_T // 2 + ARENA_STAIR_HALF
    base_y = y0 + ARENA_MEZZ_WALK // 2 + SLAB_T // 2
    emit_walk_stairs(brushes, base_x, base_y, "x", +1, ARENA_STAIR_HALF,
                     zf, climb, wtex)

    return dict(
        x0i=x0 + WALL_T // 2, x1i=x1 - WALL_T // 2,
        y0i=y0 + WALL_T // 2, y1i=y1 - WALL_T // 2,
        floor_z=zf, ceil_z=zc, mezz_z=mtop,
        gap=(g0x, g0y, g1x, g1y),
    )


# --- Scattered ceiling lights ----------------------------------------------
LIGHT_MARGIN = 192          # keep scattered lights this far off the interior walls
LIGHT_CRATE_CLEARANCE = 200 # keep a light at least this far (manhattan) from a crate

# Per-room cap on *promotable* baked lights -- lights kept as runtime entities
# (`_bake_only 0`) rather than folded into the lightmap (`_bake_only 1`). The
# normal room contract has none; only explicit animated coverage lights are
# promotable, so the small cap prevents that opt-in from turning into a runtime
# lighting load.
MAX_PROMOTABLE_PER_ROOM = 3

# Every normally lit room has four bright baked spots and one intentionally dim
# bake-only point fill. The point prevents completely black corners without
# becoming a fifth dominant fixture.
DIM_POINT_INTENSITY = 80
SPOT_INTENSITY = 220


def scatter_light_xy(lx0, lx1, ly0, ly1, crate_bases, rng):
    """Pick a ceiling-light xy inside the inset rect, biased away from crate
    stacks so a spotlight does not bake itself 'inside' a crate. Falls back to
    the rect centre when it is too small to scatter."""
    if lx1 <= lx0 or ly1 <= ly0:
        return (lx0 + lx1) // 2, (ly0 + ly1) // 2
    best = None
    for _ in range(8):
        px = rng.randint(lx0, lx1)
        py = rng.randint(ly0, ly1)
        clear = min((abs(px - bx) + abs(py - by) for bx, by in crate_bases),
                    default=10 ** 9)
        if best is None or clear > best[0]:
            best = (clear, px, py)
        if clear >= LIGHT_CRATE_CLEARANCE:
            break
    return best[1], best[2]


def light_entity(mode, origin, color, falloff, intensity, spot, rng,
                 animate=None, bake_only=False, carrier=None):
    """Return a light entity block (list of "key value" lines + classname).

    mode: 'dynamic' -> light_dynamic / light_dynamic_spot (runtime, unbaked:
          stresses the per-frame forward light loop + shadow pools, no bake).
    mode: 'static'  -> light (baked: stresses the lightmap + SH bake).

    `animate` (static lights only) selects an animation:
      'kvp'    -> an authored `brightness_curve` pulse, baked at compile time.
      'script' -> tagged `warren_script_pulse`; the data script drives the pulse
                  at runtime via setLightAnimation (which reserves the animated
                  bake automatically).
    `bake_only` marks a `_bake_only 1` fixture (bakes, no runtime entity).
    `carrier` attaches a dynamic light to a named kinematic mover.
    """
    cr, cg, cb = color
    if carrier and mode != "dynamic":
        raise ValueError("only dynamic lights can be carried by movers")
    if mode == "static":
        cls = "light"
        # `_light_size` is the bake-only emitter radius (metres) that drives the
        # soft-shadow penumbra. The default 0.25 m is sub-texel at our coarse
        # lightmap density, so shadows bake hard; 0.75 m gives a visibly soft
        # penumbra and exercises the (expensive) soft-shadow bake path.
        extra = ['"_bake_only" "1"' if bake_only else '"_bake_only" "0"',
                 '"_shadow_type" "static_light_map"',
                 '"_light_size" "0.75"']
    else:
        cls = "light_dynamic_spot" if spot else "light_dynamic"
        extra = []
    if spot:
        cls = "light_spot" if mode == "static" else "light_dynamic_spot"
        extra += ['"_cone" "30"', '"_cone2" "48"', '"angles" "-90 0 0"']
    # Animation is a baked-light feature; runtime (dynamic) lights ignore it.
    if mode == "static" and animate == "kvp":
        # A one-period brightness pulse resampled by the compiler at 32 Hz.
        extra += ['"brightness_curve" "[0, 1.0] [800, 0.25] [1600, 1.0]"',
                  '"period_ms" "1600"']
    if mode == "static" and animate == "script":
        extra += [f'"_tags" "{SCRIPT_LIGHT_TAG}"']
    out = ["{", f'"classname" "{cls}"',
           f'"origin" "{origin[0]} {origin[1]} {origin[2]}"',
           f'"light" "{intensity}"', f'"_color" "{cr} {cg} {cb}"',
           f'"_falloff_range" "{falloff}"', '"delay" "0"', '"style" "0"']
    out += extra
    if carrier:
        out.append(f'"carrier" "{carrier}"')
    out.append("}")
    return out


# --- Gameplay entities: enemies, weapon pickups, doors, lifts ---------------

def enemy_entity(origin, yaw):
    """A pre-placed reference AI enemy (health + mesh + behavior graph)."""
    return ["{", f'"classname" "{ENEMY_CLASS}"',
            f'"origin" "{origin[0]} {origin[1]} {origin[2]}"',
            f'"angles" "0 {yaw} 0"', "}"]


def weapon_entity(origin, cls):
    """A pre-placed wieldable weapon pickup (touchable world item)."""
    return ["{", f'"classname" "{cls}"',
            f'"origin" "{origin[0]} {origin[1]} {origin[2]}"',
            '"angles" "0 0 0"', "}"]


def door_entities(idx, axis, line, dcenter, zf, tag, activation):
    """A sliding automatic door filling a maze doorway.

    Returns a list of entity blocks: the `kinematic_mover` door leaf (authored
    shut, sliding up out of the opening), its two waypoints, and a
    `trigger_volume` in the passage that opens it (`auto_close_ms` shuts it).
    `activation` is either `touch` or `use`; the latter requires the player's
    action button while they overlap the same two-sided passage volume.
    axis 'x' => wall on X=line, opening in Y at dcenter; 'y' => the transpose.
    """
    h = WALL_T // 2
    d0, d1 = dcenter - DOOR_W // 2, dcenter + DOOR_W // 2
    r = DOOR_TRIGGER_REACH
    if axis == "x":
        bx0, by0, bx1, by1 = line - h, d0, line + h, d1
        tx0, ty0, tx1, ty1 = line - r, d0, line + r, d1
    else:
        bx0, by0, bx1, by1 = d0, line - h, d1, line + h
        tx0, ty0, tx1, ty1 = d0, line - r, d1, line + r
    cz = zf + DOOR_H // 2
    cx, cy = (bx0 + bx1) // 2, (by0 + by1) // 2
    shut = f"warren_door_{idx}_shut"
    opn = f"warren_door_{idx}_open"

    mover = ["{", '"classname" "kinematic_mover"', f'"path" "{shut}"',
             '"speed" "4"', '"move_mode" "once"', '"start_on_spawn" "0"',
             '"auto_close_ms" "2500"', '"block_policy" "reverse"',
             f'"_tags" "{tag}"']
    mover += box_brush(bx0, by0, zf, bx1, by1, zf + DOOR_H,
                       MOVER_TEX, MOVER_TEX, MOVER_TEX).rstrip("\n").split("\n")
    mover.append("}")

    wp_shut = ["{", '"classname" "kinematic_waypoint"', f'"name" "{shut}"',
               f'"next" "{opn}"', f'"origin" "{cx} {cy} {cz}"', "}"]
    wp_open = ["{", '"classname" "kinematic_waypoint"', f'"name" "{opn}"',
               f'"origin" "{cx} {cy} {cz + DOOR_H}"', "}"]

    trig = ["{", '"classname" "trigger_volume"', f'"activation" "{activation}"',
            f'"target_tag" "{tag}"', '"command" "start"',
            '"fire_mode" "multiple"', '"rearm_ms" "3000"']
    trig += box_brush(tx0, ty0, zf, tx1, ty1, zf + DOOR_H,
                      MOVER_TEX, MOVER_TEX, MOVER_TEX).rstrip("\n").split("\n")
    trig.append("}")
    return [mover, wp_shut, wp_open, trig]


# A closet is a small, sealed pod built inside an existing tall room. Its door
# pulls out into the parent room rather than up through the closet lintel, so
# the mover never intersects static brushwork at either waypoint.
CLOSET_W = 512
CLOSET_D = 384
CLOSET_WALL_T = 40
CLOSET_TRIGGER_REACH = 256
CLOSET_ENEMY_COUNT = 2


def monster_closet_entities(idx, cx, front_y, zf, zc):
    """Return (static_brushes, entities) for one touch-activated monster closet.

    The trigger simultaneously starts the tagged door directly and fires a
    uniquely named data-script reaction, which in turn activates only this
    closet's `entity_spawner`.  The once-only trigger makes the reveal and
    spawn deterministic even if a player revisits the doorway.
    """
    x0, x1 = cx - CLOSET_W // 2, cx + CLOSET_W // 2
    y0, y1 = front_y, front_y + CLOSET_D
    d0, d1 = cx - DOOR_W // 2, cx + DOOR_W // 2
    ztop = zf + DOOR_H
    tex = MOVER_TEX
    brushes = [
        box_brush(x0, y0, zf, x0 + CLOSET_WALL_T, y1, zc, tex, tex, tex),
        box_brush(x1 - CLOSET_WALL_T, y0, zf, x1, y1, zc, tex, tex, tex),
        box_brush(x0, y1 - CLOSET_WALL_T, zf, x1, y1, zc, tex, tex, tex),
        box_brush(x0 + CLOSET_WALL_T, y0, ztop,
                  x1 - CLOSET_WALL_T, y0 + CLOSET_WALL_T, zc, tex, tex, tex),
        box_brush(x0 + CLOSET_WALL_T, y0, zf, d0, y0 + CLOSET_WALL_T, ztop,
                  tex, tex, tex),
        box_brush(d1, y0, zf, x1 - CLOSET_WALL_T, y0 + CLOSET_WALL_T, ztop,
                  tex, tex, tex),
    ]
    tag = f"warren_closet_door_{idx}"
    spawner_tag = f"warren_closet_spawner_{idx}"
    shut = f"warren_closet_{idx}_shut"
    opn = f"warren_closet_{idx}_open"
    door_cy = y0 + CLOSET_WALL_T // 2
    door_cz = zf + DOOR_H // 2
    mover = ["{", '"classname" "kinematic_mover"', f'"path" "{shut}"',
             '"speed" "4"', '"move_mode" "once"', '"start_on_spawn" "0"',
             '"block_policy" "reverse"', f'"_tags" "{tag}"']
    mover += box_brush(d0, y0, zf, d1, y0 + CLOSET_WALL_T, ztop,
                       tex, tex, tex).rstrip("\n").split("\n")
    mover.append("}")
    trigger = ["{", '"classname" "trigger_volume"',
               f'"name" "warren_closet_trigger_{idx}"', '"activation" "touch"',
               f'"target_tag" "{tag}"', '"command" "start"',
               f'"on_fire" "warren.closet.{idx}.spawn"',
               '"fire_mode" "once"', '"enabled_on_spawn" "1"']
    trigger += box_brush(d0, y0 - CLOSET_TRIGGER_REACH, zf,
                         d1, y0 + CLOSET_WALL_T, ztop,
                         tex, tex, tex).rstrip("\n").split("\n")
    trigger.append("}")
    spawner = ["{", '"classname" "entity_spawner"',
               f'"origin" "{cx} {y1 - 96} {zf + 16}"', '"angles" "0 180 0"',
               f'"archetype" "{ENEMY_CLASS}"', f'"count" "{CLOSET_ENEMY_COUNT}"',
               f'"_tags" "{spawner_tag}"', "}"]
    return brushes, [mover,
                      ["{", '"classname" "kinematic_waypoint"', f'"name" "{shut}"',
                       f'"next" "{opn}"', f'"origin" "{cx} {door_cy} {door_cz}"', "}"],
                      ["{", '"classname" "kinematic_waypoint"', f'"name" "{opn}"',
                       f'"origin" "{cx} {door_cy - DOOR_W - 32} {door_cz}"', "}"],
                      trigger, spawner]


LIFT_HALF = 168      # car half-extent (< SHAFT//2 = 192, fits the hole)
LIFT_FLOOR_T = 24
LIFT_WALL_T = 40
LIFT_CAR_H = STORY_H
LIFT_CEIL_T = 24


def lift_entities(idx, cx, cy, zf_low, climb, tag):
    """A multi-brush, ping-pong elevator car between two room layers.

    The floor, three cabin walls, and ceiling belong to one `kinematic_mover`,
    which is the engine's supported moving-group form.  Every other car gets a
    carried dynamic cabin light to exercise the mover/light binding path.
    """
    z0 = zf_low
    z1 = z0 + LIFT_FLOOR_T
    cz = z0 + LIFT_FLOOR_T // 2
    low = f"warren_lift_{idx}_low"
    high = f"warren_lift_{idx}_high"
    name = f"warren_lift_{idx}"
    mover = ["{", '"classname" "kinematic_mover"', f'"name" "{name}"',
             f'"path" "{low}"',
             '"speed" "6"', '"wait_ms" "1400"', '"move_mode" "ping_pong"',
             '"start_on_spawn" "1"', '"block_policy" "reverse"',
             f'"_tags" "{tag}"']
    mover += box_brush(cx - LIFT_HALF, cy - LIFT_HALF, z0,
                       cx + LIFT_HALF, cy + LIFT_HALF, z1,
                       MOVER_TEX, MOVER_TEX, MOVER_TEX).rstrip("\n").split("\n")
    # Three walls leave the -Y face open for boarding from the shaft stair.
    wall_z1 = z0 + LIFT_CAR_H
    for x0, y0, x1, y1 in (
        (cx - LIFT_HALF, cy + LIFT_HALF - LIFT_WALL_T,
         cx + LIFT_HALF, cy + LIFT_HALF),
        (cx - LIFT_HALF, cy - LIFT_HALF,
         cx - LIFT_HALF + LIFT_WALL_T, cy + LIFT_HALF),
        (cx + LIFT_HALF - LIFT_WALL_T, cy - LIFT_HALF,
         cx + LIFT_HALF, cy + LIFT_HALF),
    ):
        mover += box_brush(x0, y0, z1, x1, y1, wall_z1,
                           MOVER_TEX, MOVER_TEX, MOVER_TEX).rstrip("\n").split("\n")
    mover += box_brush(cx - LIFT_HALF, cy - LIFT_HALF,
                       wall_z1, cx + LIFT_HALF, cy + LIFT_HALF,
                       wall_z1 + LIFT_CEIL_T,
                       MOVER_TEX, MOVER_TEX, MOVER_TEX).rstrip("\n").split("\n")
    mover.append("}")
    wp_low = ["{", '"classname" "kinematic_waypoint"', f'"name" "{low}"',
              f'"next" "{high}"', f'"origin" "{cx} {cy} {cz}"', "}"]
    wp_high = ["{", '"classname" "kinematic_waypoint"', f'"name" "{high}"',
               f'"origin" "{cx} {cy} {cz + climb}"', "}"]
    entities = [mover, wp_low, wp_high]
    if idx % 2 == 0:
        entities.append(light_entity(
            "dynamic", (cx, cy, wall_z1 - 32), (140, 217, 255),
            600, 420, False, None, carrier=name))
    return entities


def generate(nx, ny, nz, seed, braid_prob, shaft_prob, lights_mode, light_every,
             crates_per_room, spot_frac, static_frac, lights_per_room, stairs,
             n_arenas, n_enemies, n_weapons, n_doors, door_activation, n_lifts,
             n_closets, animated_frac):
    rng = random.Random(seed)
    spot_stride = max(1, round(1.0 / spot_frac)) if spot_frac > 0 else 0
    # center the grid near origin
    ox = -(nx * LATTICE_PITCH_XY - CORRIDOR_BAND) // 2
    oy = -(ny * LATTICE_PITCH_XY - CORRIDOR_BAND) // 2
    oz = 0
    # X/Y hold room starts, not shared cell boundaries. Each following start is
    # separated by a real corridor band; X_END/Y_END hold the opposite room
    # walls. This distinction is the core reserved-airspace invariant.
    X = [ox + i * LATTICE_PITCH_XY for i in range(nx)]
    Y = [oy + j * LATTICE_PITCH_XY for j in range(ny)]
    X_END = [x + PITCH_XY for x in X]
    Y_END = [y + PITCH_XY for y in Y]
    Z = [oz + k * PITCH_Z for k in range(nz + 1)]

    # Player spawn cell (kept clear of arenas): interior of (min(1,nx-1), min(1,ny-1), 0).
    si, sj = min(1, nx - 1), min(1, ny - 1)

    # --- Reserve arenas out of the lattice ---------------------------------
    arenas = plan_arenas(nx, ny, nz, n_arenas, (si, sj))
    arena_cells = set()
    for a in arenas:
        for k in range(a["k0"], a["k1"]):
            for j in range(a["j0"], a["j1"]):
                for i in range(a["i0"], a["i1"]):
                    arena_cells.add((i, j, k))
    is_arena = lambda i, j, k: (i, j, k) in arena_cells
    # Boundaries (slabs) an arena owns: floor, mezzanine(s), ceiling.
    arena_slab_cells = set()
    for a in arenas:
        for k in range(a["k0"], a["k1"] + 1):
            for j in range(a["j0"], a["j1"]):
                for i in range(a["i0"], a["i1"]):
                    arena_slab_cells.add((k, i, j))

    # room id per cell, unique across layers (arena cells excluded)
    layers = []
    next_base = 0
    for k in range(nz):
        blocked = {(i, j) for (i, j, kk) in arena_cells if kk == k}
        rmap = tile_layer(nx, ny, rng, blocked)
        nrooms = (max(rmap.values()) + 1) if rmap else 0
        layers.append({c: next_base + r for c, r in rmap.items()})
        next_base += nrooms
    room_of = lambda i, j, k: layers[k][(i, j)]
    total_rooms = next_base

    # --- Connectivity planning: maze doors + staggered shafts --------------
    # Per layer, connect rooms with a randomized spanning tree (a "perfect"
    # maze) plus a few extra `braid_prob` loop doors, instead of dooring every
    # shared wall at random. This removes the long straight corridors the old
    # per-wall coin flip produced.
    #
    # Every doorway hugs one of two disjoint end-slots of its wall (slot_center).
    # To kill straight sightlines we alternate slots PER GRID LINE: the x-doors
    # sharing a grid row are walked left-to-right and assigned 0,1,0,1...; the
    # y-doors sharing a grid column likewise. Because rooms are at most two cells
    # wide, two consecutive doors in a row always flank a single shared room, so
    # that room's entry and exit doors land in disjoint slots -- a straight line
    # of sight can never cross a third room (you can still see through one
    # doorway into the single adjacent room). The two axes are independent
    # (x-doors slot along Y, y-doors along X), so no constraint couples them.
    # doors[(k, axis, i, j)] -> doorway center along the wall (absent => solid).
    # Boundaries touching an arena cell are skipped here (the arena owns its
    # perimeter walls and their entryways).
    doors = {}
    for k in range(nz):
        pair_bounds = {}   # (loRoom, hiRoom) -> [(boundary_key, lo, hi), ...]
        for j in range(ny):
            for i in range(nx):
                if is_arena(i, j, k):
                    continue
                r = layers[k][(i, j)]
                if i > 0 and not is_arena(i - 1, j, k) and layers[k][(i - 1, j)] != r:
                    p = tuple(sorted((r, layers[k][(i - 1, j)])))
                    pair_bounds.setdefault(p, []).append(
                        ((k, "x", i, j), Y[j], Y_END[j]))
                if j > 0 and not is_arena(i, j - 1, k) and layers[k][(i, j - 1)] != r:
                    p = tuple(sorted((r, layers[k][(i, j - 1)])))
                    pair_bounds.setdefault(p, []).append(
                        ((k, "y", i, j), X[i], X_END[i]))
        rooms_k = {layers[k][(i, j)]
                   for j in range(ny) for i in range(nx) if not is_arena(i, j, k)}
        # Randomized Kruskal: shuffle adjacencies; keep an edge if it joins two
        # components (a tree edge), else keep it only with braid_prob (a loop).
        parent = {r: r for r in rooms_k}
        chosen = []        # (key, lo, hi) for every door this layer (tree + braid)
        adj = list(pair_bounds.keys())
        rng.shuffle(adj)
        for a, b in adj:
            ra, rb = uf_find(parent, a), uf_find(parent, b)
            if ra != rb:
                parent[ra] = rb
            elif rng.random() >= braid_prob:
                continue
            chosen.append(rng.choice(pair_bounds[(a, b)]))

        # Slot every door by alternating along its grid line: x-doors (key axis
        # 'x') alternate down their row j by ascending i; y-doors alternate along
        # their column i by ascending j. Consecutive doors on a line flank one
        # shared room, so its two doors get opposite slots and never see through.
        x_by_row = {}
        y_by_col = {}
        for key, lo, hi in chosen:
            _, axis, i, j = key
            if axis == "x":
                x_by_row.setdefault(j, []).append((i, key, lo, hi))
            else:
                y_by_col.setdefault(i, []).append((j, key, lo, hi))
        for j, row in x_by_row.items():
            for n, (i, key, lo, hi) in enumerate(sorted(row)):
                doors[key] = slot_center(lo, hi, n & 1)
        for i, col in y_by_col.items():
            for n, (j, key, lo, hi) in enumerate(sorted(col)):
                doors[key] = slot_center(lo, hi, n & 1)

    # Vertical shafts: holes through interior slabs that portal-connect stacked
    # layers. The candidate cell pattern is shifted every layer (i+k, j+2k) so
    # holes never stack vertically -- no straight shaft-of-sight through a room
    # -- and at least one shaft per interior slab keeps the complex traversable.
    # Candidate cells inside an arena footprint are skipped (an arena has its own
    # vertical connection through the mezzanine gap).
    shafts = set()
    for k in range(1, nz):
        cands = [(i, j) for j in range(ny) for i in range(nx)
                 if (i + k) % 3 == 1 and (j + 2 * k) % 3 == 1
                 and not is_arena(i, j, k) and not is_arena(i, j, k - 1)]
        chosen = [c for c in cands if rng.random() < shaft_prob]
        if cands and not chosen:
            chosen = [rng.choice(cands)]
        for (i, j) in chosen:
            shafts.add((k, i, j))

    brushes = []
    # Hallway lights are static whenever the map uses a baked-light mode. The
    # explicit dynamic-only mode remains bake-free for renderer stress tests.
    corridor_lights = [] if lights_mode in ("static", "mixed") else None

    # Vertical room walls. Every ordinary room owns four walls; a selected maze
    # edge cuts matching door openings in the two room-to-corridor interfaces.
    # No room wall is shared with another room, so corridor airspace cannot cut
    # through either a room interior or a shared wall.
    for k in range(nz):
        zf = Z[k] + SLAB_T // 2
        zc = Z[k + 1] - SLAB_T // 2
        for j in range(ny):
            for i in range(nx):
                if is_arena(i, j, k):
                    continue
                r = room_of(i, j, k)
                wt = pick(WALL_TEX, r)               # wall texture varies by room
                emit_wall(brushes, "x", X[i], Y[j], Y_END[j], zf, zc,
                          doors.get((k, "x", i, j)) if i > 0 else None, wt)
                emit_wall(brushes, "x", X_END[i], Y[j], Y_END[j], zf, zc,
                          doors.get((k, "x", i + 1, j)) if i < nx - 1 else None, wt)
                emit_wall(brushes, "y", Y[j], X[i], X_END[i], zf, zc,
                          doors.get((k, "y", i, j)) if j > 0 else None, wt)
                emit_wall(brushes, "y", Y_END[j], X[i], X_END[i], zf, zc,
                          doors.get((k, "y", i, j + 1)) if j < ny - 1 else None, wt)

        # Ordinary rooms seal the exterior along their own spans. Bridge the
        # reserved corridor gaps on the four outer edges as well; without these
        # short pieces the corridor network would be open to the exterior at
        # every perimeter junction.
        for j in range(ny - 1):
            emit_wall(brushes, "x", X[0], Y_END[j], Y[j + 1], zf, zc, None,
                      pick(WALL_TEX, k + j))
            emit_wall(brushes, "x", X_END[-1], Y_END[j], Y[j + 1], zf, zc,
                      None, pick(WALL_TEX, k + j + nx))
        for i in range(nx - 1):
            emit_wall(brushes, "y", Y[0], X_END[i], X[i + 1], zf, zc, None,
                      pick(WALL_TEX, k + i))
            emit_wall(brushes, "y", Y_END[-1], X_END[i], X[i + 1], zf, zc,
                      None, pick(WALL_TEX, k + i + ny))

    # Room floor/ceiling slabs. Dedicated corridor floors/low roofs are emitted
    # separately below, filling every gap between these footprints without
    # overlapping them. Top and bottom boundaries are solid; interior room slabs
    # get sparse shafts so layers remain portal-connected.
    for k in range(nz + 1):
        for j in range(ny):
            for i in range(nx):
                if (k, i, j) in arena_slab_cells:
                    continue
                holed = (k, i, j) in shafts
                emit_slab(brushes, X[i], Y[j], X_END[i], Y_END[j], Z[k], holed,
                          pick(FLOOR_TEX, i + j), pick(CEIL_TEX, i + j + k))

    # The corridor grid is a sealed, lower-storey network in the reserved bands
    # between rooms. Cross-junctions naturally provide corners; sparse diagonal
    # guide fins provide angled treatment without entering room space.
    for k in range(nz):
        emit_reserved_corridor_grid(
            brushes, X, Y, Z[k] + SLAB_T // 2, pick(WALL_TEX, k), seed + k,
            lambda i, j, k=k: is_arena(i, j, k), corridor_lights)

    # Arenas: open two-storey volumes with entryways, a mezzanine ledge + gap,
    # and a walkable staircase. Placed after the lattice so they overwrite the
    # (skipped) cells they reserve.
    arena_rooms = []
    for a in arenas:
        arena_rooms.append(emit_arena(brushes, X, X_END, Y, Y_END, Z, a, rng))

    # Vertical traversal: thread a climbable jump-stair spiral up through every
    # shaft so the upper layers are reachable on foot (and the rising spiral
    # makes the easy-to-miss ceiling gap visible). Off by default? No -- a map
    # with unreachable upper layers is broken; --no-stairs opts out only if the
    # extra step brushes threaten a large grid's BSP-leaf budget. Shafts
    # chosen as lifts get a platform instead of stairs (below).
    lift_shafts = set(sorted(shafts)[:max(0, n_lifts)])
    nstairs = 0
    if stairs:
        for (k, i, j) in sorted(shafts):
            if (k, i, j) in lift_shafts:
                continue
            scx = (X[i] + X_END[i]) // 2
            scy = (Y[j] + Y_END[j]) // 2
            zf_low = Z[k - 1] + SLAB_T // 2
            stex = pick(WALL_TEX, k + i + j)
            nstairs += emit_shaft_stairs(brushes, scx, scy, zf_low, PITCH_Z, stex)

    spx = (X[si] + X_END[si]) // 2
    spy = (Y[sj] + Y_END[sj]) // 2
    spz = Z[0] + SLAB_T // 2 + 32

    # --- Per-room props: crate stacks + lights -----------------------------
    # Both need the room's interior rect, so invert cell -> room once (rooms are
    # single-layer). Arena cells are excluded (arenas are lit separately below).
    entities = []          # gameplay + light entity blocks (list-of-lines each)
    lights = corridor_lights or []  # kept separate only for the summary line
    n_script_lights = 0
    ncrates = 0
    room_rects = []        # (floor_z, ceil_z, x0i, x1i, y0i, y1i) for content placement
    # Shared animation budget (one-element list so callees can decrement it);
    # caps total animated baked lights at ANIMATED_LIGHT_CAP (see the const).
    anim_budget = [ANIMATED_LIGHT_CAP if animated_frac > 0 else 0]
    if (lights_mode != "none" or crates_per_room > 0 or n_enemies or n_weapons
            or n_closets):
        room_cells = {}
        for k in range(nz):
            for (i, j), r in layers[k].items():
                room_cells.setdefault(r, (k, []))[1].append((i, j))
        nlit = len(lights)
        for r in sorted(room_cells):
            k, cells = room_cells[r]
            i0 = min(c[0] for c in cells); i1 = max(c[0] for c in cells)
            j0 = min(c[1] for c in cells); j1 = max(c[1] for c in cells)
            x0i, x1i = X[i0] + WALL_T // 2, X_END[i1] - WALL_T // 2
            y0i, y1i = Y[j0] + WALL_T // 2, Y_END[j1] - WALL_T // 2
            zf = Z[k] + SLAB_T // 2                  # interior floor
            zc = Z[k + 1] - SLAB_T // 2              # interior ceiling
            room_rects.append((zf, zc, x0i, x1i, y0i, y1i))

            # crate stacks (one wood texture per room so abutting stacks match).
            # Remember each stack's base xy so lights can be scattered clear of
            # them (a spot directly over a crate bakes shadow onto its own floor).
            crate_tex = pick(CRATE_TEX, r)
            crate_bases = []
            for _ in range(crates_per_room):
                base = emit_crate_stack(brushes, x0i, y0i, x1i, y1i, zf, zc,
                                        crate_tex, rng)
                if base is not None:
                    crate_bases.append(base)
                    ncrates += 1

            if lights_mode != "none" and r % max(1, light_every) == 0:
                added, ns = emit_room_lights(
                    lights, x0i, x1i, y0i, y1i, zf, zc, crate_bases, rng,
                    lights_mode, lights_per_room, spot_stride, static_frac,
                    animated_frac, nlit, anim_budget)
                nlit += added
                n_script_lights += ns

    # Arenas get their own lights (bright bake-only fixture + scattered coverage)
    # near the ceiling, clear of the central gap.
    for ar in arena_rooms:
        added, ns = emit_room_lights(
            lights, ar["x0i"], ar["x1i"], ar["y0i"], ar["y1i"], ar["floor_z"],
            ar["ceil_z"],
            [], rng, lights_mode if lights_mode != "none" else "static",
            max(3, lights_per_room), spot_stride, static_frac, animated_frac,
            len(lights), anim_budget) if lights_mode != "none" else (0, 0)
        n_script_lights += ns

    entities.extend(lights)

    # --- Gameplay: enemies + weapon pickups distributed across rooms -------
    place_rects = []
    for (zf, zc, x0i, x1i, y0i, y1i) in room_rects:
        place_rects.append(("room", zf, x0i, x1i, y0i, y1i, None))
    for ar in arena_rooms:
        place_rects.append(("arena", ar["floor_z"], ar["x0i"], ar["x1i"],
                            ar["y0i"], ar["y1i"], ar["gap"]))
    prng = random.Random(seed ^ 0xA11CE)
    prng.shuffle(place_rects)

    def floor_point(rect, inset=256):
        _, zf, x0i, x1i, y0i, y1i, gap = rect
        for _ in range(8):
            px = prng.randint(x0i + inset, max(x0i + inset, x1i - inset))
            py = prng.randint(y0i + inset, max(y0i + inset, y1i - inset))
            if gap is not None:                      # keep off an arena's drop gap
                g0x, g0y, g1x, g1y = gap
                if g0x <= px <= g1x and g0y <= py <= g1y:
                    continue
            return px, py, zf
        return (x0i + x1i) // 2, y0i + inset, zf

    if place_rects:
        for e in range(max(0, n_enemies)):
            rect = place_rects[e % len(place_rects)]
            px, py, zf = floor_point(rect)
            entities.append(enemy_entity((px, py, zf + 16), prng.randint(0, 359)))
        for w in range(max(0, n_weapons)):
            rect = place_rects[(w + 3) % len(place_rects)]
            px, py, zf = floor_point(rect)
            entities.append(weapon_entity((px, py, zf + 16),
                                          WEAPON_CLASSES[w % len(WEAPON_CLASSES)]))

    # --- Gameplay: sliding doors over maze doorways ------------------------
    door_keys = list(doors.keys())
    random.Random(seed ^ 0xD0084).shuffle(door_keys)
    ndoors = 0
    for key in door_keys[:max(0, n_doors)]:
        k, axis, i, j = key
        dcenter = doors[key]
        zf = Z[k] + SLAB_T // 2
        # The mover occupies the lower-index room's interface with the
        # corridor; its trigger reaches across the dedicated band and into that
        # room, never through another room's interior.
        line = X[i] if axis == "x" else Y[j]
        entities.extend(door_entities(ndoors, axis, line, dcenter, zf,
                                      f"warren_door_{ndoors}", door_activation))
        ndoors += 1

    # --- Gameplay: sealed monster-closet pods ------------------------------
    # Spread pods over regular rooms (not arenas) and keep their pull-out doors
    # on the room's +Y half.  A minimum lattice room is 1024 u wide/deep, so the
    # 512 x 384 pod plus its 256 u trigger approach stays inside that room.
    requested_closets = max(0, n_closets)
    nclosets = 0
    closet_rects = list(room_rects)
    random.Random(seed ^ 0xC105E7).shuffle(closet_rects)
    for zf, zc, x0i, x1i, y0i, y1i in closet_rects[:requested_closets]:
        if x1i - x0i < CLOSET_W or y1i - y0i < CLOSET_D + CLOSET_TRIGGER_REACH:
            continue
        closet_brushes, closet_entities = monster_closet_entities(
            nclosets, (x0i + x1i) // 2, y0i + CLOSET_TRIGGER_REACH, zf, zc)
        brushes.extend(closet_brushes)
        entities.extend(closet_entities)
        nclosets += 1

    # --- Gameplay: lifts in the reserved shafts ----------------------------
    nlifts = 0
    for (k, i, j) in sorted(lift_shafts):
        scx = (X[i] + X_END[i]) // 2
        scy = (Y[j] + Y_END[j]) // 2
        zf_low = Z[k - 1] + SLAB_T // 2
        entities.extend(lift_entities(nlifts, scx, scy, zf_low, PITCH_Z,
                                     f"warren_lift_{nlifts}"))
        nlifts += 1

    return (brushes, (spx, spy, spz), total_rooms, entities, lights, ncrates,
            nstairs, len(arena_rooms), ndoors, nlifts, nclosets, n_script_lights)


def emit_room_lights(out, x0i, x1i, y0i, y1i, zf, zc, crate_bases, rng, lights_mode,
                     lights_per_room, spot_stride, static_frac, animated_frac,
                     global_idx, anim_budget):
    """Append one room's lights to `out`; return (count, script_light_count).

    Lights near the ceiling, SCATTERED across the room rather than one central
    fixture, so coverage is even and crate faces are lit (and shadowed) from
    several directions. Every Nth light (by GLOBAL count, so spot_frac holds
    across the whole map) is a downward spotlight. When lights are baked
    (static/mixed), a share (`animated_frac`) of the scattered lights animate --
    half via an authored `brightness_curve` (KVP-driven) and half via the
    `warren_script_pulse` tag the data script pulses (script-driven).

    Static room contract. Every static/mixed room receives one dim bake-only
    point fill and `lights_per_room` coverage lights. The normal settings use
    four static spots. Steady static coverage is also bake-only; animation is an
    explicit opt-in that promotes only the selected coverage lights so a script
    can address them. Dynamic and mixed modes remain deliberate stress overrides.
    """
    cz = zc - 24
    lx0, lx1 = x0i + LIGHT_MARGIN, x1i - LIGHT_MARGIN
    ly0, ly1 = y0i + LIGHT_MARGIN, y1i - LIGHT_MARGIN
    n_script = 0
    nlit = global_idx
    added = 0
    promotable = 0    # runtime-present (`_bake_only 0`) baked lights this room

    # One dim bake-only point fill per room. Its range reaches the far floor
    # corner, including the room's vertical distance, so it remains useful in
    # large arenas rather than only brightening their centre.
    if lights_mode in ("static", "mixed"):
        bx, by = (x0i + x1i) // 2, (y0i + y1i) // 2
        dim_falloff = math.ceil(math.sqrt(
            ((x1i - x0i) / 2) ** 2 + ((y1i - y0i) / 2) ** 2 + (cz - zf) ** 2
        ) + LIGHT_MARGIN)
        out.append(light_entity("static", (bx, by, cz), (255, 244, 214),
                                dim_falloff, DIM_POINT_INTENSITY, False, rng, animate=None,
                                bake_only=True))
        added += 1
    per = max(1, lights_per_room)
    for s in range(per):
        px, py = scatter_light_xy(lx0, lx1, ly0, ly1, crate_bases, rng)
        color = LIGHT_COLORS[nlit % len(LIGHT_COLORS)]
        spot = (spot_stride > 0 and nlit % spot_stride == 0)
        falloff = 1600 if spot else 1400
        intensity = SPOT_INTENSITY if spot else DIM_POINT_INTENSITY
        if lights_mode == "mixed":
            this_mode = "static" if rng.random() < static_frac else "dynamic"
        else:
            this_mode = lights_mode
        # Baked coverage lights: steady ones are BAKE-ONLY; an animated one is
        # PROMOTABLE (it keeps a runtime entity) but only while under the
        # promotable cap. Animation is a baked-light feature bounded by the
        # shared animation budget (the compiler's animated weight-map packer
        # limit -- see ANIMATED_LIGHT_CAP). Dynamic lights are runtime already;
        # `_bake_only` does not apply to them (light_entity ignores it).
        animate = None
        bake_only = False
        if this_mode == "static":
            want_animate = (animated_frac > 0 and anim_budget[0] > 0
                            and rng.random() < animated_frac)
            if want_animate and promotable < MAX_PROMOTABLE_PER_ROOM:
                anim_budget[0] -= 1
                # Alternate KVP/script across the animated set (by consumed
                # count, not nlit parity) so a map with >= 2 animated lights
                # always has both a KVP-driven and a script-driven one.
                used = ANIMATED_LIGHT_CAP - anim_budget[0]
                if used % 2 == 1:
                    animate = "kvp"
                else:
                    animate = "script"
                    n_script += 1
                promotable += 1       # animated coverage light is runtime-present
            else:
                bake_only = True      # steady (or cap-reached) coverage: bake only
        out.append(light_entity(this_mode, (px, py, cz), color, falloff,
                                intensity, spot, rng, animate=animate,
                                bake_only=bake_only))
        nlit += 1
        added += 1
    # Only animated coverage lights are promotable. Pure `--lights dynamic`
    # bakes nothing, so the baked-promotable rule is moot.
    if lights_mode in ("static", "mixed"):
        assert 0 <= promotable <= MAX_PROMOTABLE_PER_ROOM
    return added, n_script


def write_map(path, brushes, spawn, nx, ny, nz, entities, data_script):
    lines = []
    lines.append("// Game: Postretro")
    lines.append("// Format: Standard")
    lines.append(f"// Generated by gen_stress_map.py --grid {nx} {ny} {nz}")
    lines.append("// entity 0")
    lines.append("{")
    lines.append('"classname" "worldspawn"')
    lines.append('"initialGravity" "-9.81"')
    lines.append('"ambient_color" "64 64 72"')
    # The navmesh bake is unconditional and scales with footprint/cell_size^2; at
    # the default 0.25 m it dominates compile time for a map this large (minutes).
    # This map is a render/visibility stress test, not a pathfinding test, so bake
    # it coarse.
    lines.append('"nav_cell_size" "1.0"')
    lines.append('"wad" ""')
    lines.append('"_tb_mod" "dev"')
    if data_script:
        # Data script drives the script-animated lights; path is relative to the
        # .map directory.
        rel = os.path.relpath(data_script, os.path.dirname(os.path.abspath(path)))
        lines.append(f'"data_script" "{rel}"')
    for n, b in enumerate(brushes):
        lines.append(f"// brush {n}")
        lines.append(b.rstrip("\n"))
    lines.append("}")
    n = 1
    lines.append(f"// entity {n}")
    lines.append("{")
    lines.append('"classname" "player_spawn"')
    lines.append(f'"origin" "{spawn[0]} {spawn[1]} {spawn[2]}"')
    lines.append('"angle" "0"')
    lines.append("}")
    for ent in entities:
        n += 1
        lines.append(f"// entity {n}")
        lines.extend(ent)
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def write_generated_data_script(path, script_lights, closet_count):
    """Write the per-map reactions required by generated gameplay entities.

    `entity_spawner` deliberately has no map-KVP target field.  Each closet
    therefore owns a named reaction so it cannot accidentally release every
    other closet in the map.  The sidecar is written next to the generated map
    and referenced by a relative worldspawn `data_script` path.
    """
    lines = [
        "// Generated by tools/gen_stress_map.py; do not hand-edit.",
        'import { defineReaction, spawner, world } from "postretro";',
        "",
        "export function setupLevel(_ctx: unknown) {",
    ]
    if script_lights:
        lines.append(
            '  const lights = world.query({ component: "light", tag: "warren_script_pulse" });'
        )
    lines.append("  const reactions = [")
    if script_lights:
        lines += [
            '    defineReaction("levelLoad", {',
            "      sequence: lights.flatMap((light) =>",
            "        light.pulse({ min: 0.2, max: 1.0, periodMs: 1400 }),",
            "      ),",
            "    }),",
        ]
    for idx in range(closet_count):
        lines += [
            f'    defineReaction("warren.closet.{idx}.spawn",',
            f'      spawner({{ tag: "warren_closet_spawner_{idx}" }}).fire(),',
            "    ),",
        ]
    lines += ["  ];", "  return { reactions };", "}", ""]
    with open(path, "w") as f:
        f.write("\n".join(lines))


def main(argv):
    # Presets set a known-good bundle of knobs. Each preset value is applied
    # ONLY where the user did not pass that flag explicitly (we default the
    # flags to None and resolve below), so a preset never overrides the CLI.
    #
    # overflow: bake spills the lightmap atlas into >=2 texture_2d_array layers
    #           at bounded memory (many small crates + modest grid + moderate
    #           density). Lights are a point+spot BLEND (spot_frac=0.3): mostly
    #           bright point lights for room coverage + direct light on all atlas
    #           layers (navigable), plus ~30% spots to bake crate shadows. Crate
    #           count -- not light type -- drives the overflow. Recommended bake
    #           density: 0.08 m/texel. See the module docstring "Lightmap-array
    #           overflow preset" for the full rationale.
    #
    # warren:   the full gameplay showcase -- arenas, enemies, weapon pickups,
    #           sliding doors, lifts, and animated lights (KVP + script) on a
    #           conservative feature-heavy grid. Bake it at
    #           --lightmap-density 0.25: the animated-light weight-map packer
    #           needs enough texels per face, and the warren's large room walls
    #           collapse animated chunks into overlapping atlas rects at the
    #           coarser 0.5 (the packer aborts). 0.25 clears it and stays under
    #           the 8192^2 atlas cap.
    PRESETS = {
        "overflow": dict(
            grid=[4, 4, 2], crates=4, lights="static", spot_frac=0.3,
            light_every=1, door_prob=0.4, shaft_prob=1.0, lights_per_room=3,
        ),
        "warren": dict(
            # 6x5x3: nx >= aw+2 and ny >= ah+2 so one NFL-field arena (4x2 cells
            # at the stock pitch, see arena_cell_span) plus its one-cell frame
            # fits interior with room for the feature-heavy layout.
            grid=[6, 5, 3], lights="static", spot_frac=1.0, light_every=1,
            door_prob=0.3, shaft_prob=0.6, lights_per_room=4, crates=1,
            arenas=1, enemies=12, weapons=6, doors=6, door_activation="use",
            lifts=2, monster_closets=3, animated_frac=0.0,
        ),
    }

    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--preset", choices=sorted(PRESETS),
                    help="apply a named bundle of known-good knob values "
                         "(overridable per-flag on the CLI). 'overflow' => a map "
                         "whose bake overflows the lightmap atlas into >=2 array "
                         "layers at bounded memory; bake it at "
                         "--lightmap-density 0.06. 'warren' => the full gameplay "
                         "showcase (arenas, enemies, weapons, use doors, closets, lifts, "
                         "animated lights) on a conservative feature-heavy grid; bake it at "
                         "--lightmap-density 0.25.")
    ap.add_argument("--grid", nargs=3, type=int, default=None,
                    metavar=("NX", "NY", "NZ"),
                    help="cells along X, Y, and vertical layers (default 7 6 3, "
                         "a conservative size for the added corridor shells)")
    ap.add_argument("-o", "--out", default="content/dev/maps/stress-warren.map")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--door-prob", type=float, default=None,
                    help="maze braid factor: rooms are first connected by a "
                         "spanning-tree maze (one door per tree edge), then each "
                         "*extra* adjacency gets a loop door with this "
                         "probability. 0 = a perfect maze (one path between any "
                         "two rooms); higher = more loops and shortcuts. Tree "
                         "doors alternate wall slots so they never open a "
                         "straight sightline through a third room; braid doors "
                         "may occasionally, so keep this low. (default 0.15)")
    ap.add_argument("--shaft-prob", type=float, default=None,
                    help="fraction of candidate cells that get a vertical shaft "
                         "connecting layers (at least one is forced per interior "
                         "slab so the complex stays traversable; the candidate "
                         "pattern shifts each layer so shafts never stack into a "
                         "vertical sightline). (default 0.5)")
    ap.add_argument("--lights", choices=["none", "dynamic", "static", "mixed"],
                    default=None,
                    help="add lights per room. 'dynamic' = light_dynamic "
                         "(runtime, no bake; stresses the per-frame forward "
                         "light loop + the 96-slot spot / 6-slot cube shadow "
                         "pools). 'static' = light (baked; stresses the lightmap "
                         "+ SH bake -- much slower compile). 'mixed' = a per-room "
                         "blend of both (see --static-frac), stressing the bake "
                         "AND the runtime path in one scene. Static/mixed rooms "
                         "also get one dim bake-only point fill. (default "
                         "none)")
    ap.add_argument("--static-frac", type=float, default=0.5,
                    help="in --lights mixed, fraction of lights that are baked "
                         "(static); the rest are dynamic. (default 0.5)")
    ap.add_argument("--light-every", type=int, default=None, metavar="N",
                    help="place lights in every Nth room (default 1 = all)")
    ap.add_argument("--crates", type=int, default=None, metavar="N",
                    help="crate stacks per room (solid box-brushes on the floor; "
                         "cast spot-light shadows and add to the geometry walk, "
                         "but each spends BSP leaves so room count must drop). "
                         "(default 0)")
    ap.add_argument("--spot-frac", type=float, default=None,
                    help="fraction of lights that are spotlights. Only spots "
                         "cast shadows from world geometry (crates), so raise "
                         "this to stress shadow-map rendering. (default 1.0)")
    ap.add_argument("--lights-per-room", type=int, default=None, metavar="N",
                    help="number of scattered coverage lights per lit room "
                         "(default 4; on top of the per-room dim bake-only point). "
                         "Lights are jittered clear of crate stacks.")
    ap.add_argument("--animated-frac", type=float, default=None,
                    help="fraction of baked (static) coverage lights that "
                         "animate; half get a KVP brightness_curve and half are "
                         "script-driven (tagged, pulsed by the companion data "
                         "script). Only baked lights animate. (default 0.0)")
    ap.add_argument("--arenas", type=int, default=None, metavar="N",
                    help="number of arenas: large open two-storey rooms with "
                         "four entryways, a walkable staircase to a mezzanine "
                         "ledge, and a wide central gap the player can jump down "
                         "through. Each is sized so its interior is never smaller "
                         "than a regulation NFL field (360x160 ft = 4320x1920 u; "
                         "a 4x2-cell block at the stock pitch), reserved out of "
                         "the maze lattice. Arenas spend BSP leaves and need a "
                         "grid with room for the footprint plus a one-cell frame, "
                         "so a map with arenas needs a larger grid than the "
                         "footprint but fewer free rooms. (default 0)")
    ap.add_argument("--enemies", type=int, default=None, metavar="N",
                    help="number of reference_enemy AI enemies pre-placed across "
                         "the rooms (default 0)")
    ap.add_argument("--weapons", type=int, default=None, metavar="N",
                    help="number of wieldable weapon pickups pre-placed across "
                         "the rooms (default 0)")
    ap.add_argument("--doors", type=int, default=None, metavar="N",
                    help="number of maze doorways upgraded to automatic sliding "
                         "kinematic_mover doors (auto-close). "
                         "(default 0)")
    ap.add_argument("--door-activation", choices=["touch", "use"], default=None,
                    help="how generated maze-door triggers activate: 'touch' opens "
                         "on overlap; 'use' requires the action button while the "
                         "player overlaps the doorway trigger. (default touch; the "
                         "warren preset uses action-button doors)")
    ap.add_argument("--monster-closets", type=int, default=None, metavar="N",
                    help="number of sealed one-storey closet pods placed in regular "
                         "rooms. Each touch trigger opens its door and fires only its "
                         "own two-enemy entity_spawner through a generated data-script "
                         "reaction. (default 0)")
    ap.add_argument("--lifts", type=int, default=None, metavar="N",
                    help="number of shafts whose jump-stairs are replaced with a "
                         "ping-pong multi-brush elevator car between layers. "
                         "(default 0)")
    ap.add_argument("--no-stairs", action="store_true",
                    help="do NOT thread jump-stair spirals up through shafts. By "
                         "default every shaft gets a compact spiral of jumpable "
                         "platforms so upper layers are reachable on foot (a bare "
                         "shaft hole cannot be climbed) and the rising spiral "
                         "marks the ceiling gap. Disable only if the extra step "
                         "brushes make a large grid expensive to build.")
    args = ap.parse_args(argv)

    # Resolve each knob: explicit CLI value wins; else the preset's value (if a
    # preset was named and sets it); else the original committed default.
    preset = PRESETS.get(args.preset, {})
    _DEFAULTS = dict(grid=[7, 6, 3], door_prob=0.15, shaft_prob=0.5,
                     lights="none", light_every=1, crates=0, spot_frac=1.0,
                     lights_per_room=4, animated_frac=0.0, arenas=0, enemies=0,
                     weapons=0, doors=0, door_activation="touch", lifts=0,
                     monster_closets=0)

    def resolve(name):
        if getattr(args, name) is not None:
            return getattr(args, name)
        if name in preset:
            return preset[name]
        return _DEFAULTS[name]

    args.grid = resolve("grid")
    args.door_prob = resolve("door_prob")
    args.shaft_prob = resolve("shaft_prob")
    args.lights = resolve("lights")
    args.light_every = resolve("light_every")
    args.crates = resolve("crates")
    args.spot_frac = resolve("spot_frac")
    args.lights_per_room = resolve("lights_per_room")
    args.animated_frac = resolve("animated_frac")
    args.arenas = resolve("arenas")
    args.enemies = resolve("enemies")
    args.weapons = resolve("weapons")
    args.doors = resolve("doors")
    args.door_activation = resolve("door_activation")
    args.lifts = resolve("lifts")
    args.monster_closets = resolve("monster_closets")
    stairs = not args.no_stairs

    nx, ny, nz = args.grid
    if min(nx, ny, nz) < 1:
        ap.error("grid dimensions must be >= 1")

    # Envelope sanity check against the +/-16384-unit Quake bound.
    half_x = (nx * LATTICE_PITCH_XY - CORRIDOR_BAND) // 2
    half_y = (ny * LATTICE_PITCH_XY - CORRIDOR_BAND) // 2
    if max(half_x, half_y) > 16384:
        print(f"warning: grid spans +/-{max(half_x, half_y)} units, beyond the "
              f"classic +/-16384 envelope (still f32-exact, but unusually large)",
              file=sys.stderr)

    (brushes, spawn, rooms, entities, lights, ncrates, nstairs, narenas,
     ndoors, nlifts, nclosets, n_script_lights) = generate(
        nx, ny, nz, args.seed, args.door_prob, args.shaft_prob,
        args.lights, args.light_every, args.crates, args.spot_frac,
        args.static_frac, args.lights_per_room, stairs,
        args.arenas, args.enemies, args.weapons, args.doors, args.door_activation,
        args.lifts, args.monster_closets, args.animated_frac)
    data_script = None
    if n_script_lights or nclosets:
        data_script = os.path.splitext(os.path.abspath(args.out))[0] + ".generated.ts"
        write_generated_data_script(data_script, n_script_lights > 0, nclosets)
    write_map(args.out, brushes, spawn, nx, ny, nz, entities, data_script)
    if args.arenas > 0 and narenas < args.arenas:
        print(f"warning: requested {args.arenas} arena(s) but only seated "
              f"{narenas}; grid {nx}x{ny}x{nz} is too small to fit an NFL-field "
              f"arena ({'x'.join(map(str, arena_cell_span()))} cells) plus its "
              f"one-cell frame. Enlarge --grid.", file=sys.stderr)
    nspot = sum(1 for L in lights if "spot" in L[1])
    ndyn = sum(1 for L in lights if "dynamic" in L[1])
    nstat = len(lights) - ndyn
    nbake_only = sum(1 for L in lights if '"_bake_only" "1"' in L)
    npromote = sum(1 for L in lights if '"_bake_only" "0"' in L)
    nanim = sum(1 for L in lights
                if any("brightness_curve" in ln or SCRIPT_LIGHT_TAG in ln for ln in L))
    nenem = sum(1 for e in entities if e[1] == f'"classname" "{ENEMY_CLASS}"')
    nweap = sum(1 for e in entities
                if any(e[1] == f'"classname" "{c}"' for c in WEAPON_CLASSES))
    print(f"grid {nx}x{ny}x{nz} = {nx*ny*nz} cells -> {rooms} rooms, "
          f"{narenas} arenas, {len(brushes)} brushes ({ncrates} crates, "
          f"{nstairs} stair steps)")
    print(f"lights: {len(lights)} {args.lights} "
          f"({nstat} static, {ndyn} dynamic; {nspot} spot, {len(lights)-nspot} point; "
          f"{npromote} promotable, {nbake_only} bake-only, {nanim} animated)")
    print(f"gameplay: {nenem} enemies, {nweap} weapons, {ndoors} {args.door_activation} doors, "
          f"{nclosets} monster closets, {nlifts} lifts"
          + (f"; data_script {data_script}" if data_script else ""))
    print(f"extent: X/Y +/-{max(half_x, half_y)} u, Z {nz*PITCH_Z} u tall")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main(sys.argv[1:])
