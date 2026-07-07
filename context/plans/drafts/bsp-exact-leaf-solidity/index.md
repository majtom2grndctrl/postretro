# BSP Exact Leaf Solidity (Non-Cuboid Inner-Face Culling)

## Goal

Compile out shared inner faces between abutting non-cuboid brushes (wedges, prisms — any convex brush with non-axis-aligned planes), exactly as already happens for right-angle cuboids. Root cause is upstream of face extraction: BSP leaf solidity in `prl-build` is AABB-approximate, so non-axis-aligned brush interiors misclassify as empty and buried faces survive into rendered geometry.

## Root cause (confirmed by failing repro test)

`crates/level-compiler/src/partition/brush_bsp.rs` classifies regions by axis-aligned AABB:

- `brush_contains_region` requires the region **AABB** fully behind every brush plane. A wedge's diagonal plane always cuts any axis-aligned box around the wedge's own interior, so the wedge never enters the inside set.
- `tighten_region` shrinks the region AABB only for axis-aligned splitters; after a diagonal split, children inherit the parent AABB, so region AABBs never converge to the true leaf polytope.
- `build_recursive` therefore never reaches its solid verdict for wedge interiors; recursion exhausts splitters and the "no qualifying splitter → empty" fallback marks the leaf empty.
- `face_extract` Pass 1 keeps any side fragment that lands in an "empty" leaf — the shared face between abutting wedges renders.

Verified: two grid-snapped triangular prisms forming a 10×10×10 box split along the vertical plane x=z; `build_bsp_from_brushes` classifies both wedge interiors as **not** solid.

A second latent defect must be fixed together with the first: the solid verdict requires **all** candidate brushes to contain the region (`candidates.len() == inside.len()`). Candidate routing is brush-AABB-based, so a neighboring wedge whose AABB overlaps the region lingers as a candidate and blocks the verdict even with exact containment. The correct rule: a region is solid when **any** brush fully contains it.

## Scope

### In scope

- Exact region-vs-plane classification during BSP construction: carry the leaf region down the tree as a convex polytope, not an AABB.
- Exact `brush_contains_region` (region polytope fully behind every brush plane) and exact splitter qualification (plane strictly separates polytope vertices).
- Solidity rule change: solid iff at least one brush contains the region.
- Tightened leaf `bounds` derived from region polytope vertices (consumers tolerate this today; it narrows runtime cell AABBs and improves the fog-mask fast-reject).
- Regression tests: wedge solidity, wedge shared-face culling, wedge portal/leak behavior, preservation of all existing axis-aligned behavior.

### Out of scope

