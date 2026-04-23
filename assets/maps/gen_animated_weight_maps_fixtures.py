#!/usr/bin/env python3
"""Generate three fixture maps that exercise the animated-light weight-maps
path end-to-end.

Outputs:
  - test_animated_weight_maps_single.map: one sealed room, one animated light.
  - test_animated_weight_maps_occluded.map: one sealed room, one animated
    light above a parallel-plate blocker brush.
  - test_animated_weight_maps_cap.map: a room with enough overlapping animated
    lights that some texels hit MAX_ANIMATED_LIGHTS_PER_CHUNK (= 4).
  - test_animated_weight_maps_mixed.map: one non-animated static light FIRST
    followed by one animated light. Regression fixture for the chunk-list ↔
    descriptor-buffer namespace alignment: if the compiler emits light_index
    values in the `!is_dynamic` namespace (buggy) rather than the
    `!is_dynamic && animation.is_some()` namespace (correct), every texel
    references slot 0 — which is the static light, not a valid descriptor.

Usage:
  python3 gen_animated_weight_maps_fixtures.py

Brush face convention matches the other gen_* scripts in this directory:
an axis-aligned box is expressed as six planes with outward normals, using
three counter-clockwise vertices per plane.
"""

TEX = "concrete_pavement_036"
TEX_BLOCKER = "concrete_stone_022"


def box_brush(x1, y1, z1, x2, y2, z2, tex=TEX, comment=None):
    lines = []
    if comment:
        lines.append(f"// {comment}")
    lines.append("{")
    lines.append(f"( {x2} {y1} {z1} ) ( {x2} {y2} {z2} ) ( {x2} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y2} {z1} ) ( {x1} {y1} {z2} ) ( {x1} {y1} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x2} {y2} {z1} ) ( {x1} {y2} {z2} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y1} {z2} ) ( {x2} {y1} {z1} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y2} {z2} ) ( {x2} {y1} {z2} ) ( {x1} {y1} {z2} ) {tex} 0 0 0 1 1")
    lines.append(f"( {x1} {y1} {z1} ) ( {x2} {y2} {z1} ) ( {x1} {y2} {z1} ) {tex} 0 0 0 1 1")
    lines.append("}")
    return "\n".join(lines)


def sealed_room(ix1, iy1, iz1, ix2, iy2, iz2, wall=16):
    ox1, oy1, oz1 = ix1 - wall, iy1 - wall, iz1 - wall
    ox2, oy2, oz2 = ix2 + wall, iy2 + wall, iz2 + wall
    brushes = [
        box_brush(ox1, oy1, oz1, ox2, oy2, iz1, comment="floor"),
        box_brush(ox1, oy1, iz2, ox2, oy2, oz2, comment="ceiling"),
        box_brush(ox1, oy1, iz1, ix1, oy2, iz2, comment="west wall"),
        box_brush(ix2, oy1, iz1, ox2, oy2, iz2, comment="east wall"),
        box_brush(ix1, oy1, iz1, ix2, iy1, iz2, comment="south wall"),
        box_brush(ix1, iy2, iz1, ix2, oy2, iz2, comment="north wall"),
    ]
    return brushes


def light_entity(origin, intensity=200, style=2, phase=0.0,
                 color=None, start_inactive=False, fade=2048, cone=None):
    """style=2 is a slow strong pulse — exercises the Catmull-Rom sampler.
    Triangle brackets intentional only around "classname" etc.
    """
    out = ["{"]
    out.append('"classname" "light"')
    out.append(f'"origin" "{origin[0]} {origin[1]} {origin[2]}"')
    out.append(f'"light" "{intensity}"')
    out.append(f'"style" "{style}"')
    if phase:
        out.append(f'"_phase" "{phase}"')
    if color is not None:
        out.append(f'"_color" "{color[0]} {color[1]} {color[2]}"')
    if start_inactive:
        out.append('"_start_inactive" "1"')
    out.append(f'"_fade" "{fade}"')
    out.append("}")
    return "\n".join(out)


def info_player_start(origin, angle=0):
    out = ["{"]
    out.append('"classname" "info_player_start"')
    out.append(f'"origin" "{origin[0]} {origin[1]} {origin[2]}"')
    out.append(f'"angle" "{angle}"')
    out.append("}")
    return "\n".join(out)


def worldspawn(brushes):
    out = ["{"]
    out.append('"classname" "worldspawn"')
    out.append('"wad" ""')
    out.extend(brushes)
    out.append("}")
    return "\n".join(out)


