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
omitted (the cells fuse into one air volume); walls between different rooms get
a centered doorway, producing real portals. Sparse vertical shafts punch holes
through interior slabs so the stacked layers are portal-connected.

All coordinates are emitted in Quake units (Z-up); prl-build applies the
1 unit = 0.0254 m scale and the Z-up -> Y-up swizzle.

Usage
-----
    python3 tools/gen_stress_map.py            # committed default, fits the cap
    python3 tools/gen_stress_map.py --grid 8 8 4 --door-prob 0.2

Then compile with a COARSE SH probe spacing (the SH irradiance volume bakes a
probe grid over the whole world AABB regardless of lights; at the default 1.0 m
spacing a map this large would bake millions of probes and gigabytes):

    prl-build content/dev/maps/stress-warren.map \\
        -o content/dev/maps/stress-warren.prl --sh-probe-spacing 10.0 --no-cache

Push the room count up by enlarging the grid and/or lowering --door-prob, and
watch that the compile stays under the 4096 BSP-leaf cap (see below).
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

TEX_WALL = "debug_wall_grey"
TEX_FLOOR = "debug_floor_grey"
TEX_CEIL = "debug_ceiling_grey"


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


def wall_box(brushes, x0, y0, x1, y1, zf, zc):
    """Solid wall slab spanning [x0,x1]x[y0,y1] over interior height [zf,zc]."""
    if x1 - x0 < 1 or y1 - y0 < 1:
        return
    brushes.append(box_brush(x0, y0, zf, x1, y1, zc, TEX_WALL, TEX_WALL, TEX_WALL))


def emit_wall(brushes, axis, line, lo, hi, zf, zc, doored):
    """Emit a wall on an interior/edge boundary, optionally with a centered door.

    axis 'x': wall lies on plane X=line, spanning Y in [lo,hi].
    axis 'y': wall lies on plane Y=line, spanning X in [lo,hi].
    The wall is WALL_T thick, centered on `line`. Interior height [zf,zc].
    """
    h = WALL_T // 2
    if not doored:
        if axis == "x":
            wall_box(brushes, line - h, lo, line + h, hi, zf, zc)
        else:
            wall_box(brushes, lo, line - h, hi, line + h, zf, zc)
        return

    # Centered full-thickness doorway: split into two jambs + a lintel.
    mid = (lo + hi) // 2
    d0, d1 = mid - DOOR_W // 2, mid + DOOR_W // 2
    ztop = zf + DOOR_H
    if axis == "x":
        wall_box(brushes, line - h, lo, line + h, d0, zf, zc)       # jamb low
        wall_box(brushes, line - h, d1, line + h, hi, zf, zc)       # jamb high
        wall_box(brushes, line - h, d0, line + h, d1, ztop, zc)     # lintel
    else:
        wall_box(brushes, lo, line - h, d0, line + h, zf, zc)
        wall_box(brushes, d1, line - h, hi, line + h, zf, zc)
        wall_box(brushes, d0, line - h, d1, line + h, ztop, zc)


def emit_slab(brushes, x0, y0, x1, y1, zc, holed):
    """Horizontal slab centered on Z=zc over footprint [x0,x1]x[y0,y1].

    When `holed`, a centered square shaft is carved (slab split into 4 rims)
    to portal-connect the room below to the room above.
    """
    h = SLAB_T // 2
    z0, z1 = zc - h, zc + h
    if not holed:
        brushes.append(box_brush(x0, y0, z0, x1, y1, z1, TEX_FLOOR, TEX_FLOOR, TEX_CEIL))
        return
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    a0, a1 = cx - SHAFT // 2, cx + SHAFT // 2
    b0, b1 = cy - SHAFT // 2, cy + SHAFT // 2
    # four rims around the hole
    brushes.append(box_brush(x0, y0, z0, x1, b0, z1, TEX_FLOOR, TEX_FLOOR, TEX_CEIL))
    brushes.append(box_brush(x0, b1, z0, x1, y1, z1, TEX_FLOOR, TEX_FLOOR, TEX_CEIL))
    brushes.append(box_brush(x0, b0, z0, a0, b1, z1, TEX_FLOOR, TEX_FLOOR, TEX_CEIL))
    brushes.append(box_brush(a1, b0, z0, x1, b1, z1, TEX_FLOOR, TEX_FLOOR, TEX_CEIL))


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