- Splitter *selection* quality and candidate routing — both stay brush-AABB-based (they affect tree shape/perf only, not correctness).
- dmap-style brush-fragment pushdown (alternative architecture; rejected — larger rewrite for the same observable result).
- Runtime changes. Portal traversal, cell locator, AABB-cull fallback are untouched; they consume the corrected compiler output as-is.
- Removing or altering the runtime per-cell AABB culling path (it is the engine's standing fallback, not part of this fix).
- Concave or invalid brush handling (brushes remain convex by construction from `.map` planes).
- Build-cache changes — parse/BSP/portals/geometry stages run uncached and stay uncached.

## Acceptance criteria

- [ ] Two abutting grid-snapped triangular-prism brushes sharing a rectangular face compile with zero faces on the shared plane, in both orientations (A's face against B and vice versa) — matching existing cuboid behavior.
- [ ] An interior point of a triangular-prism brush resolves to a solid BSP leaf; an interior point of a genuine air gap between two angled brushes resolves to an empty leaf.
- [ ] Two wedge brushes assembled into a cuboid emit exactly the six outer cuboid faces and nothing on the diagonal plane.
- [ ] No portal connects into a wedge-brush interior, and a sealed room whose shell includes wedge brushes does not leak (exterior flood-fill keeps its interior interior).
- [ ] All existing compiler tests pass unchanged in meaning: narrow air gaps stay empty, hollow-room interiors stay empty, abutting cuboids still cull shared faces, `campaign-test.map` partition guard still holds (`cargo test -p postretro-level-compiler`).
- [ ] Every leaf's `bounds` remains finite with `min ≤ max` and contains all of that leaf's emitted face vertices.
- [ ] `cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o <tmp>.prl` succeeds end-to-end.

## Tasks

### Task 1: Promote plane-winding helpers to `geometry_utils`

Move the "build a large bounded polygon on an arbitrary plane" helper (`make_base_winding` in `crates/level-compiler/src/portals.rs`, including its stable reference-axis basis pick) and an "iteratively clip a winding by a list of half-spaces" loop (the pattern inside `make_node_portal`) into `crates/level-compiler/src/geometry_utils.rs`, alongside the existing `split_polygon` / `clip_polygon_to_front`. Rewrite `portals.rs` to call the promoted versions. Behavior-preserving: all portal tests pass byte-for-byte; no signature changes elsewhere. This exists so the new polytope module (Task 2) can build facet windings without duplicating portal code.

### Task 2: Region polytope module

New module `crates/level-compiler/src/partition/region_polytope.rs` (registered in `partition.rs`'s module list) providing a convex-region type built from bounding half-spaces, using the Task 1 helpers from `geometry_utils`. Construction: seed from a world AABB as six quad facet windings; `clip(normal, distance)` returns the front and back child polytopes — each existing facet clipped by the half-space via `clip_polygon_to_front`/`split_polygon`, plus a new cap facet built by clipping the splitting plane's base winding against the polytope's other half-spaces. Queries over the facet vertex set: `all_vertices_behind(normal, distance, tol)` (on-plane counts as behind), `plane_spans(normal, distance, tol)` (vertices strictly beyond tolerance on both sides), and `vertex_aabb()`. Degenerate facets (< 3 vertices after clipping) drop silently; a polytope with no vertices reports non-spanning and not-behind. Unit tests: world-box seed round-trip, axial and diagonal clips, wedge assembly from a box (clip by a diagonal → verify vertex sets and both queries on each child), tolerance edge cases for on-plane vertices.

### Task 3: Rewire BSP solidity to exact polytope classification

In `crates/level-compiler/src/partition/brush_bsp.rs`: thread a region polytope (Task 2 type) through `build_recursive` alongside the existing region AABB — root seeded from `world_aabb_from_brushes`, children produced by `clip` on the chosen splitter (front child gets the front polytope, back child the back). Replace the AABB test in `brush_contains_region` with the exact polytope query (all region vertices behind every brush plane, mm-scale tolerance); replace the splitter-qualification test in `select_splitter` (`classify_aabb(region,…) == Spanning`) with `plane_spans` on the polytope. Change the solid verdict from `candidates.len() == inside.len()` to `!inside.is_empty()` — a region fully inside any one brush is solid; this may legitimately produce shallower trees for overlapping brushes. Leaf `bounds` becomes the polytope's `vertex_aabb()` intersected with the recursion's region AABB; `defining_planes` accumulation is unchanged (fog masks consume it). Candidate routing (`partition_candidates`, `count_partition`) and `tighten_region` stay AABB-based. Classification tolerance is a decision during implementation under two constraints: large enough to absorb clip accumulation error on grid-snapped diagonal planes, and far smaller than the 2-unit air gap in `adjacent_brushes_with_narrow_air_gap_preserve_air` (suggested starting point 1e-3, matching `COPLANAR_DISTANCE_EPSILON` scale — not the 0.1 `PLANE_EPSILON`). Add `brush_bsp` unit tests: wedge-interior-is-solid (two grid-snapped triangular prisms forming a box split on x=z; interior points of both wedges solid, points outside empty), wedge-with-air-gap stays empty. All existing `brush_bsp` tests pass.

### Task 4: Face-level and pipeline regression coverage

In `crates/level-compiler/src/partition/face_extract.rs` tests: add a `wedge_brush_with_sides` fixture helper (sibling of `box_brush_with_sides_per_face` with one diagonal side; winding normals must match the outward plane normals per the right-hand-rule convention documented on that helper). Add tests: (a) wedge sibling of `abutting_brushes_do_not_emit_shared_boundary_face` — two prisms abutting on a shared plane emit no face on that plane; (b) two wedges forming a cuboid emit exactly the six outer faces; (c) a wedge standing alone emits all five of its faces. In `crates/level-compiler/src/portals.rs` tests: a sealed configuration containing wedge brushes emits no portal whose either side is a wedge-interior leaf. Add a small wedge `.map` fixture under `content/dev/maps/` and a `fixture_pipeline.rs`-harness test asserting no emitted face lies on the shared internal plane after the full parse → partition → visibility → geometry run (the in-crate harness, not the `prl-build` shell-out).

## Sequencing

**Phase 1 (sequential):** Task 1 — Task 2 consumes the promoted winding helpers.
**Phase 2 (sequential):** Task 2 — Task 3 consumes the polytope API.
**Phase 3 (sequential):** Task 3 — the behavior change everything verifies.
**Phase 4 (sequential):** Task 4 — regression coverage over Task 3's output.

## Rough sketch

- Approach chosen over dmap-style brush-fragment pushdown: exact region polytopes localize the change to `brush_bsp.rs` + one new module; node structure, `face_extract`, `portals`, `visibility`, `pack` all stay untouched and consume corrected `is_solid`/`bounds` transparently.
- Soundness of the existing "mixed candidates, no qualifying splitter → empty" fallback is restored by exactness: with exact spanning, every brush plane either fails to span the region (region wholly on one side) or is an ancestor. A brush with the region behind all its planes lands in the inside set (→ solid); otherwise the region is outside at least one plane of every brush, hence genuinely air.
- Blast radius of leaves flipping empty→solid (all intended): their faces drop in `face_extract`, portals touching them are suppressed, exterior flood-fill loses them as traversal nodes, runtime cells gain the solid flag, light probes and alpha-light origins inside them are rejected. Full consumer map: `research.md`.
- `brush_bsp.rs` sits at ~725 lines; polytope machinery must land in the new `region_polytope.rs`, not inline, to keep it under the split threshold. `face_extract.rs` (~1000) and `portals.rs` (~1250) receive only test additions.
- Parse/BSP stages run uncached by design; polytope clipping adds O(facets × vertices) per node — negligible against bake stages, but keep facet counts bounded (each clip adds at most one cap facet).

## Open questions

- None blocking. Classification tolerance is pinned as a decision-during-implementation in Task 3 with explicit constraints.
