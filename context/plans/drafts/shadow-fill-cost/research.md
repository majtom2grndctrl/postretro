# shadow-fill-cost — research

Read at 2a4bd9eb3. Findings that inform the brief and decide nothing on their own.

## Origin
This brief merges two drafts:
- `shadow-span-draws`, split out of `visible-span-draws` on 2026-10-03.
- `shadow-cone-cull-parallel-dispatch`, from the #311 BVH/culling audit (2026-07-24).

The second draft's task list survives in history: `git show 2a4bd9eb3:context/plans/drafts/shadow-cone-cull-parallel-dispatch/index.md`. Its stale facts:
- It named the struct `ShadowCull`; the struct is `ShadowCullPipeline`.
- It gated on `stress-warren-lit`, which has no dynamic lights.
- Its proof needed GPU timestamps, which the owner's Mac lacks.

## When a slot draws world depth
- A warm slot skips the cull dispatch and the world draw. The skip comes from `should_dispatch_spot_cull` / `should_dispatch_cube_cull` on both frame plans, applied through the filter passed to `ShadowCullPipeline::dispatch_occupied_slots_filtered`.
- **Two caches.**
  - `DynamicDepthCache` holds 3 spot layers and 4 cube units for dynamic-tier lights. It is keyed on `{source_index, matrix_bits}`, so any change of pose or projection makes it cold.
  - `PromotedDepthCache` holds `MAX_PROMOTED_SPOT` 8 / `MAX_PROMOTED_CUBE` 2. It is keyed without a matrix, and when it is over capacity it drops the shadow for that frame.
- **Slots that draw world:**
  - an uncached live-pool slot (a dynamic light past cache capacity);
  - a dynamic cold fill (a new key: the light moved, or it was re-lit after its brightness fell below 0.01);
  - a promoted cold fill (on assignment).
- The six `draw_slot_indirect` call sites in `renderer_dynamic_shadow_passes.rs` are these three cases, once each for spot and once for cube.
- Both frame plans are set in `update_dynamic_light_slots_with_capture_overrides`, before `record_spot_shadow_depth` / `record_cube_shadow_depth` run.

## Frequency in content
- **campaign-test:** 0 slots refill per frame. Its `arena-lights.ts` pulses lights to 0, so each one cold-fills once per pulse.
- **stress-warren-hallway-inspection:** its only dynamic light (entity 825) is a `light_dynamic` point light (cube shadow) with `carrier` `warren_lift_0`.
  - The lift is a ping-pong `kinematic_mover`: speed 6, a 1,400 ms wait at each end, `start_on_spawn 1`, about 16 m of travel.
  - On every moving frame the cube key changes, so all six faces cold-fill.
  - The map has 5,671 cells and L = 8,437 leaves (`spatial-residency--lightmap-cell-blocks/findings.md`, `bvh-leaf-clustering/research.md`).
- **kinematic-platform:** all three dynamic lights are on movers.
- **stress-warren-crates (37 dynamic lights), -mini (21) and -showcase (45):** several regions refill per frame whenever more than 3 spot or 4 point lights pass the reachability gate at once. How often depends on the pose and was not measured. Showcase has no local bake.
- **Runtime-spawned lights** (projectiles, impact flashes, the enemy rifle) cast no pool shadow (`rendering_pipeline.md` §4).

## Cost model (extrapolated, not measured)
- `draw_slot_indirect` passes whole `bucket_ranges`. wgpu-hal Metal expands a multi-draw of N into N `drawIndexedPrimitives`, so a region costs L driver draws and a cube light costs 6·L.
- The 49 ns per draw is 0.83 ms over 16,874 camera draws (`visible-span-draws/research.md` §Measurements). The hallway lift light comes out at 6 × 8,437 × 49 ns ≈ 2.5 ms per moving frame.
- The draw cost lands in `render_submit` (`CommandEncoder::finish` → `encode_render_pass`), not in `rec_shadow_depth`.
- **GPU cull.** `bvh_cull.wgsl` `cull_main` runs at `@workgroup_size(1)`, one dispatch per region, in one compute pass labelled "Shadow Cull Pass" with no timestamps. It has never been measured. `build_frame_timing` has no entry for it.

## Containment
Cell bounds do not contain their leaves.
- Cell bounds come from the cell polytope: `brush_bsp.rs::make_leaf` → `leaf_bounds` → `RegionPolytope::vertex_aabb`. They have no padding and pass unchanged into `CellData`.
- Faces are clipped down the tree by `geometry_utils::split_polygon` with `SPLIT_EPSILON = 0.1`. A vertex within 0.1 m of a splitter counts as on the plane and stays at its original position, so a face can extend up to 0.1 m past its cell's polytope. On an axis-aligned splitter that puts it outside the cell's AABB.
- No test asserts leaf ⊆ cell.
- There are no orphan leaves: `every_drawable_leaf_covered_exactly_once`.
- A per-cell union of leaf AABBs is the exact box the GPU cull tests leaf by leaf.

## Plane and test identity
- `extract_frustum_planes_for_gpu` (`render-data/src/cone_frustum.rs`) builds the per-region uniform. `cone_frustum_planes` delegates to it.
- `aabb_intersects_frustum` mirrors the WGSL `is_aabb_outside_frustum` (positive vertex, `< 0.0`, no epsilon). Its callers are `instance_casts_into_cone` and `mover_occluders_in_cone`.
- The test is monotone in the box, so a union box that fails a plane means every leaf inside it fails that plane too.
- The one gap is rounding: the GPU may contract the plane dot into a fused multiply-add. A small margin on the CPU side covers it.

## Stale slots
- On rejecting an inner node, the tree walk leaves the slots of the leaves beneath it untouched, and they keep an earlier value. It zeroes only a rejected leaf node.
- `draw_slot_indirect` documents why that is safe: those leaves lie outside the region's frustum and clip out against its projection.
- Span draws keep that argument. A stale slot inside a drawn span is still a leaf outside this frame's frustum.

## Rivals
- **Per-slot candidate gather on the GPU.** Reuse `CandidateCullPipeline` per region, using the reach set as candidates. Rejected because it needs per-region uniform, candidate and params buffers, per-region bind groups with offsets, and a mandatory region clear. That is new persistent GPU state, bought to save arithmetic a flat dispatch over about 8.4k leaves (about 132 workgroups) does cheaply.
- **Baked padded cell bounds.** Rejected: a format change for data the load can derive.
- **Bridging gaps between spans.** Rejected for the same reason `visible-span-draws` rejected it.

## Headless capture
- `--capture` skips every `is_dynamic` light (`capture_static_lights_and_shadow_selection`; regression test `capture_lights_remain_static_only_and_remap_shadow_selection`).
- Movers stay at their spawn pose.
- So no existing capture shows a dynamic-light shadow, and there is no byte-identical before/after for this brief.

## Adjacent and not verified
- The camera's fallback paths (solid-cell, exterior, no-portals, step-limit) cull cells by `CellData` AABB against the frustum.
- Given §Containment, a leaf extending up to 0.1 m past a culled cell's AABB could be missing at the frustum edge.
- If that is real, it predates span draws, because the tree walk already gated on the cell bit.
