#!/usr/bin/env python3
"""Generate a dense, multi-layer "warren" stress map for Postretro.

Purpose
-------
The runtime currently walks the whole geometry/BVH every frame. To find out
whether that is a real bottleneck (and to later validate BVH node-visibility
masks) we need a map that pushes the *room/node count* as high as possible
while staying inside the engine's real size envelope.

The binding engine constraint
-----------------------------
prl-build rejects any map with more than 4096 geometry-bearing BSP leaves
(`MAX_CELL_ID_EXCLUSIVE` in `bvh_build.rs`): the runtime visible-cell bitmask
is a fixed 4096-bit structure. This -- not coordinates or buffer widths -- is
what caps room count. Every doorway and shaft fragments the empty space into
extra leaves, so the trade-off is direct: more connectivity per room => fewer
rooms fit under the 4096 cap. `--door-prob` / `--shaft-prob` expose that knob.
Watch the "geometry leaves" line prl-build prints and keep it under 4096.

Design (why it compiles and does not leak)
------------------------------------------
* A uniform axis-aligned 3D lattice of cells. Axis-aligned grids are the
  best case for the brush BSP splitter (clean axis-aligned cuts, almost no
  spanning brushes), so the tree stays shallow well under the
  MAX_RECURSION_DEPTH=256 guard in `partition/brush_bsp.rs`.
* Coordinates stay inside the classic Quake +/-16384-unit envelope. Input is
  parsed as f32 (`parse.rs`), whose integers are exact to 16.7M, so every
  vertex here is represented exactly.
* No static `light` entities. With zero baked lights the lightmap is a
  placeholder (no 8192^2 atlas cap, no multi-hour bake), so the per-frame
  geometry/BVH walk is the dominant cost -- exactly what we want to measure.
* The complex is fully sealed: solid edge walls and solid top/bottom slabs
  wrap the whole grid, so the exterior flood-fill (leaf culling) cannot reach
  the interior and delete geometry.

Varying room sizes
------------------
Each layer is tiled greedily with random rectangular blocks (1x1..2x2 cells),
so rooms come in several footprints while every room stays a clean rectangular
box that never overlaps a neighbour. Interior shared walls inside one room are
omitted (the cells fuse into one air volume). Rooms are connected as a maze:
per layer a randomized spanning tree (plus a few `--door-prob` braid doors)
decides which shared walls get a doorway, and adjacent doorways alternate
between two disjoint end-slots of their walls so a room's entry and exit doors
never line up -- no straight line of sight ever crosses a third room (you can
still see through one doorway into the adjacent room). Vertical shafts punch holes
through interior slabs to portal-connect the stacked layers; the shaft pattern
shifts every layer so shafts never stack into a vertical sightline.

All coordinates are emitted in Quake units (Z-up); prl-build applies the
1 unit = 0.0254 m scale and the Z-up -> Y-up swizzle.

Usage
-----
    python3 tools/gen_stress_map.py            # committed default, fits the cap
    python3 tools/gen_stress_map.py --grid 8 8 4 --door-prob 0.2
    # crate stacks (shadow-casting occluders) + spot-heavy dynamic lights:
    python3 tools/gen_stress_map.py --grid 7 6 3 --lights dynamic \
        --crates 2 --spot-frac 0.5

Then compile with a COARSE SH probe spacing (the SH irradiance volume bakes a
probe grid over the whole world AABB regardless of lights; at the default 1.0 m
spacing a map this large would bake millions of probes and gigabytes):

    prl-build content/dev/maps/stress-warren.map \\
        -o content/dev/maps/stress-warren.prl --sh-probe-spacing 10.0 --no-cache

Push the room count up by enlarging the grid and/or lowering --door-prob, and
watch that the compile stays under the 4096 BSP-leaf cap (see below).

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

The preset sets: --grid 4 4 2, --crates 10, --lights static, --spot-frac 0.3,
--light-every 1, --door-prob 0.15, --shaft-prob 0.5. The light mix is a
POINT+SPOT BLEND (coverage + crate shadows): every room gets a light, ~70% of
them point (`light`) and ~30% spot (`light_spot`). WHY the blend -- point lights
are omnidirectional, so they fully light each room (navigable, no dim-SH-only
surfaces) AND directly light crate faces on EVERY atlas layer, which is the
clean confirmation that the multi-layer atlas renders direct light correctly.
Spotlights are narrow cones that leave faces angled away from the cone dark, so
a pure-spot map reads as broken; we keep ~30% spots only because static spots
(`_shadow_type static_light_map`) are what bake crate SHADOWS into the lightmap,
which we still want to verify. Crate count -- not light type -- drives the atlas
overflow, so the blend does not affect the >=2-layer result. Every preset value
is still overridable on the CLI.

  Recommended bake (writes the .prl to /tmp; do NOT commit it):

    python3 tools/gen_stress_map.py --preset overflow \\
        -o content/dev/maps/stress-warren-overflow.map
    RUST_LOG=info ./target/release/prl-build --no-cache \\
        --lightmap-density 0.08 --sh-probe-spacing 10.0 \\
        content/dev/maps/stress-warren-overflow.map -o /tmp/stress-overflow.prl

  Look for `[PRL] Lightmap: WxH atlas, N layer(s)` with N >= 2. If N == 1, lower
  the density a notch (e.g. 0.06) or raise --crates; if the bake OOMs or reports
  ChartTooLarge, RAISE the density (coarser) so the per-layer dim drops.
"""

