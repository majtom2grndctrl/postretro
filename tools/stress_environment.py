"""Environment-volume and particle-drift content for the stress-warren generator.
Governing doc: `content/dev/maps/stress-env-volumes.README.md`.

`gen_stress_map.py` owns the warren skeleton; this module layers the content an
environment-resolution stress run needs on top of it:

* `fluid_volume` / `gravity_volume` brush entities (the E24 authoring
  vocabulary: `fluid` + `priority`, `gravity` + `priority`). Shapes cover the
  cases environment resolution must get right -- floor pools, floating blocks,
  room-wide gravity, pools nested inside gravity volumes, overlaps with tied
  priorities, volumes spanning a doorway, non-AABB convex hulls (yawed boxes,
  slanted tops, triangular prisms) and multi-brush entities.
* `billboard_emitter` point entities sized so a target particle count is live in
  steady state, placed near room centres where the floating blocks and pools sit
  so particles drift across volume boundaries.

Both are pure functions of their inputs plus a dedicated RNG stream, so turning
volumes on never perturbs any other placement: a zero-volume map is the volume
map minus its volume entities (the baseline the stress run compares against).

Map axes throughout (Quake units, Z-up). Gravity vectors are m/s^2 in map axes
and are only ever straight down or zero.
"""

import math
import random

# Dedicated RNG stream salts (XORed with the map seed). Distinct from every
# stream gen_stress_map.py uses so neither content kind shifts another.
VOLUME_STREAM = 0xF1D0
EMITTER_STREAM = 0xE3177E

FLUIDS = ("water", "sludge")
# Straight-down or zero, m/s^2 in map axes. Zero gravity is a valid authored
# value; the mix keeps several distinct values so resolution results differ.
GRAVITIES = ("0 0 -3", "0 0 0", "0 0 -1.62", "0 0 -6", "0 0 -14")
# A small priority range forces frequent ties, which resolve by map order.
PRIORITIES = (0, 1, 2)
# Every Nth explicit-0 volume omits `priority` instead, exercising the
# default (0): resolution is the same either way.
DEFAULT_PRIORITY_EVERY = 64

# Real textures, so the compiler's texture resolution stays quiet. A fluid
# surface will draw with its face texture once fluids render.
_C = "50-free-textures/"
FLUID_TEX = {"water": _C + "concrete_pavement_044", "sludge": _C + "default_dirt_011"}
GRAVITY_TEX = _C + "concrete_stone_030"

# Share of the volume budget spent on doorway-spanning volumes.
DOORWAY_SHARE = 20      # one in every N volumes, capped by the doorway count

# Particle emitters. `rate * lifetime` is an emitter's steady-state live count;
# the runtime caps one emitter at MAX_SPRITES_PER_EMITTER live particles
# (`MAX_SPRITES` in crates/render-cpu/src/fx/smoke.rs).
MAX_SPRITES_PER_EMITTER = 4096
EMITTER_SPRITE = "smoke_puff"
# Engine-axis buoyancy: vertical accel = gravity * -buoyancy. Rising emitters
# sit on the floor, falling ones near the ceiling; the drag bounds terminal
# speed so a particle crosses a few volume faces within its lifetime.
EMITTER_RISE_BUOYANCY = 0.15
EMITTER_FALL_BUOYANCY = -0.15
EMITTER_DRAG = 0.8
EMITTER_SPREAD = 0.6
EMITTER_CENTRE_JITTER = 128


def _num(v):
    """Integers print bare (matching box_brush); others to three decimals."""
    r = round(v)
    if abs(v - r) < 1e-9:
        return str(int(r))
    return f"{v:.3f}"


def _point(p):
    return f"( {_num(p[0])} {_num(p[1])} {_num(p[2])} )"


