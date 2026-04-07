# CSG Face Clipping (Without Full BSP)

Notes from visibility system research (2026-04-06). Relevant when implementing textured rendering (Phase 3).

## The problem

Overlapping brushes produce faces inside solid space. In wireframe rendering this is invisible. In textured rendering it causes z-fighting — two surfaces at the same depth flicker against each other.

## CSG clipping vs full BSP

Full BSP builds a tree from brushes, recursively partitioning space and splitting faces along every plane. CSG face clipping is simpler: for each face, clip it against other brushes to remove the portion inside solid space.

### Algorithm (per face)

1. Start with the face polygon.
2. For each other brush, test if the face overlaps the brush (AABB pre-filter).
3. Clip the polygon against the brush's half-planes (Sutherland-Hodgman).
4. If fully inside the brush, discard the face entirely.
5. If partially inside, keep only the portion outside.

### Complexity

O(faces × overlapping_brushes × planes_per_brush). Compile-time only. AABB pre-filter makes this practical — most face/brush pairs don't overlap.

## Why this matters for exterior scenes

City-block levels with dense buildings have many interior faces (walls facing into sealed building interiors). CSG clipping removes these at compile time, significantly reducing face count. This is geometry optimization independent of visibility.

## When to implement

Before Phase 3 (textured world). Z-fighting on overlapping surfaces will be visible and distracting with textures. Not needed for wireframe rendering.

## What we have

- Brush volumes with half-planes (parsed from .map)
- Sutherland-Hodgman polygon clipping is a well-documented algorithm
- The voxel grid could provide a coarser alternative: test face vertices/centroid against solid voxels, discard faces entirely inside solid. Less precise than geometric clipping but trivial to implement with existing infrastructure.