import argparse
import random
import sys

# --- Lattice geometry (Quake units) ---------------------------------------
PITCH_XY = 1280   # cell pitch on X and Y; interior = PITCH_XY - WALL_T = 1024
WALL_T = 256      # wall thickness -> horizontal room interior 1024 x 1024 (>= 1024)
PITCH_Z = 384     # vertical cell pitch; interior height = PITCH_Z - SLAB_T = 256
SLAB_T = 128      # floor/ceiling slab thickness (>= 256-tall rooms)

DOOR_W = 256      # doorway opening width
DOOR_H = 192      # doorway opening height (leaves a solid lintel under the ceiling)
SHAFT = 384       # vertical shaft opening (square hole in interior slabs)

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


def pick(pool, key):
    """Deterministically pick a texture from a pool by an integer key."""
    return pool[key % len(pool)]

# Crate stacks: small solid box-brushes piled on the room floor. They are world
# geometry, so they (a) add to the per-frame geometry/BVH walk, (b) cast real
# dynamic shadows under SPOT lights (the spot-shadow depth pass rasterizes world
# geometry; point/cube shadows render entity occluders only -- see
# rendering_pipeline.md §4, §7.1), and (c) carve the room's empty leaf into
# several BSP leaves, so they spend the 4096-leaf budget and lower room count.
CRATE_EDGE = 112      # crate cube edge (Quake units, ~2.8 m)
CRATE_MARGIN = 192    # keep stacks this far from interior walls (clear of doors)

# Cyberpunk-ish palette (0-255 RGB) so lights vary in color.
LIGHT_COLORS = [
    (0, 255, 200), (255, 0, 200), (255, 160, 40), (40, 160, 255),
    (180, 0, 255), (0, 255, 120), (255, 60, 60), (120, 220, 255),
]


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


def tile_layer(nx, ny, rng):
    """Greedy random rectangular tiling of one layer.

    Returns room_id[(i,j)] -> int. Blocks are 1x1..2x2, so room footprints vary
    while every room is a clean non-overlapping rectangle.
    """
    room = {}
    rid = 0
    for j in range(ny):
        for i in range(nx):
            if (i, j) in room:
                continue
            w = 2 if (i + 1 < nx and (i + 1, j) not in room and rng.random() < 0.45) else 1
            h = 2 if (j + 1 < ny and (i, j + 1) not in room and rng.random() < 0.45) else 1
            # only take the 2x2 corner if it is free
            if w == 2 and h == 2 and (i + 1, j + 1) in room:
                h = 1
            for dj in range(h):
                for di in range(w):
                    room[(i + di, j + dj)] = rid
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