def map_file(entities):
    """entities[0] must be worldspawn."""
    lines = [
        "// Game: Generic",
        "// Format: Standard",
    ]
    for i, ent in enumerate(entities):
        lines.append(f"// entity {i}")
        lines.append(ent)
    return "\n".join(lines) + "\n"


def write_single():
    # 256x256x128 interior, one animated light centered near the ceiling.
    brushes = sealed_room(0, 0, 0, 256, 256, 128)
    ent = [
        worldspawn(brushes),
        info_player_start((128, 128, 24), angle=0),
        # style=2 = slow strong pulse (period ≈ 5.0 s), exercises Catmull-Rom.
        light_entity((128, 128, 100), intensity=300, style=2),
    ]
    from pathlib import Path
    out_dir = Path(__file__).parent
    with open(out_dir / "test_animated_weight_maps_single.map", "w") as f:
        f.write(map_file(ent))


def write_occluded():
    # Same 256x256x128 room; blocker brush straddles the midline horizontally
    # between the light (z=112, near ceiling) and the floor (z=0). Blocker is
    # a thin plate at z=48..56 spanning most of the room.
    brushes = sealed_room(0, 0, 0, 256, 256, 128)
    # Parallel-plate blocker: 224x224 plate centered in the room, thickness 8.
    brushes.append(box_brush(16, 16, 48, 240, 240, 56, tex=TEX_BLOCKER,
                             comment="parallel-plate blocker"))
    ent = [
        worldspawn(brushes),
        info_player_start((128, 128, 24), angle=0),
        light_entity((128, 128, 112), intensity=300, style=2),
    ]
    from pathlib import Path
    out_dir = Path(__file__).parent
    with open(out_dir / "test_animated_weight_maps_occluded.map", "w") as f:
        f.write(map_file(ent))


def write_cap():
    # Cluster of four animated lights sharing overlap in a central region
    # of the ceiling. Cap is MAX_ANIMATED_LIGHTS_PER_CHUNK = 4; at 4 lights
    # every texel in the overlap region references exactly the cap, so the
    # `count ≤ cap` invariant is exercised without the chunk partitioner
    # bottoming out at the min-extent floor.
    # Small single-room box; four animated lights at the exact same point so
    # they produce one combined influence region and only one chunk on each
    # face (rather than partitioning into adjacent chunks that can touch
    # boundaries — the known UV-packer edge case). At cap=4 the one chunk
    # that covers the overlap carries exactly four light entries per texel.
    brushes = sealed_room(0, 0, 0, 128, 128, 96)
    lights = []
    # Four lights at the same origin — each with a distinct animation style
    # so descriptor slots are unique, but spatial footprint is identical.
    shared_origin = (64, 64, 80)
    styles = [1, 2, 3, 5]
    for i, st in enumerate(styles):
        lights.append(light_entity(shared_origin, intensity=80, style=st,
                                    phase=(i * 0.2) % 1.0, fade=256))
    ent = [
        worldspawn(brushes),
        info_player_start((64, 64, 24), angle=0),
    ]
    ent.extend(lights)
    from pathlib import Path
    out_dir = Path(__file__).parent
    with open(out_dir / "test_animated_weight_maps_cap.map", "w") as f:
        f.write(map_file(ent))


def write_mixed():
    # One non-animated static light FIRST, one animated light SECOND. In the
    # `!is_dynamic` namespace the animated light lives at index 1; in the
    # `!is_dynamic && animation.is_some()` namespace it lives at index 0.
    # The weight-map baker's emitted `light_index` values MUST land in the
    # animated-only namespace (the runtime descriptor-buffer namespace) or
    # the compose shader indexes the wrong descriptor at runtime.
    brushes = sealed_room(0, 0, 0, 256, 256, 128)
    ent = [
        worldspawn(brushes),
        info_player_start((128, 128, 24), angle=0),
        # Non-animated static light (style=0 → no animation). Listed first on
        # purpose: with the old buggy filter, the animated light would emit
        # light_index=1 and the runtime would read past the descriptor buffer.
        light_entity((64, 64, 100), intensity=200, style=0),
        # Animated light, style=2 (slow pulse). Listed second.
        light_entity((192, 192, 100), intensity=300, style=2),
    ]
    from pathlib import Path
    out_dir = Path(__file__).parent
    with open(out_dir / "test_animated_weight_maps_mixed.map", "w") as f:
        f.write(map_file(ent))


if __name__ == "__main__":
    write_single()
    write_occluded()
    write_cap()
    write_mixed()
    print("Wrote test_animated_weight_maps_{single,occluded,cap,mixed}.map")
