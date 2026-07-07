# Research Notes — BSP Exact Leaf Solidity

Investigation record backing `index.md`. Line numbers valid as of drafting (2026-07-07); treat as hints, not contracts.

## Repro

Temporary in-crate test (reverted after confirmation): two grid-snapped triangular prisms forming a 10×10×10 box split on the vertical plane x=z. Wedge A planes: +X @ 10, −Z @ 0, ±Y, diagonal (−1/√2, 0, 1/√2) @ 0. Wedge B mirrors it. `build_bsp_from_brushes` → `find_leaf_for_point(7,5,3)` (wedge A interior) landed in an **empty** leaf; assertion `is_solid` failed. Confirms the AABB-classification root cause without any face-extraction involvement.

## Failure chain

1. `brush_contains_region` (`brush_bsp.rs:133`) — region AABB must be fully `Back` of every brush plane; a diagonal brush plane classifies `Spanning` against any AABB around the wedge interior, so the wedge never enters its own region's inside set.
2. `tighten_region` (`brush_bsp.rs:266`) — axis-aligned planes only; diagonal splits pass the parent AABB through unchanged, so region AABBs never converge.
3. `build_recursive` (`brush_bsp.rs:341-350`) — solid verdict `candidates.len() == inside.len()` never fires for wedge interiors; splitter pool drains via ancestor dedup; "mixed candidates, no qualifying splitter → empty" fallback fires instead.
4. `face_extract.rs` Pass 1 (`clip_side_by_tree`) keeps fragments landing in empty leaves — buried shared faces emit and render.

Second defect (found during analysis, later covered by candidate-routing regression tests): even with exact containment, the all-candidates solid rule fails when a neighboring brush's AABB overlaps the region, and non-conservative routing can drop an owner/touching brush from the child it fully contains. Rule must be "any brush contains region → solid" — which is geometrically correct — and candidate routing must keep near-plane-only and genuine straddles conservative.

## BSP consumer map (blast radius)

`BspNode` planes/children: read by `find_leaf_for_point` (`partition/bsp.rs:27`), portal winding build (`portals.rs:52-172`), `encode_nodes` (`visibility/mod.rs:164`), `encode_cell_locator` (`pack.rs:504`), `face_extract` plane routing. Fix does not alter node planes for a given splitter choice; exact spanning qualification can change which planes qualify (tree shape), which every consumer recomputes per build. `BspNode.parent` is written but never read.

`BspLeaf.is_solid` readers — all inherit the corrected classification, all intended:
- `face_extract.rs:210,295` — faces only into empty leaves (the fix's target).
- `geometry.rs:399` — solid leaves skipped for vertex/index emission.
- `portals.rs:135` — portal emitted iff both adjacent leaves non-solid → portal count drops for newly solid leaves.
- `visibility/mod.rs:69,93,108,121` — exterior flood-fill seed guard + BFS; newly solid leaves leave the portal graph.
- `fog_cell_masks.rs:53`, `cell_draw_index_bake.rs:117,153`, `pack.rs:421,433` — runtime cell flags/drawability.
- Probe/light validity: `affinity_grid.rs:207,269`, `chunk_light_list_bake.rs:168,219`, `sdf_bake.rs:478`, `sh_bake.rs:429`, `direct_sh_bake.rs`, `pack.rs:85` — probes and light origins inside wedges now correctly rejected.

`BspLeaf.bounds` readers and looseness tolerance:
- `fog_cell_masks.rs:65` — stage-1 fast reject; loose bounds cause false positives only, stage-2 `defining_planes` test is exact. Tighter bounds = pure perf win.
- `visibility/mod.rs:50-57` — map AABB for the exterior probe; looser only pushes the probe out.
- `visibility/mod.rs:195-217` → `pack.rs:448-485` — the one place bounds ship (runtime `CellRecord`); validation is finiteness + `min ≤ max` only, so tightening is safe.
- Runtime cell locator does **not** read bounds — it descends node planes (`pack.rs:491-518`).

`BspLeaf.defining_planes`: sole real consumer `fog_cell_masks.rs:72-75` (exact convex-region test). Already the inward half-space stack the polytope needs; accumulation unchanged by the fix.

## Reusable helpers

- `geometry_utils.rs` (166 lines): `split_polygon`, `clip_polygon_to_front` — the clip primitives.
- `portals.rs:100-121` `make_base_winding` — large quad on an arbitrary plane, stable basis; `make_node_portal` (`:77-97`) — clip a base winding against a plane stack. Private to portals; promote to `geometry_utils` (spec Task 1).
- No existing whole-polytope-from-planes builder; assembly is new (spec Task 2).

## Test landscape

- No existing test or fixture exercises angled world brushes through partition/face-extract. Only non-axial brush anywhere: a 15-sided prism in `parse.rs:2086` exercising fog-volume plane budget.
- Axis-aligned invariants to preserve: `adjacent_brushes_with_narrow_air_gap_preserve_air` (`brush_bsp.rs:659`), `room_with_doorway_has_connected_air` (`:604`), `hollow_room_interior_air_exterior_solid` (`:556`), `abutting_brushes_do_not_emit_shared_boundary_face` (`face_extract.rs:814`), sealed-box flood-fill (`visibility/mod.rs:611-636`), `partition_with_test_map` (`partition.rs:322`), `floating_cube_near_ceiling_faces_survive_pipeline` (`portals.rs:840`).
- End-to-end vehicle: in-crate `fixture_pipeline.rs` harness (`load_fixture` at `:74`) runs parse → partition → visibility → geometry on `content/dev/maps/` fixtures without shelling out to the binary; the `tests/` shell-out integration test is `#[ignore]` and unsuitable.
- File sizes: `brush_bsp.rs` 725, `face_extract.rs` 998, `portals.rs` 1255, `geometry.rs` 1269, `geometry_utils.rs` 166, `partition/types.rs` 145.

## Epsilon notes

Existing constants: `PLANE_EPSILON = 0.1` (AABB corner classification), `SPLIT_EPSILON = 0.1` (polygon splitting), `COPLANAR_DISTANCE_EPSILON = 1e-3` (face dedup), ancestor dedup `1e-4`. Polytope vertices are exact plane intersections (unlike AABB support corners), so containment/spanning tolerance can and should be mm-scale (~1e-3), not 0.1 — 0.1 m would risk swallowing thin authored gaps. Grid-snapped diagonal planes carry only ~1e-12 normalization error; clip accumulation stays far below 1e-3 at map scale.
