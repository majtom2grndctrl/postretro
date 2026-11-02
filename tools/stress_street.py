"""Street layout for the occluder classifier's proving ground (`--preset street`).
Governing doc: `content/dev/maps/stress-street.README.md`.

`gen_stress_map.py` owns the warren; this module lays out a city street instead,
built to exercise occluder-first partitioning (context/lib/build_pipeline.md
§Compiler pipeline) on purpose:

* A building-lined street canyon that snakes around two corners (a left turn,
  then a right). A promenade runs along its floor, an elevated street deck
  hangs off one facade above it, and a further deck for opposite-direction
  traffic hangs off the other facade above that. Ramps join the levels.
* One side is a continuous row of abutting buildings that wraps both corners;
  the other side is broken by side streets, each closed by a barrier on the
  ground and a guard on the deck, so players stay off a full grid.
* Every facade is narrow strips around see-through windows (no strip reaches
  the classifier's 1.2 m floor on two axes), so a facade can only occlude as a
  cluster. A free-standing building stands in the plaza at the street's start.
* Lamp posts are TrenchBroom linked groups (pole, arm and head) holding a light;
  curbs, railings and deck columns are the small exempt brushes that must not
  slice the street into columns.

Dimensions are world metres converted to map units (1 u = 0.0254 m) only at
emission, so brushes that share a metre coordinate share a unit coordinate and
touch exactly. Several pieces sit on either side of a classifier constant (the
20 m absolute length, half the open span, the 2.5 m see-over height, the 50 %
cluster coverage); the README lists them. Map axes throughout (Quake units,
Z-up). Nothing here draws randomness: the map is a pure function of this file.
"""

import math
from dataclasses import dataclass, field

from stress_environment import prism_brush, ramp_top

UNIT_M = 0.0254

# Classifier starting constants (research.md §Constants), mirrored from the Rust
# sources crates/level-compiler/src/partition/occluder_classify.rs and
# crates/level-compiler/src/partition/occluder_cluster.rs; update here when
# those are retuned. Recorded so the tests and README can show which side of
# each a piece sits on.
FLOOR_M = 1.2
ABSOLUTE_M = 20.0
SEE_OVER_M = 2.5
COVERAGE = 0.5

# How high a player reaches from the ground: the jump apex under the default
# gravity plus the vault's maxHeight above the feet (content/dev/scripts/
# player.ts: jumpVelocity 6.86 m/s, gravity 20.32 m/s^2, vault maxHeight 1.6 m).
# A barrier taller than this cannot be climbed.
PLAYER_REACH_M = 6.86 ** 2 / (2 * 20.32) + 1.6

# --- Hull -------------------------------------------------------------------
HULL_X = 140.0        # interior plan extent; the sealing slabs sit outside it
HULL_Y = 84.0
SKY_Z = 30.0          # the "sky" is a ceiling slab; open air above every roof
HULL_T = 1.0
FILL_H = 16.0         # solid city blocks behind and between the buildings

# --- Street canyon ------------------------------------------------------------
# Facade line A, the abutting row's street face, travelled from the plaza. The
# street lies to the right of travel: offset `d` metres from line A toward the
# far facade B (d = CANYON_W). Turns: left at (88, 30), right at (88, 74).
ROUTE = ((48.0, 30.0), (88.0, 30.0), (88.0, 74.0), (140.0, 74.0))
CANYON_W = 12.0
PLAZA = (0.0, 0.0, 48.0, 48.0)     # x0, y0, x1, y1

# Facades: strips in a slab FACADE_T thick behind the facade line. A storey is
# a 1.0 m spandrel band over a 1.5 m window row; every band and pier stays under
# FLOOR_M on one of its two largest axes, so each strip is exempt alone. Decks
# sit inside a spandrel band so no window is cut by a deck.
FACADE_T = 0.4
STOREY = 2.5
BAND = 1.0
ROOM_D = 3.0          # see-through room behind each facade
ROOM_END = 0.3        # solid kept at each end of a room (party walls)
ROOF_T = 0.5
BUILDING_D = 10.0     # building footprint depth from its facade line

# Window patterns: (pier width, nominal window width). Coverage (the strips'
# share of the facade) brackets the 50 % cluster constant: ~63 % standard,
# ~68 % dense, ~49 % glassy.
PATTERNS = {
    "standard": (0.8, 1.6),
    "dense": (1.0, 1.2),
    "glassy": (0.3, 3.3),
}