def _sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def _cross(a, b):
    return (a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def _dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _face(p1, p2, p3, inside, tex):
    """One Standard-format plane line with its normal facing away from `inside`.

    The Standard format's outward normal is (p3 - p1) x (p2 - p1) (the winding
    `box_brush` uses); swapping p2/p3 flips it, so orientation is derived from
    a known interior point rather than hand-wound per face.
    """
    n = _cross(_sub(p3, p1), _sub(p2, p1))
    if _dot(n, _sub(inside, p1)) > 0:
        p2, p3 = p3, p2
    return f"{_point(p1)} {_point(p2)} {_point(p3)} {tex} 0 0 0 1 1"


def prism_brush(footprint, z0, top_z, tex):
    """A convex vertical prism as brush lines.

    `footprint` is a convex XY polygon (either winding, no collinear runs);
    `top_z(x, y)` gives the top surface, which must be planar over the
    footprint (a constant or a linear ramp) and above `z0` everywhere. Side
    faces are vertical; the bottom is flat at `z0`.
    """
    tops = [top_z(x, y) for x, y in footprint]
    if min(tops) <= z0:
        raise ValueError("prism top must lie above its bottom")
    cx = sum(x for x, _ in footprint) / len(footprint)
    cy = sum(y for _, y in footprint) / len(footprint)
    inside = (cx, cy, (z0 + sum(tops) / len(tops)) / 2)
    lines = ["{"]
    a, b, c = footprint[0], footprint[1], footprint[2]
    lines.append(_face((a[0], a[1], z0), (b[0], b[1], z0), (c[0], c[1], z0),
                       inside, tex))
    lines.append(_face((a[0], a[1], tops[0]), (b[0], b[1], tops[1]),
                       (c[0], c[1], tops[2]), inside, tex))
    for n, p in enumerate(footprint):
        q = footprint[(n + 1) % len(footprint)]
        lines.append(_face((p[0], p[1], z0), (q[0], q[1], z0),
                           (p[0], p[1], z0 + 64), inside, tex))
    lines.append("}")
    return lines


def rect_footprint(cx, cy, length, width, yaw_deg=0.0):
    """A rectangle centred at (cx, cy), rotated `yaw_deg` about Z."""
    t = math.radians(yaw_deg)
    ux, uy = math.cos(t), math.sin(t)
    vx, vy = -uy, ux
    hl, hw = length / 2, width / 2
    return [(cx + ux * s + vx * w, cy + uy * s + vy * w)
            for s, w in ((-hl, -hw), (hl, -hw), (hl, hw), (-hl, hw))]


def flat_top(z):
    return lambda _x, _y: z


def ramp_top(footprint, yaw_deg, z_lo, z_hi):
    """A planar top rising from `z_lo` to `z_hi` along `yaw_deg` across the
    footprint: the slanted-top (wedge-like) hull."""
    t = math.radians(yaw_deg)
    dx, dy = math.cos(t), math.sin(t)
    s = [dx * x + dy * y for x, y in footprint]
    smin, smax = min(s), max(s)
    span = max(smax - smin, 1e-6)
    return lambda x, y: z_lo + (z_hi - z_lo) * ((dx * x + dy * y) - smin) / span


def volume_entity(kind, value, priority, brushes):
    """A `fluid_volume` (value = fluid name) or `gravity_volume` (value = a
    straight-down gravity vector string) owning one or more brushes."""
    if kind == "fluid":
        head = ['"classname" "fluid_volume"', f'"fluid" "{value}"']
    elif kind == "gravity":
        head = ['"classname" "gravity_volume"', f'"gravity" "{value}"']
    else:
        raise ValueError(f"unknown volume kind {kind!r}")
    out = ["{"] + head + [f'"priority" "{priority}"']
    for brush in brushes:
        out.extend(brush)
    out.append("}")
    return out


# --- Per-room volume recipes -------------------------------------------------
# Each recipe takes (rng, room) and returns one entity. `room` is the interior
# rect (zf, zc, x0i, x1i, y0i, y1i). Shapes centre near the room's middle; a
# yawed or slanted footprint can reach ~64 u past the interior rect, so faces
# may bury into the walls (and pools dip into the floor slab), but never reach
# a corridor band or the hull.

def _centre(room, rng, jitter):
    """A point near the room centre. In a room much larger than an ordinary
    one (an arena) the jitter widens so shapes spread across its floor while
    keeping ~384 u clear of the walls."""
    _, _, x0, x1, y0, y1 = room
    jx = max(jitter, (x1 - x0) // 2 - 384)
    jy = max(jitter, (y1 - y0) // 2 - 384)
    return ((x0 + x1) // 2 + rng.randint(-jx, jx),
            (y0 + y1) // 2 + rng.randint(-jy, jy))


def room_gravity(rng, room):
    """The whole room interior: the outer gravity every other volume nests in."""
    zf, zc, x0, x1, y0, y1 = room
    fp = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    return volume_entity("gravity", rng.choice(GRAVITIES), 0,
                         [prism_brush(fp, zf, flat_top(zc), GRAVITY_TEX)])


def floor_pool(rng, room):
    """An axis-aligned pool whose bottom face is buried in the floor slab."""
    zf = room[0]
    fluid = rng.choice(FLUIDS)
    cx, cy = _centre(room, rng, 256)
    fp = rect_footprint(cx, cy, rng.randint(192, 512), rng.randint(192, 512))
    return volume_entity("fluid", fluid, rng.choice(PRIORITIES),
                         [prism_brush(fp, zf - 32, flat_top(zf + rng.randint(48, 160)),
                                      FLUID_TEX[fluid])])


def slanted_pool(rng, room):
    """A non-AABB pool: a yawed footprint under a sloped top."""
    zf = room[0]
    fluid = rng.choice(FLUIDS)
    cx, cy = _centre(room, rng, 256)
    yaw = rng.randint(0, 359)
    fp = rect_footprint(cx, cy, rng.randint(192, 448), rng.randint(160, 384), yaw)
    top = ramp_top(fp, rng.randint(0, 359), zf + rng.randint(32, 64),
                   zf + rng.randint(128, 256))
    return volume_entity("fluid", fluid, rng.choice(PRIORITIES),
                         [prism_brush(fp, zf - 32, top, FLUID_TEX[fluid])])


def floating_block(rng, room):
    """A yawed fluid block in mid-air near the room centre, where the emitters
    are: rising particles enter its bottom, falling ones leave through it."""
    zf, zc = room[0], room[1]
    fluid = rng.choice(FLUIDS)
    cx, cy = _centre(room, rng, 160)
    fp = rect_footprint(cx, cy, rng.randint(160, 384), rng.randint(160, 384),
                        rng.choice((0, 15, 30, 45, 60)))
    z0 = rng.randint(zf + 128, zc - 224)
    return volume_entity("fluid", fluid, rng.choice(PRIORITIES),
                         [prism_brush(fp, z0, flat_top(z0 + rng.randint(64, 192)),
                                      FLUID_TEX[fluid])])


def overlap_gravity(rng, room):
    """A mid-height gravity box over the room centre: overlaps the pools and
    floating blocks around it, often at an equal priority."""
    zf, zc = room[0], room[1]
    cx, cy = _centre(room, rng, 192)
    fp = rect_footprint(cx, cy, rng.randint(256, 640), rng.randint(256, 640))
    z0 = zf + rng.randint(0, 96)
    return volume_entity("gravity", rng.choice(GRAVITIES), rng.choice(PRIORITIES),
                         [prism_brush(fp, z0, flat_top(min(zc, z0 + rng.randint(160, 320))),
                                      GRAVITY_TEX)])


def floating_wedge(rng, room):
    """A floating gravity prism with a triangular footprint."""
    zf, zc = room[0], room[1]
    cx, cy = _centre(room, rng, 224)
    r = rng.randint(128, 288)
    base = rng.randint(0, 119)
    fp = [(cx + r * math.cos(math.radians(base + a)),
           cy + r * math.sin(math.radians(base + a))) for a in (0, 120, 240)]
    z0 = rng.randint(zf + 96, zc - 192)
    return volume_entity("gravity", rng.choice(GRAVITIES), rng.choice(PRIORITIES),
                         [prism_brush(fp, z0, flat_top(z0 + rng.randint(96, 160)),
                                      GRAVITY_TEX)])


def multi_brush_pool(rng, room):
    """One fluid entity of two or three abutting boxes (an L or a step): the
    faces they share are interior to the entity."""
    zf = room[0]
    fluid = rng.choice(FLUIDS)
    cx, cy = _centre(room, rng, 192)
    a, b = rng.randint(160, 256), rng.randint(160, 256)
    depth = zf + rng.randint(64, 160)
    tex = FLUID_TEX[fluid]
    boxes = [
        prism_brush([(cx - a, cy - b), (cx, cy - b), (cx, cy), (cx - a, cy)],
                    zf - 32, flat_top(depth), tex),
        prism_brush([(cx, cy - b), (cx + a, cy - b), (cx + a, cy), (cx, cy)],
                    zf - 32, flat_top(depth), tex),
    ]
    if rng.random() < 0.5:
        boxes.append(prism_brush([(cx - a, cy), (cx, cy), (cx, cy + b), (cx - a, cy + b)],
                                 zf - 32, flat_top(depth + 64), tex))
    return volume_entity("fluid", fluid, rng.choice(PRIORITIES), boxes)


def slanted_gravity(rng, room):
    """A floor-standing gravity ramp: a sloped top over a yawed footprint."""
    zf, zc = room[0], room[1]
    cx, cy = _centre(room, rng, 256)
    yaw = rng.randint(0, 359)
    fp = rect_footprint(cx, cy, rng.randint(256, 512), rng.randint(192, 384), yaw)
    top = ramp_top(fp, yaw + 90, zf + rng.randint(48, 96), min(zc, zf + rng.randint(256, 448)))
    return volume_entity("gravity", rng.choice(GRAVITIES), rng.choice(PRIORITIES),
                         [prism_brush(fp, zf, top, GRAVITY_TEX)])


FULL_RECIPES = (floor_pool, floating_block, overlap_gravity, slanted_pool,
                floating_wedge, multi_brush_pool, slanted_gravity, floating_block)


def low_gravity_room(rng, room):
    """The whole room at lunar gravity: the brief's low-gravity playtest room."""
    zf, zc, x0, x1, y0, y1 = room
    fp = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    return volume_entity("gravity", "0 0 -1.62", 0,
                         [prism_brush(fp, zf, flat_top(zc), GRAVITY_TEX)])


# --- Anchored recipes ----------------------------------------------------------
# Combination zones place water relative to a feature that exists in BOTH the
# volume map and its baseline (a lift shaft, an emitter, a fog volume, a crate,
# an underwater light), so the volume is guaranteed to touch that feature.
# Anchors are computed by the feature placement, never from volumes.

def _anchored_box(kind, value, priority, cx, cy, half_x, half_y, z0, z1):
    fp = [(cx - half_x, cy - half_y), (cx + half_x, cy - half_y),
          (cx + half_x, cy + half_y), (cx - half_x, cy + half_y)]
    tex = FLUID_TEX[value] if kind == "fluid" else GRAVITY_TEX
    return volume_entity(kind, value, priority, [prism_brush(fp, z0, flat_top(z1), tex)])


def anchored_volumes(rng, room, anchor):
    """Volumes for one anchor `(tag, x, y, z)` in `room`."""
    tag, x, y, z = anchor
    zf, zc = room[0], room[1]
    fluid = rng.choice(FLUIDS)
    if tag == "lift_pool":          # the car's floor settles into a pool
        return [_anchored_box("fluid", fluid, 1, x, y, 224, 224, zf - 32, zf + 96)]
    if tag == "lift_block":         # the cabin rises through a floating block
        return [_anchored_box("fluid", fluid, 1, x, y, 224, 224, zf + 192, zf + 320)]
    if tag == "pool":               # a pool holding the anchor point
        return [_anchored_box("fluid", fluid, rng.choice(PRIORITIES),
                              x, y, 320, 320, zf - 32, zf + 96)]
    if tag == "emit_rise":          # rises out of a pool, through a block, out
        return [                    # of a zero-g layer into the room's gravity
            _anchored_box("fluid", fluid, 1, x, y, 160, 160, zf - 32, z + 48),
            _anchored_box("fluid", rng.choice(FLUIDS), 1, x, y, 192, 192,
                          zf + 256, zf + 352),
            _anchored_box("gravity", "0 0 0", 1, x, y, 256, 256, zf, zf + 200),
        ]
    if tag == "emit_fall":          # falls out of a block into a pool below
        return [
            _anchored_box("fluid", fluid, 1, x, y, 160, 160, z - 64, z + 64),
            _anchored_box("fluid", rng.choice(FLUIDS), 1, x, y, 256, 256,
                          zf - 32, zf + 96),
        ]
    if tag == "fog_box":            # fog overlapping a pool and a block
        return [_anchored_box("fluid", fluid, 1, x, y, 256, 256, zf - 32, zf + 128),
                _anchored_box("fluid", rng.choice(FLUIDS), 2, x, y, 128, 128,
                              z, z + 160)]
    if tag == "crate":              # a crate standing in a pool, one beside it
        return [_anchored_box("fluid", fluid, rng.choice(PRIORITIES),
                              x, y, 160, 160, zf - 32, zf + 64),
                _anchored_box("fluid", rng.choice(FLUIDS), rng.choice(PRIORITIES),
                              x + 256, y, 96, 160, zf - 32, zf + 48)]
    raise ValueError(f"unknown volume anchor {tag!r}")


# Zone -> the volume recipes a room of that zone receives (after its anchored
# volumes). Combination zones keep the set small so a defect is attributable;
# `mixed` rooms instead share the remaining budget, cycling every shape.
ZONE_RECIPES = {
    "water": (room_gravity,) + FULL_RECIPES * 2,
    "lift": (floor_pool,),
    "animated": (floating_block,),
    "arena": (floor_pool, multi_brush_pool, slanted_pool, floor_pool, slanted_pool,
              floor_pool, multi_brush_pool, floor_pool),
    "particles": (room_gravity,),
    "fog": (),
    "doors": (floor_pool,),
    "crates": (),
    "lowgrav": (low_gravity_room, floor_pool, floating_block),
}


def doorway_volume(rng, doorway):
    """A volume spanning a doorway: through both walls and the corridor band.

    `doorway` is the opening box (x0, y0, x1, y1, zf, ztop) in map units. Its
    faces cross solid wall, so some are buried and some are open air.
    """
    x0, y0, x1, y1, zf, ztop = doorway
    fp = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    if rng.random() < 0.5:
        fluid = rng.choice(FLUIDS)
        return volume_entity("fluid", fluid, rng.choice(PRIORITIES),
                             [prism_brush(fp, zf - 32, flat_top(zf + rng.randint(64, 128)),
                                          FLUID_TEX[fluid])])
    return volume_entity("gravity", rng.choice(GRAVITIES), rng.choice(PRIORITIES),
                         [prism_brush(fp, zf, flat_top(ztop), GRAVITY_TEX)])


def env_volume_entities(count, seed, rooms, doorways, zone_doorways=()):
    """Return exactly `count` environment-volume entities (fewer only if there
    is nowhere to put them).

    `rooms` is a list of `(zone, rect, anchors)`; `doorways` the opening boxes
    eligible for a doorway-spanning volume and `zone_doorways` those a zone
    must have (water-only and doors rooms). The budget is spent in order:
    every non-mixed room's anchored volumes and zone recipes (combination and
    water-only zones are small and fixed), one volume per zone doorway, then
    up to one in DOORWAY_SHARE volumes on other doorways, then the rest dealt over `mixed` rooms in a shuffled
    order -- each mixed room's first volume is its room-wide gravity, later
    ones cycle FULL_RECIPES. A few explicit-0 volumes then drop `priority`
    (omit_default_priorities). Uses only its own RNG stream, so `count` never
    changes any other placement.
    """
    if count <= 0 or not rooms:
        return []
    rng = random.Random(seed ^ VOLUME_STREAM)
    out = []

    def finish():
        return omit_default_priorities(out)

    def room_volumes(zone, rect, anchors):
        for anchor in anchors:
            yield from anchored_volumes(rng, rect, anchor)
        for recipe in ZONE_RECIPES[zone]:
            yield recipe(rng, rect)

    for zone, rect, anchors in rooms:
        if zone == "mixed":
            continue
        for entity in room_volumes(zone, rect, anchors):
            if len(out) == count:
                return finish()
            out.append(entity)

    for doorway in zone_doorways:
        if len(out) == count:
            return finish()
        out.append(doorway_volume(rng, doorway))

    n_door = min(len(doorways), (count - len(out)) // DOORWAY_SHARE)
    door_order = list(range(len(doorways)))
    rng.shuffle(door_order)
    for d in door_order[:n_door]:
        out.append(doorway_volume(rng, doorways[d]))

    mixed = [rect for zone, rect, _ in rooms if zone == "mixed"]
    if not mixed:
        return finish()
    order = list(range(len(mixed)))
    rng.shuffle(order)
    quota = [0] * len(mixed)
    for n in range(count - len(out)):
        quota[order[n % len(order)]] += 1
    for r, rect in enumerate(mixed):
        for n in range(quota[r]):
            recipe = room_gravity if n == 0 else FULL_RECIPES[(n - 1) % len(FULL_RECIPES)]
            out.append(recipe(rng, rect))
    return finish()


def omit_default_priorities(volumes):
    """Drop `"priority" "0"` from every DEFAULT_PRIORITY_EVERY-th volume that
    carries it, the first one included, so even a small map has one."""
    zeros = 0
    for entity in volumes:
        if '"priority" "0"' in entity:
            if zeros % DEFAULT_PRIORITY_EVERY == 0:
                entity.remove('"priority" "0"')
            zeros += 1
    return volumes


# --- Particle emitters -------------------------------------------------------

def emitter_entities(count, seed, rooms, rate, lifetime, salt=0):
    """`count` billboard emitters near room centres, alternating rising (on the
    floor) and falling (near the ceiling) so particles cross floating blocks,
    pools and room boundaries both ways. Steady state holds `rate * lifetime`
    live particles per emitter. Placement depends only on the rooms and its
    own RNG stream (`salt` separates independent calls), never on volumes.

    Returns `(entities, anchors)`; `anchors` holds `(room_index, anchor)` with
    an `emit_rise` / `emit_fall` anchor a zone can surround with volumes.
    """
    if count <= 0 or not rooms:
        return [], []
    if rate * lifetime > MAX_SPRITES_PER_EMITTER:
        raise ValueError(
            f"rate {rate} x lifetime {lifetime} exceeds the "
            f"{MAX_SPRITES_PER_EMITTER}-particle per-emitter cap")
    rng = random.Random(seed ^ EMITTER_STREAM ^ salt)
    order = list(range(len(rooms)))
    rng.shuffle(order)
    entities, anchors = [], []
    for e in range(count):
        room_index = order[e % len(order)]
        zf, zc, x0, x1, y0, y1 = rooms[room_index]
        j = EMITTER_CENTRE_JITTER
        px = (x0 + x1) // 2 + rng.randint(-j, j)
        py = (y0 + y1) // 2 + rng.randint(-j, j)
        rising = e % 2 == 0
        pz = zf + 48 if rising else zc - 96
        entities.append([
            "{", '"classname" "billboard_emitter"',
            f'"origin" "{px} {py} {pz}"',
            f'"sprite" "{EMITTER_SPRITE}"',
            f'"rate" "{_num(rate)}"', f'"lifetime" "{_num(lifetime)}"',
            f'"spread" "{EMITTER_SPREAD}"',
            f'"buoyancy" "{EMITTER_RISE_BUOYANCY if rising else EMITTER_FALL_BUOYANCY}"',
            f'"drag" "{EMITTER_DRAG}"',
            f'"initial_velocity_y" "{0.8 if rising else 0.0}"',
            "}",
        ])
        anchors.append((room_index, ("emit_rise" if rising else "emit_fall", px, py, pz)))
    return entities, anchors