def light_entity(mode, origin, color, falloff, intensity, spot, rng):
    """Return a light entity block (list of "key value" lines + classname).

    mode: 'dynamic' -> light_dynamic / light_dynamic_spot (runtime, unbaked:
          stresses the per-frame forward light loop + shadow pools, no bake).
    mode: 'static'  -> light (baked: stresses the lightmap + SH bake).
    """
    cr, cg, cb = color
    if mode == "static":
        cls = "light"
        # `_light_size` is the bake-only emitter radius (metres) that drives the
        # soft-shadow penumbra. The default 0.25 m is sub-texel at our coarse
        # lightmap density, so shadows bake hard; 0.75 m gives a visibly soft
        # penumbra and exercises the (expensive) soft-shadow bake path.
        extra = ['"_bake_only" "0"', '"_shadow_type" "static_light_map"',
                 '"_light_size" "0.75"']
    else:
        cls = "light_dynamic_spot" if spot else "light_dynamic"
        extra = []
    if spot:
        cls = "light_spot" if mode == "static" else "light_dynamic_spot"
        extra += ['"_cone" "30"', '"_cone2" "48"', '"angles" "-90 0 0"']
    out = ["{", f'"classname" "{cls}"',
           f'"origin" "{origin[0]} {origin[1]} {origin[2]}"',
           f'"light" "{intensity}"', f'"_color" "{cr} {cg} {cb}"',
           f'"_falloff_range" "{falloff}"', '"delay" "0"', '"style" "0"']
    out += extra
    out.append("}")
    return out