# Decks: (offset band from line A, slab z range). The elevated street hangs off
# facade B over the promenade; the opposite-direction deck hangs off facade A
# above it. Their bands overlap by 3 m, leaving a 4 m light well over the
# promenade along facade A.
LOWER_DECK = ((4.0, 12.0), (5.5, 6.0))
UPPER_DECK = ((0.0, 7.0), (10.5, 11.0))
RAILING = (4.0, 4.15)          # lower deck's open (well-side) edge, 1 m tall
RAILING_H = 1.0
CURB = (3.7, 4.0)              # promenade curb under the lower deck's edge
CURB_H = 0.3
COLUMN = (4.2, 5.0)            # deck columns, ground to deck soffit, 0.8 m square
COLUMNS = ((54, 64, 74, 84), (36, 46, 56), (106, 116, 126, 136))

# Side streets: gaps in side B, d = 12..30 from line A (18 m deep, ending at the
# hull or a block). A ground barrier closes each 6 m in; a guard closes the
# deck edge across the gap.
SIDE_STREET_D = (CANYON_W, CANYON_W + 18.0)
BARRIER_D = (18.0, 18.5)
BARRIER_H = 3.0
GUARD_D = (11.7, 12.0)
GUARD_H = 3.0

# Ramps (metres): plaza -> elevated street, elevated street -> upper deck.
# A 0.2 m lip at the foot stays under the 0.5 m step height; slope ~20 deg.
RAMP_LIP = 0.2
PLAZA_RAMP = ((32.0, 18.0, 48.0, 22.0), 0.0, 6.0)   # rect, foot z, head z
DECK_RAMP_SEG, DECK_RAMP_ALONG, DECK_RAMP_D = 0, (52.0, 68.0), (7.0, 11.0)

# Buildings: name -> (storeys, pattern, has a see-through room). Corner pieces
# are solid behind their strips (recessed windows) and carry both arms of the
# facade around a corner.
BUILDINGS = {
    "A0": (6, "standard", True), "A1": (7, "dense", True),
    "A-corner": (8, "standard", False),
    "A2": (6, "standard", True), "A3": (8, "dense", True), "A4": (7, "standard", True),
    "A5": (6, "dense", True), "A6": (7, "standard", True), "A7": (6, "standard", True),
    "B0": (6, "dense", True), "B1": (7, "standard", True),
    "B2": (6, "standard", True), "B3": (7, "dense", True),
    "B-corner": (6, "standard", False),
    "B4": (8, "standard", True), "B5": (6, "glassy", True),
}
# Frontages per segment: (building, along end); None as the end runs to the
# facade band's end, and None as the building marks a side-street gap.
ROW_A = (
    (("A0", 62), ("A1", 78), ("A-corner", None)),
    (("A-corner", 40), ("A2", 54), ("A3", 66), ("A4", None)),
    (("A5", 106), ("A6", 124), ("A7", None)),
)
ROW_B = (
    (("B0", 66), (None, 74), ("B1", None)),
    (("B2", 34), (None, 42), ("B3", 52), ("B-corner", None)),
    (("B-corner", 110), ("B4", 126), (None, 134), ("B5", None)),
)

# The free-standing building in the plaza: 16 m x 24 m, so one pair of faces
# passes the 20 m absolute length and the other relies on its open span.
FREE_BUILDING = ((8.0, 12.0, 24.0, 36.0), 6, "standard")

