# camera-fallback-cell-bounds-overhang

Seed · not yet drafted. Run `/draft-session` before choosing a route. Split out of `shadow-fill-cost` on 2026-10-04.

## Observation
A `shadow-fill-cost` research finding: a BVH leaf's geometry can extend up to about 0.1 m past its cell's baked AABB. The camera's fallback paths cull whole cells by that AABB. So a leaf that pokes into the view frustum from a culled cell may never be drawn. If real, the result is a missing sliver of geometry at the screen edge on fallback frames, and `rendering_pipeline.md` §2's superset contract fails there.

## Mechanism (verified at 2a4bd9eb3; see `shadow-fill-cost/research.md` §Containment)
- Cell bounds come from the cell polytope with no padding: `brush_bsp.rs::make_leaf` → `leaf_bounds` → `RegionPolytope::vertex_aabb`. They pass unchanged into `CellData`.
- Faces are clipped by `geometry_utils::split_polygon` with `SPLIT_EPSILON = 0.1`. A vertex within 0.1 m of a splitter counts as on the plane and keeps its position. On an axis-aligned splitter, that puts the face outside its cell's AABB.
- No test asserts that a leaf lies inside its cell's bounds.

## Not verified
- Which camera paths cull cells by `CellData` bounds against the frustum. Candidates are the solid-cell, exterior, no-portals and step-limit fallbacks, and how the tree walk gates on the cell bit.
- Whether the portal walk can also drop such a leaf. A cell reached through a portal is drawn whole, so probably not.
- Whether a real map has an overhang at a pose where it shows. A sliver at most 0.1 m deep at the frustum edge may never be visible.

## Candidate fixes (hypothesis)
- Pad cell bounds by `SPLIT_EPSILON` at bake or load.
- Or cull fallback cells by the union of their leaves' AABBs, the same box `shadow-fill-cost` uses.
- Or project "on-plane" vertices onto the splitter in `split_polygon`. This changes geometry, so it is a bake change.

## Questions for the session
- Does a probe or test reproduce a missing leaf on a fallback path?
- Does any other consumer treat `CellData` bounds as containing their geometry, such as fog, SH or audio?