def generate(nx, ny, nz, seed, braid_prob, shaft_prob, lights_mode, light_every,
             crates_per_room, spot_frac, static_frac):
    rng = random.Random(seed)
    spot_stride = max(1, round(1.0 / spot_frac)) if spot_frac > 0 else 0
    # center the grid near origin
    ox = -(nx * PITCH_XY) // 2
    oy = -(ny * PITCH_XY) // 2
    oz = 0
    X = [ox + i * PITCH_XY for i in range(nx + 1)]
    Y = [oy + j * PITCH_XY for j in range(ny + 1)]
    Z = [oz + k * PITCH_Z for k in range(nz + 1)]

    # room id per cell, unique across layers
    layers = []
    next_base = 0
    for k in range(nz):
        rmap = tile_layer(nx, ny, rng)
        nrooms = max(rmap.values()) + 1
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
    doors = {}
    for k in range(nz):
        rooms_k = set(layers[k].values())
        pair_bounds = {}   # (loRoom, hiRoom) -> [(boundary_key, lo, hi), ...]
        for j in range(ny):
            for i in range(nx):
                r = layers[k][(i, j)]
                if i > 0 and layers[k][(i - 1, j)] != r:
                    p = tuple(sorted((r, layers[k][(i - 1, j)])))
                    pair_bounds.setdefault(p, []).append(
                        ((k, "x", i, j), Y[j], Y[j + 1]))
                if j > 0 and layers[k][(i, j - 1)] != r:
                    p = tuple(sorted((r, layers[k][(i, j - 1)])))
                    pair_bounds.setdefault(p, []).append(
                        ((k, "y", i, j), X[i], X[i + 1]))
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
    shafts = set()
    for k in range(1, nz):
        cands = [(i, j) for j in range(ny) for i in range(nx)
                 if (i + k) % 3 == 1 and (j + 2 * k) % 3 == 1]
        chosen = [c for c in cands if rng.random() < shaft_prob]
        if cands and not chosen:
            chosen = [rng.choice(cands)]
        for (i, j) in chosen:
            shafts.add((k, i, j))

    brushes = []

    # Vertical walls. For each cell, emit its low-X and low-Y boundary, plus the
    # far edges. Interior boundaries between two cells of the same room are open.
    for k in range(nz):
        zf = Z[k] + SLAB_T // 2
        zc = Z[k + 1] - SLAB_T // 2
        for j in range(ny):
            for i in range(nx):
                r = room_of(i, j, k)
                wt = pick(WALL_TEX, r)               # wall texture varies by room
                # X-boundary at X[i] (between cell i-1 and i)
                if i == 0:
                    emit_wall(brushes, "x", X[0], Y[j], Y[j + 1], zf, zc, None, wt)
                elif room_of(i - 1, j, k) != r:
                    emit_wall(brushes, "x", X[i], Y[j], Y[j + 1], zf, zc,
                              doors.get((k, "x", i, j)), wt)
                if i == nx - 1:
                    emit_wall(brushes, "x", X[nx], Y[j], Y[j + 1], zf, zc, None, wt)
                # Y-boundary at Y[j]
                if j == 0:
                    emit_wall(brushes, "y", Y[0], X[i], X[i + 1], zf, zc, None, wt)
                elif room_of(i, j - 1, k) != r:
                    emit_wall(brushes, "y", Y[j], X[i], X[i + 1], zf, zc,
                              doors.get((k, "y", i, j)), wt)
                if j == ny - 1:
                    emit_wall(brushes, "y", Y[ny], X[i], X[i + 1], zf, zc, None, wt)

    # Horizontal slabs at every Z-boundary, full cell footprint. Top and bottom
    # boundaries (k==0, k==nz) are always solid (seal). Interior boundaries get a
    # sparse shaft so layers are portal-connected.
    for k in range(nz + 1):
        for j in range(ny):
            for i in range(nx):
                holed = (k, i, j) in shafts
                emit_slab(brushes, X[i], Y[j], X[i + 1], Y[j + 1], Z[k], holed,
                          pick(FLOOR_TEX, i + j), pick(CEIL_TEX, i + j + k))

    # Player spawn: interior of cell (min(1,nx-1), min(1,ny-1), 0).
    si, sj = min(1, nx - 1), min(1, ny - 1)
    spx = (X[si] + X[si + 1]) // 2
    spy = (Y[sj] + Y[sj + 1]) // 2
    spz = Z[0] + SLAB_T // 2 + 32

    # Per-room props: crate stacks on the floor and one ceiling light. Both need
    # the room's interior rect, so invert cell -> room once (rooms are single-layer).
    lights = []
    ncrates = 0
    if lights_mode != "none" or crates_per_room > 0:
        room_cells = {}
        for k in range(nz):
            for (i, j), r in layers[k].items():
                room_cells.setdefault(r, (k, []))[1].append((i, j))
        nlit = 0
        for r in sorted(room_cells):
            k, cells = room_cells[r]
            i0 = min(c[0] for c in cells); i1 = max(c[0] for c in cells)
            j0 = min(c[1] for c in cells); j1 = max(c[1] for c in cells)
            x0i, x1i = X[i0] + WALL_T // 2, X[i1 + 1] - WALL_T // 2
            y0i, y1i = Y[j0] + WALL_T // 2, Y[j1 + 1] - WALL_T // 2
            zf = Z[k] + SLAB_T // 2                  # interior floor
            zc = Z[k + 1] - SLAB_T // 2              # interior ceiling
            cx = (X[i0] + X[i1 + 1]) // 2
            cy = (Y[j0] + Y[j1 + 1]) // 2

            # crate stacks (one wood texture per room so abutting stacks match)
            crate_tex = pick(CRATE_TEX, r)
            for _ in range(crates_per_room):
                before = len(brushes)
                emit_crate_stack(brushes, x0i, y0i, x1i, y1i, zf, zc, crate_tex, rng)
                ncrates += (len(brushes) > before)

            # light near the ceiling. Every Nth light is a spotlight aimed down --
            # spots are what cast crate shadows (world geo into the spot depth pass).
            if lights_mode != "none" and r % max(1, light_every) == 0:
                # hug the ceiling, above the tallest crate stack (which is capped
                # in emit_crate_stack at zc-32) so a centroid crate never engulfs it
                cz = zc - 24
                color = LIGHT_COLORS[r % len(LIGHT_COLORS)]
                spot = (spot_stride > 0 and nlit % spot_stride == 0)
                falloff = 1600 if spot else 1400
                # Point lights are the navigation/coverage lights in the overflow
                # blend (they omni-light every room and every crate face on all
                # atlas layers), so keep them bright enough to read clearly; spots
                # stay a touch brighter still since they also carry the cone.
                intensity = 220 if spot else 200
                # In 'mixed' mode each light is independently baked (static) or
                # runtime (dynamic); the four combos (static/dynamic x spot/point)
                # exercise the lightmap+SH bake AND the per-frame forward/shadow
                # path in one scene. Static spots/points bake crate shadows into
                # the lightmap; dynamic spots cast them at runtime.
                if lights_mode == "mixed":
                    this_mode = "static" if rng.random() < static_frac else "dynamic"
                else:
                    this_mode = lights_mode
                lights.append(light_entity(this_mode, (cx, cy, cz), color,
                                           falloff, intensity, spot, rng))
                nlit += 1

    return brushes, (spx, spy, spz), total_rooms, lights, ncrates