# Lamp posts: one linked group, every instance a quarter-turn and translation
# of one base lamp. Base in integer map units, arm along +X: a 0.2 m pole 5 m
# tall, a 1.5 m arm, a head under the arm's end and a light 0.25 m below it.
LAMP_LINKED_GROUP = "{5f0c3e2a-7b1d-4c8e-9a6f-2d4b8e1c0a71}"
LAMP_BASE = (                  # (x0, y0, z0, x1, y1, z1) in map units
    ("pole", (-4, -4, 0, 4, 4, 197)),
    ("arm", (4, -3, 189, 59, 3, 197)),
    ("head", (47, -6, 181, 63, 6, 189)),
)
LAMP_LIGHT = (55, 0, 171)
LAMP_RANGE_M = 12.0
# (segment, along, d, level z). Promenade and upper-deck lamps stand by facade
# A with the arm over the street; elevated-street lamps by facade B. None as the
# segment is a plaza lamp given in plan (x, y, arm quarter-turn).
LAMPS = (
    [(0, a, 1.0, 0.0) for a in (56, 72)] + [(1, a, 1.0, 0.0) for a in (46, 58)]
    + [(2, a, 1.0, 0.0) for a in (104, 120, 134)]
    + [(0, a, 11.4, 6.0) for a in (76, 92)] + [(1, a, 11.4, 6.0) for a in (28, 50)]
    + [(2, a, 11.4, 6.0) for a in (106, 118, 138)]
    + [(0, a, 1.0, 11.0) for a in (56, 80)] + [(1, a, 1.0, 11.0) for a in (40, 56)]
    + [(2, a, 1.0, 11.0) for a in (106, 122, 136)]
    + [(None, 40.0, 8.0, 1), (None, 40.0, 42.0, 3)]
)
# Script-pulsed neon lights by the free-standing building's east face; the
# generated data script drives them, which is the sidecar's job.
NEON = (((25.0, 18.0, 4.0), (255, 0, 200)), ((25.0, 30.0, 4.0), (0, 255, 200)))

SPAWN = (40.0, 26.0)            # plaza, facing east up the street
# Named sweep poses (map metres, feet z, facing degrees CCW from +X).
POSES = (
    ("plaza", (40.0, 26.0, 0.0), 0),
    ("promenade", (90.0, 32.0, 0.0), 90),
    ("elevated street", (96.0, 30.0, 6.0), 90),
    ("upper deck", (100.0, 70.0, 11.0), 0),
)

_C = "50-free-textures/"
FACADE_TEX = _C + "concrete_stone_025"
BLOCK_TEX = _C + "concrete_stone_027"
ROOF_TEX = _C + "concrete_stone_033"
DECK_TEX = _C + "concrete_pavement_038"
DECK_SIDE_TEX = _C + "concrete_stone_029"
GROUND_TEX = _C + "concrete_pavement_036"
SKY_TEX = _C + "concrete_stone_031"
HULL_TEX = _C + "concrete_stone_021"
BARRIER_TEX = _C + "concrete_stone_022"
CURB_TEX = _C + "concrete_pavement_040"
RAMP_TEX = _C + "concrete_pavement_042"
LAMP_TEX = _C + "concrete_stone_035"


def u(m):
    """World metres to integer map units. The inner round collapses float noise
    so two computations of one metre coordinate land on one unit."""
    return round(round(m, 6) / UNIT_M)


# --- Route geometry -----------------------------------------------------------

def _segments():
    segs = []
    for (x0, y0), (x1, y1) in zip(ROUTE, ROUTE[1:]):
        axis = 0 if y0 == y1 else 1
        sign = 1 if (x1 - x0, y1 - y0)[axis] > 0 else -1
        t = (sign, 0) if axis == 0 else (0, sign)
        segs.append(dict(a=(x0, y0), b=(x1, y1), axis=axis, sign=sign,
                         r=(t[1], -t[0])))       # right of travel, toward B
    return segs


SEGMENTS = _segments()


def cross_range(i, d0, d1):
    """The absolute cross-axis range of offsets d0..d1 on segment `i`."""
    s = SEGMENTS[i]
    c = 1 - s["axis"]
    v0, v1 = s["a"][c] + s["r"][c] * d0, s["a"][c] + s["r"][c] * d1
    return (min(v0, v1), max(v0, v1))


def span_rect(i, lo, hi, d0, d1):
    """Plan rect (x0, y0, x1, y1) of along lo..hi and offsets d0..d1 on segment i."""
    c0, c1 = cross_range(i, d0, d1)
    lo, hi = min(lo, hi), max(lo, hi)
    return (lo, c0, hi, c1) if SEGMENTS[i]["axis"] == 0 else (c0, lo, c1, hi)


def band_along(i, d0, d1):
    """Along range of the offset band d0..d1 on segment i. At a corner a segment
    runs to the far side of the next one's band and the next starts past its
    own, so the band turns as touching rectangles, mitre-free."""
    s = SEGMENTS[i]

    def far(rng):
        return rng[1] if s["sign"] > 0 else rng[0]

    lo = s["a"][s["axis"]] if i == 0 else far(cross_range(i - 1, d0, d1))
    hi = s["b"][s["axis"]] if i == len(SEGMENTS) - 1 else far(cross_range(i + 1, d0, d1))
    return (min(lo, hi), max(lo, hi))