def generate(nx, ny, nz, seed, door_prob, shaft_prob):
    rng = random.Random(seed)
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

    brushes = []

    # Vertical walls. For each cell, emit its low-X and low-Y boundary, plus the
    # far edges. Interior boundaries between two cells of the same room are open.
    for k in range(nz):
        zf = Z[k] + SLAB_T // 2
        zc = Z[k + 1] - SLAB_T // 2
        for j in range(ny):
            for i in range(nx):
                r = room_of(i, j, k)
                # X-boundary at X[i] (between cell i-1 and i)
                if i == 0:
                    emit_wall(brushes, "x", X[0], Y[j], Y[j + 1], zf, zc, False)
                elif room_of(i - 1, j, k) != r:
                    emit_wall(brushes, "x", X[i], Y[j], Y[j + 1], zf, zc,
                              rng.random() < door_prob)
                if i == nx - 1:
                    emit_wall(brushes, "x", X[nx], Y[j], Y[j + 1], zf, zc, False)
                # Y-boundary at Y[j]
                if j == 0:
                    emit_wall(brushes, "y", Y[0], X[i], X[i + 1], zf, zc, False)
                elif room_of(i, j - 1, k) != r:
                    emit_wall(brushes, "y", Y[j], X[i], X[i + 1], zf, zc,
                              rng.random() < door_prob)
                if j == ny - 1:
                    emit_wall(brushes, "y", Y[ny], X[i], X[i + 1], zf, zc, False)

    # Horizontal slabs at every Z-boundary, full cell footprint. Top and bottom
    # boundaries (k==0, k==nz) are always solid (seal). Interior boundaries get a
    # sparse shaft so layers are portal-connected.
    for k in range(nz + 1):
        for j in range(ny):
            for i in range(nx):
                interior = 0 < k < nz
                holed = (interior and (i % 3 == 1) and (j % 3 == 1)
                         and rng.random() < shaft_prob)
                emit_slab(brushes, X[i], Y[j], X[i + 1], Y[j + 1], Z[k], holed)

    # Player spawn: interior of cell (min(1,nx-1), min(1,ny-1), 0).
    si, sj = min(1, nx - 1), min(1, ny - 1)
    spx = (X[si] + X[si + 1]) // 2
    spy = (Y[sj] + Y[sj + 1]) // 2
    spz = Z[0] + SLAB_T // 2 + 32

    return brushes, (spx, spy, spz), total_rooms


def write_map(path, brushes, spawn, nx, ny, nz):
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
    lines.append("// entity 1")
    lines.append("{")
    lines.append('"classname" "player_spawn"')
    lines.append(f'"origin" "{spawn[0]} {spawn[1]} {spawn[2]}"')
    lines.append('"angle" "0"')
    lines.append("}")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--grid", nargs=3, type=int, default=[9, 8, 4],
                    metavar=("NX", "NY", "NZ"),
                    help="cells along X, Y, and vertical layers (default 9 8 4, "
                         "which lands just under the 4096 BSP-leaf cap)")
    ap.add_argument("-o", "--out", default="content/dev/maps/stress-warren.map")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--door-prob", type=float, default=0.35,
                    help="fraction of inter-room walls that get a doorway "
                         "(more doors = more portals but more BSP leaves; "
                         "default 0.5)")
    ap.add_argument("--shaft-prob", type=float, default=0.5,
                    help="fraction of candidate interior slabs that get a "
                         "vertical shaft connecting layers (default 1.0)")
    args = ap.parse_args(argv)

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

    brushes, spawn, rooms = generate(nx, ny, nz, args.seed,
                                     args.door_prob, args.shaft_prob)
    write_map(args.out, brushes, spawn, nx, ny, nz)
    print(f"grid {nx}x{ny}x{nz} = {nx*ny*nz} cells -> {rooms} rooms, "
          f"{len(brushes)} brushes")
    print(f"extent: X/Y +/-{max(half_x, half_y)} u, Z {nz*PITCH_Z} u tall")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main(sys.argv[1:])