def write_map(path, brushes, spawn, nx, ny, nz, lights):
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
    for light in lights:
        n += 1
        lines.append(f"// entity {n}")
        lines.extend(light)
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


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
    PRESETS = {
        "overflow": dict(
            grid=[4, 4, 2], crates=10, lights="static", spot_frac=0.3,
            light_every=1, door_prob=0.15, shaft_prob=0.5,
        ),
    }

    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--preset", choices=sorted(PRESETS),
                    help="apply a named bundle of known-good knob values "
                         "(overridable per-flag on the CLI). 'overflow' => a map "
                         "whose bake overflows the lightmap atlas into >=2 array "
                         "layers at bounded memory; bake it at "
                         "--lightmap-density 0.08. Sets --grid 4 4 2 --crates 10 "
                         "--lights static --spot-frac 1.0.")
    ap.add_argument("--grid", nargs=3, type=int, default=None,
                    metavar=("NX", "NY", "NZ"),
                    help="cells along X, Y, and vertical layers (default 9 8 4, "
                         "which lands just under the 4096 BSP-leaf cap)")
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
                    help="add one light per room. 'dynamic' = light_dynamic "
                         "(runtime, no bake; stresses the per-frame forward "
                         "light loop + the 96-slot spot / 6-slot cube shadow "
                         "pools). 'static' = light (baked; stresses the lightmap "
                         "+ SH bake -- much slower compile). 'mixed' = a per-room "
                         "blend of both (see --static-frac), stressing the bake "
                         "AND the runtime path in one scene. (default none)")
    ap.add_argument("--static-frac", type=float, default=0.5,
                    help="in --lights mixed, fraction of lights that are baked "
                         "(static); the rest are dynamic. (default 0.5)")
    ap.add_argument("--light-every", type=int, default=None, metavar="N",
                    help="place a light in every Nth room (default 1 = all)")
    ap.add_argument("--crates", type=int, default=None, metavar="N",
                    help="crate stacks per room (solid box-brushes on the floor; "
                         "cast spot-light shadows and add to the geometry walk, "
                         "but each spends BSP leaves so room count must drop). "
                         "(default 0)")
    ap.add_argument("--spot-frac", type=float, default=None,
                    help="fraction of lights that are spotlights. Only spots "
                         "cast shadows from world geometry (crates), so raise "
                         "this to stress shadow-map rendering. (default 0.2)")
    args = ap.parse_args(argv)

    # Resolve each knob: explicit CLI value wins; else the preset's value (if a
    # preset was named and sets it); else the original committed default.
    preset = PRESETS.get(args.preset, {})
    _DEFAULTS = dict(grid=[9, 8, 4], door_prob=0.15, shaft_prob=0.5,
                     lights="none", light_every=1, crates=0, spot_frac=0.2)

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

    nx, ny, nz = args.grid
    if min(nx, ny, nz) < 1:
        ap.error("grid dimensions must be >= 1")

    # Envelope sanity check against the +/-16384-unit Quake bound.
    half_x = nx * PITCH_XY // 2
    half_y = ny * PITCH_XY // 2
    if max(half_x, half_y) > 16384:
        print(f"warning: grid spans +/-{max(half_x, half_y)} units, beyond the "
              f"classic +/-16384 envelope (still f32-exact, but unusually large)",
              file=sys.stderr)

    brushes, spawn, rooms, lights, ncrates = generate(
        nx, ny, nz, args.seed, args.door_prob, args.shaft_prob,
        args.lights, args.light_every, args.crates, args.spot_frac,
        args.static_frac)
    write_map(args.out, brushes, spawn, nx, ny, nz, lights)
    nspot = sum(1 for L in lights if "spot" in L[1])
    ndyn = sum(1 for L in lights if "dynamic" in L[1])
    nstat = len(lights) - ndyn
    print(f"grid {nx}x{ny}x{nz} = {nx*ny*nz} cells -> {rooms} rooms, "
          f"{len(brushes)} brushes ({ncrates} crates)")
    print(f"lights: {len(lights)} {args.lights} "
          f"({nstat} static, {ndyn} dynamic; {nspot} spot, {len(lights)-nspot} point)")
    print(f"extent: X/Y +/-{max(half_x, half_y)} u, Z {nz*PITCH_Z} u tall")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main(sys.argv[1:])