def band(d0, d1):
    """The offset band d0..d1 along the whole route, one rect per segment."""
    return [span_rect(i, *band_along(i, d0, d1), d0, d1) for i in range(len(SEGMENTS))]


def route_point(i, along, d):
    """Plan point at `along` on segment i, offset d toward facade B."""
    s = SEGMENTS[i]
    p = list(s["a"])
    p[s["axis"]] = along
    return (p[0] + s["r"][0] * d, p[1] + s["r"][1] * d)


def quarter_turn(vec):
    """Quarter turns (CCW) taking +X to the plan direction `vec`."""
    return {(1, 0): 0, (0, 1): 1, (-1, 0): 2, (0, -1): 3}[tuple(vec)]


# --- Facades ------------------------------------------------------------------

def building_height(storeys):
    return storeys * STOREY + BAND


def facade_strips(lo, hi, storeys, pattern):
    """Strip boxes `(along0, along1, z0, z1)` of one frontage: a spandrel band
    under every storey and over the top one, full frontage length, and piers
    between the windows of each storey. Windows widen to fit the frontage."""
    pier, window = PATTERNS[pattern]
    length = hi - lo
    n = max(1, int((length - pier) // (pier + window)))
    window = (length - (n + 1) * pier) / n
    strips = [(lo, hi, k * STOREY, k * STOREY + BAND) for k in range(storeys + 1)]
    for k in range(storeys):
        z0, z1 = k * STOREY + BAND, (k + 1) * STOREY
        for p in range(n + 1):
            a = lo + p * (pier + window)
            strips.append((a, a + pier, z0, z1))
    return strips


def coverage(strips, lo, hi, height):
    """Share of a frontage's face its strips cover (they never overlap)."""
    solid = sum((a1 - a0) * (z1 - z0) for a0, a1, z0, z1 in strips)
    return solid / ((hi - lo) * height)


def frontages():
    """Every building frontage and side-street gap, in route order."""
    out, gaps = [], []
    for side, rows, (d0, d1) in (("A", ROW_A, (-FACADE_T, 0.0)),
                                 ("B", ROW_B, (CANYON_W, CANYON_W + FACADE_T))):
        for i, row in enumerate(rows):
            lo, end = band_along(i, d0, d1)
            for name, hi in row:
                hi = end if hi is None else hi
                if name is None:
                    gaps.append(dict(side=side, seg=i, lo=lo, hi=hi))
                else:
                    storeys, pattern, room = BUILDINGS[name]
                    out.append(dict(name=name, side=side, seg=i, lo=lo, hi=hi,
                                    storeys=storeys, pattern=pattern, room=room,
                                    slab=(d0, d1)))
                lo = hi
    return out, gaps


# --- Solid fill ----------------------------------------------------------------

def _inside(rect, x, y):
    return rect[0] < x < rect[2] and rect[1] < y < rect[3]


def solid_fill(free, footprints):
    """Solid blocks over the hull plan outside every free rect, row-merged into
    boxes `(x0, y0, x1, y1, height)`. A block takes the height of the building
    footprint it lies in, else FILL_H, so a room's back and party walls stand
    as tall as its building. Free rects are the open street, side streets,
    plaza and rooms."""
    rects = free + [f for f, _ in footprints] + [(0.0, 0.0, HULL_X, HULL_Y)]
    xs = sorted({round(min(max(v, 0.0), HULL_X), 6) for r in rects for v in (r[0], r[2])})
    ys = sorted({round(min(max(v, 0.0), HULL_Y), 6) for r in rects for v in (r[1], r[3])})
    boxes, open_runs = [], {}
    for j in range(len(ys) - 1):
        cy = (ys[j] + ys[j + 1]) / 2
        runs = []
        for i in range(len(xs) - 1):
            cx = (xs[i] + xs[i + 1]) / 2
            if any(_inside(r, cx, cy) for r in free):
                continue
            h = max((fh for f, fh in footprints if _inside(f, cx, cy)), default=FILL_H)
            if runs and runs[-1][1] == xs[i] and runs[-1][2] == h:
                runs[-1][1] = xs[i + 1]
            else:
                runs.append([xs[i], xs[i + 1], h])
        keys = {tuple(r) for r in runs}
        for key in sorted(set(open_runs) - keys):
            x0, x1, h = key
            boxes.append((x0, open_runs.pop(key), x1, ys[j], h))
        for key in sorted(keys - set(open_runs)):
            open_runs[key] = ys[j]
    for key in sorted(open_runs):
        x0, x1, h = key
        boxes.append((x0, open_runs[key], x1, ys[-1], h))
    return boxes


# --- Lamp posts ------------------------------------------------------------------

def lamp_transform(q, t):
    """The quarter-turn `q` about Z then translation `t` (map units) as the
    row-major 4x4 TrenchBroom writes in `_tb_transformation`."""
    c, s = ((1, 0), (0, 1), (-1, 0), (0, -1))[q]
    return (c, -s, 0, t[0], s, c, 0, t[1], 0, 0, 1, t[2], 0, 0, 0, 1)


def apply_transform(m, p):
    return (m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
            m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
            m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11])


def transform_box(m, box):
    a = apply_transform(m, box[:3])
    b = apply_transform(m, box[3:])
    return tuple(min(a[k], b[k]) for k in range(3)) + tuple(max(a[k], b[k]) for k in range(3))


def lamp_placements():
    """Every lamp as (quarter turn, translation in map units)."""
    out = []
    for seg, along, d, z in LAMPS:
        if seg is None:
            out.append((z, (u(along), u(d), 0)))   # plaza: (x, y, turn)
            continue
        x, y = route_point(seg, along, d)
        r = SEGMENTS[seg]["r"]
        arm = r if d < CANYON_W / 2 else (-r[0], -r[1])   # over the street
        out.append((quarter_turn(arm), (u(x), u(y), u(z))))
    return out


# --- Assembly -------------------------------------------------------------------

@dataclass
class Role:
    role: str
    name: str
    box: tuple          # metres (x0, y0, z0, x1, y1, z1); a ramp's bounds


@dataclass
class Street:
    brushes: list = field(default_factory=list)     # worldspawn brush strings
    roles: list = field(default_factory=list)       # parallel to `brushes`
    entities: list = field(default_factory=list)    # after player_spawn
    spawn: tuple = ()
    frontages: list = field(default_factory=list)
    side_streets: list = field(default_factory=list)
    lamps: list = field(default_factory=list)       # (group id, matrix, boxes, light)
    script_lights: int = 0


def generate(box_brush, script_tag):
    """Build the street. `box_brush` is the generator's axis-aligned brush
    factory (map units); `script_tag` tags the lights the data script pulses."""
    st = Street()

    def emit(role, name, box, side, top=None, bottom=None):
        x0, y0, z0, x1, y1, z1 = box
        st.brushes.append(box_brush(u(x0), u(y0), u(z0), u(x1), u(y1), u(z1),
                                    side, top or side, bottom or side))
        st.roles.append(Role(role, name, box))

    def emit_rect(role, name, rect, z0, z1, side, top=None, bottom=None):
        emit(role, name, (rect[0], rect[1], z0, rect[2], rect[3], z1), side, top, bottom)

    def emit_ramp(name, rect, axis, z_foot, z_head):
        fp = [(u(rect[0]), u(rect[1])), (u(rect[2]), u(rect[1])),
              (u(rect[2]), u(rect[3])), (u(rect[0]), u(rect[3]))]
        lo, hi = u(z_foot + RAMP_LIP), u(z_head)
        st.brushes.append("\n".join(prism_brush(fp, u(z_foot),
                                                ramp_top(fp, 0 if axis == 0 else 90, lo, hi),
                                                RAMP_TEX)) + "\n")
        st.roles.append(Role("ramp", name, (rect[0], rect[1], z_foot,
                                            rect[2], rect[3], z_head)))

    # Hull: ground, sky and four walls around the interior plan.
    t = HULL_T
    emit("hull", "ground", (-t, -t, -t, HULL_X + t, HULL_Y + t, 0.0), HULL_TEX, GROUND_TEX)
    emit("hull", "sky", (-t, -t, SKY_Z, HULL_X + t, HULL_Y + t, SKY_Z + t), SKY_TEX)
    emit("hull", "west", (-t, -t, 0.0, 0.0, HULL_Y + t, SKY_Z), HULL_TEX)
    emit("hull", "east", (HULL_X, -t, 0.0, HULL_X + t, HULL_Y + t, SKY_Z), HULL_TEX)
    emit("hull", "south", (0.0, -t, 0.0, HULL_X, 0.0, SKY_Z), HULL_TEX)
    emit("hull", "north", (0.0, HULL_Y, 0.0, HULL_X, HULL_Y + t, SKY_Z), HULL_TEX)

    # Buildings: strips, rooms and roofs; footprints set the block heights.
    st.frontages, gaps = frontages()
    free = [PLAZA] + band(-FACADE_T, CANYON_W + FACADE_T)
    footprints = []
    for f in st.frontages:
        h = building_height(f["storeys"])
        i, lo, hi = f["seg"], f["lo"], f["hi"]
        back = (-BUILDING_D, 0.0) if f["side"] == "A" else (CANYON_W, CANYON_W + BUILDING_D)
        footprints.append((span_rect(i, lo, hi, *back), h))
        slab = f["slab"]
        f["strips"] = facade_strips(lo, hi, f["storeys"], f["pattern"])
        f["height"] = h
        f["coverage"] = coverage(f["strips"], lo, hi, h)
        for a0, a1, z0, z1 in f["strips"]:
            emit_rect("facade_strip", f["name"], span_rect(i, a0, a1, *slab), z0, z1, FACADE_TEX)
        if f["room"]:
            room_d = ((-FACADE_T - ROOM_D, -FACADE_T) if f["side"] == "A"
                      else (CANYON_W + FACADE_T, CANYON_W + FACADE_T + ROOM_D))
            room = span_rect(i, lo + ROOM_END, hi - ROOM_END, *room_d)
            free.append(room)
            emit_rect("roof", f["name"], room, h - ROOF_T, h, ROOF_TEX)

    # Side streets, each closed by a ground barrier and a deck-edge guard.
    for g in gaps:
        i, lo, hi = g["seg"], g["lo"], g["hi"]
        g["rect"] = span_rect(i, lo, hi, *SIDE_STREET_D)
        free.append(g["rect"])
        emit_rect("barrier", f"side street {len(st.side_streets)}",
                  span_rect(i, lo, hi, *BARRIER_D), 0.0, BARRIER_H, BARRIER_TEX)
        deck_top = LOWER_DECK[1][1]
        emit_rect("deck_guard", f"side street {len(st.side_streets)}",
                  span_rect(i, lo, hi, *GUARD_D), deck_top, deck_top + GUARD_H, BARRIER_TEX)
        st.side_streets.append(g)

    for x0, y0, x1, y1, h in solid_fill(free, footprints):
        emit("block", "", (x0, y0, 0.0, x1, y1, h), BLOCK_TEX, ROOF_TEX)

    # The free-standing building: four strip facades round a see-through
    # interior, roofed. North and south facades run the full width.
    (bx0, by0, bx1, by1), storeys, pattern = FREE_BUILDING
    h = building_height(storeys)
    ft = FACADE_T
    sides = (((bx0, bx1), (by0, by0 + ft), 0), ((bx0, bx1), (by1 - ft, by1), 0),
             ((by0 + ft, by1 - ft), (bx0, bx0 + ft), 1),
             ((by0 + ft, by1 - ft), (bx1 - ft, bx1), 1))
    for (lo, hi), (c0, c1), axis in sides:
        for a0, a1, z0, z1 in facade_strips(lo, hi, storeys, pattern):
            rect = (a0, c0, a1, c1) if axis == 0 else (c0, a0, c1, a1)
            emit_rect("free_strip", "free-standing", rect, z0, z1, FACADE_TEX)
    emit_rect("free_roof", "free-standing", (bx0 + ft, by0 + ft, bx1 - ft, by1 - ft),
              h - ROOF_T, h, ROOF_TEX)

    # Decks over the promenade, both turning both corners.
    for role, ((d0, d1), (z0, z1)) in (("lower_deck", LOWER_DECK), ("upper_deck", UPPER_DECK)):
        for n, rect in enumerate(band(d0, d1)):
            emit_rect(role, f"segment {n}", rect, z0, z1, DECK_SIDE_TEX, DECK_TEX, DECK_SIDE_TEX)
    deck_top = LOWER_DECK[1][1]
    for n, rect in enumerate(band(*RAILING)):
        emit_rect("railing", f"segment {n}", rect, deck_top, deck_top + RAILING_H, BARRIER_TEX)
    for n, rect in enumerate(band(*CURB)):
        emit_rect("curb", f"segment {n}", rect, 0.0, CURB_H, CURB_TEX)
    soffit = LOWER_DECK[1][0]
    for n, alongs in enumerate(COLUMNS):
        for a in alongs:
            half = (COLUMN[1] - COLUMN[0]) / 2
            emit_rect("column", f"segment {n}", span_rect(n, a - half, a + half, *COLUMN),
                      0.0, soffit, DECK_SIDE_TEX)

    # Ramps: plaza up to the elevated street, then up to the upper deck.
    rect, foot, head = PLAZA_RAMP
    emit_ramp("plaza", rect, 0, foot, head)
    emit_ramp("deck", span_rect(DECK_RAMP_SEG, *DECK_RAMP_ALONG, *DECK_RAMP_D),
              SEGMENTS[DECK_RAMP_SEG]["axis"], LOWER_DECK[1][1], UPPER_DECK[1][1])

    # Lamp posts: one TrenchBroom linked group, each instance a group entity
    # owning its three brushes plus a light that names it through `_tb_group`.
    for gid, (q, tr) in enumerate(lamp_placements(), start=1):
        m = lamp_transform(q, tr)
        boxes = [transform_box(m, box) for _, box in LAMP_BASE]
        light = apply_transform(m, LAMP_LIGHT)
        group = ["{", '"classname" "func_group"', '"_tb_type" "_tb_group"',
                 '"_tb_name" "Lamp post"', f'"_tb_id" "{gid}"',
                 f'"_tb_linked_group_id" "{LAMP_LINKED_GROUP}"',
                 '"_tb_transformation" "' + " ".join(str(v) for v in m) + '"']
        for n, b in enumerate(boxes):
            group.append(f"// brush {n}")
            group.append(box_brush(*b, LAMP_TEX, LAMP_TEX, LAMP_TEX).rstrip("\n"))
        group.append("}")
        st.entities.append(group)
        st.entities.append(["{", '"classname" "light"',
                            f'"origin" "{light[0]} {light[1]} {light[2]}"',
                            '"light" "220"', '"_color" "255 214 160"',
                            f'"_falloff_range" "{u(LAMP_RANGE_M)}"', '"delay" "0"',
                            '"_bake_only" "0"', '"_shadow_type" "static_light_map"',
                            f'"_tb_group" "{gid}"', "}"])
        st.lamps.append((gid, m, boxes, light))

    for (x, y, z), (cr, cg, cb) in NEON:
        st.entities.append(["{", '"classname" "light"',
                            f'"origin" "{u(x)} {u(y)} {u(z)}"', '"light" "260"',
                            f'"_color" "{cr} {cg} {cb}"', f'"_falloff_range" "{u(8.0)}"',
                            '"delay" "0"', '"_bake_only" "0"',
                            '"_shadow_type" "static_light_map"', f'"_tags" "{script_tag}"',
                            "}"])
        st.script_lights += 1

    # Pawn origin 32 u above the floor, as the warren places it.
    st.spawn = (u(SPAWN[0]), u(SPAWN[1]), 32)
    return st


def engine_pose(point, facing_deg):
    """A `--start-pose` (engine metres) for a map-metre point and plan facing.

    Map (x, y, z) -> engine (-y, z, -x); the pawn origin sits 32 u above the
    feet like `player_spawn`; engine yaw 0 faces -Z (map +X).
    """
    x, y, z = point
    return f"{-y:.2f},{z + 32 * UNIT_M:.2f},{-x:.2f},{facing_deg:.0f},0"


def route_turns():
    """Turn direction at each corner: +1 left, -1 right."""
    out = []
    for s, n in zip(SEGMENTS, SEGMENTS[1:]):
        ts = (s["sign"], 0) if s["axis"] == 0 else (0, s["sign"])
        tn = (n["sign"], 0) if n["axis"] == 0 else (0, n["sign"])
        out.append(int(math.copysign(1, ts[0] * tn[1] - ts[1] * tn[0])))
    return out
