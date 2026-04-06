# PVS Visibility Threshold — Alternatives to Explore

Current approach: `MIN_VISIBILITY_RATIO = 0.03` — a flat percentage of sample rays that must be unblocked to mark a cluster pair as visible. Filters degenerate "lucky ray" sightlines but is a magic number with no geometric basis.

## Why it's imprecise

Portal-based systems (Quake, Doom 3) solve this geometrically: a portal polygon is clipped through a chain of other portals, and visibility is determined by whether the clipped polygon has nonzero area. Our ratio is a probabilistic approximation of portal area.

## Alternatives worth investigating

1. **Absolute ray count.** "At least N unblocked rays" instead of "N% of rays." More stable because total ray count varies with cluster size and sample density. Simple to implement.

2. **Solid angle / subtended angle.** How large does the target cluster appear from the source? A cluster subtending < X steradians isn't meaningfully visible. Captures the geometric intuition that distant small openings don't matter.

3. **Spatial coherence of unblocked rays.** Are successful rays spread across the opening, or all threading the same narrow gap? A 10% hit rate through a 4-unit gap is less meaningful than 10% through a 64-unit opening.

4. **Distance attenuation.** Scale the threshold by distance between clusters. A 3% hit rate at 100 units is more meaningful than 3% at 2000 units. Portal systems get this naturally (distant portals subtend less area).

## Context

- Current implementation: `postretro-level-compiler/src/visibility/pvs.rs`, `MIN_VISIBILITY_RATIO` const
- Research on how other engines handle visibility: `context/reference/other-engine-research.md`
