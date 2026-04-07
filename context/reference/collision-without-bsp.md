# Collision Detection Without BSP

Notes from visibility system research (2026-04-06). Context for Phase 7 (grounded player movement).

## Brush volumes are the collision primitive

The .map file's brush volumes are convex hulls defined by half-planes — the same data the voxelizer already consumes. No BSP tree is needed for the actual intersection math:

- **Point-in-brush:** dot product against each half-plane. Already implemented in `voxel_grid.rs` (`point_inside_brush`).
- **Ray-vs-brush:** slab method (clip ray parameter against each half-plane). Already implemented in `pvs.rs` (`ray_vs_brush`, currently dead code).
- **Swept AABB vs brush:** expand each half-plane by the player's AABB extents (Minkowski sum), then ray-test the expanded brush. This is how Quake handles player movement — it operates on brush half-planes, not BSP nodes.

## BSP is an acceleration structure, not a collision algorithm

Quake's BSP tree finds *which brushes to test* (walk tree to leaf, test brushes in leaf). The collision math itself is brush half-plane tests. Any spatial index that answers "which brushes are near this point?" works.

## Acceleration structure options

| Option | Pros | Cons |
|--------|------|------|
| **Spatial hash** | Simple, O(1) lookup, easy to build at load time | Uniform cell size, wastes memory in sparse areas |
| **BVH** | Adapts to geometry density, good for mixed indoor/outdoor | More complex to build, tree traversal overhead |
| **Spatial grid (reuse existing)** | Already have the infrastructure | Uniform grid has the same alignment issues we saw with visibility cells |
| **Flat scan** | Zero overhead, trivial to implement | O(n) per query — fine at hundreds of brushes, bad at thousands |

At our expected scale (hundreds of brushes per level), a flat scan with AABB pre-filter may be sufficient. Profile before optimizing.

## Storage in .prl

Store brush volumes (half-plane lists) in a dedicated .prl section. Build the spatial index at load time from the brush data. This is analogous to Quake's BRUSHLIST BSPX lump — raw brush hulls stored alongside the level, engine builds its own acceleration structure.

## What we already have

- Brush volumes parsed from .map files (`map_data.rs`, `BrushVolume` / `BrushPlane` types)
- Point-in-brush and ray-vs-brush implementations (compile-time, but the math is identical for runtime)
- Voxel grid for compile-time solid/empty queries (not useful at runtime unless shipped in .prl)

## Decision deferred

The acceleration structure choice depends on actual brush counts and level complexity at Phase 7. These notes capture the options and the fact that BSP is not required.
