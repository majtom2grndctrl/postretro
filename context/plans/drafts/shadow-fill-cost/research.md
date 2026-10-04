# shadow-fill-cost — research

Read at 2c9a7ca2c. Findings that inform the brief and decide nothing on their own.

## Origin
This brief merges two drafts:
- `shadow-span-draws`, split out of `visible-span-draws` on 2026-10-03.
- `shadow-cone-cull-parallel-dispatch`, from the #311 audit. Its full text is in history: `git show 2a4bd9eb3:context/plans/drafts/shadow-cone-cull-parallel-dispatch/index.md`.

The first draft (2c9a7ca2c) restricted shadow draws to reached cells' spans, kept the GPU cull, and gated a flat per-leaf cull on measurement. `/validate-plan` returned *Reshape*: once draws are limited to reached cells, the GPU cull has almost nothing left to cull. The owner chose to make the CPU the only culler, with direct draws.

## When a slot draws world depth
- A warm slot skips the cull dispatch and the world draw. The skip comes from `should_dispatch_spot_cull` / `should_dispatch_cube_cull` on both frame plans, applied through the filter passed to `ShadowCullPipeline::dispatch_occupied_slots_filtered`.
- **Two caches.**
  - `DynamicDepthCache` holds 3 spot layers and 4 cube units for dynamic-tier lights. It is keyed on `{source_index, matrix_bits}`, so any change of pose or projection makes it cold.
  - `PromotedDepthCache` holds `MAX_PROMOTED_SPOT` 8 / `MAX_PROMOTED_CUBE` 2. It is keyed without a matrix, and when it is over capacity it drops the shadow for that frame.
- **Slots that draw world:**
  - an uncached live-pool slot (a dynamic light past cache capacity);
  - a dynamic cold fill (a new key: the light moved, or it was re-lit after its brightness fell below 0.01);
  - a promoted cold fill (on assignment).
- The six `draw_slot_indirect` sites in `renderer_dynamic_shadow_passes.rs` are these three cases, once each for spot and once for cube.
- A light holds a slot only if its influence reaches a portal-reachable cell (`update_dynamic_light_slots_with_capture_overrides`).

## Frequency in content
- **campaign-test:** 0 slots refill per frame. Its pulsing lights cold-fill once per pulse, which recorded runs show as 0.044–0.048 refreshes per frame.
- **stress-warren-hallway-inspection:** entity 825 is a `light_dynamic` point light (cube shadow) with `carrier` `warren_lift_0`.
  - The lift is a ping-pong `kinematic_mover`: speed 6, a 1,400 ms wait at each end, `start_on_spawn 1`, about 16 m of travel.
  - While the light holds a slot, every moving frame changes its cube key, so all six faces cold-fill.
  - Recorded spawn-pose runs saw no hallway refreshes, probably because the light fails the reach gate from spawn. The proof pose must be near the lift.
  - The map has 5,671 cells and 8,437 leaves.
- **kinematic-platform:** all three dynamic lights are on movers.
- **stress-warren-crates (37 dynamic lights) and -mini (21):** several regions refill per frame whenever more than 3 spot or 4 point lights pass the reach gate at once. Showcase has no local bake.
- **Runtime-spawned lights** cast no pool shadow (`rendering_pipeline.md` §4).

## Cost model
Measured: `[CpuTiming]` medians at `dcde8f292` (2026-10-01), before `visible-span-draws`, release, vsync on, warm shadow cache, spawn pose. The local logs are under `measurements/release-indirect-validation/runtime/`.

| stage (ms) | hallway | campaign |
|---|---|---|
| work | 7.46 | 7.14 |
| render_record | 1.09 | 1.23 |
| — rec_shadow_depth | 0.041 | 0.042 |
| — rec_cull | 0.029 | 0.036 |
| render_submit | 3.81 | 3.48 |

Extrapolated, not measured:
- A region draws whole `bucket_ranges`, and wgpu-hal Metal expands a multi-draw of N into N `drawIndexedPrimitives`. A region therefore costs L draws, and a cube light costs 6·L.
- The 49 ns per draw is 0.83 ms over 16,874 camera draws (`visible-span-draws/research.md` §Measurements). That puts the lift light at about 2.5 ms per moving frame, landing in `render_submit` (`CommandEncoder::finish` → `encode_render_pass`) rather than `rec_shadow_depth`.
- No uncached shadow fill has ever been measured.
- The GPU cull, `bvh_cull.wgsl` `cull_main` at `@workgroup_size(1)`, has never been timed.

## Containment
Cell bounds do not contain their leaves.
- Cell bounds come from the cell polytope: `brush_bsp.rs::make_leaf` → `leaf_bounds` → `RegionPolytope::vertex_aabb`. They have no padding and pass unchanged into `CellData`.
- Faces are clipped by `geometry_utils::split_polygon` with `SPLIT_EPSILON = 0.1`. A vertex within 0.1 m of a splitter counts as on the plane and keeps its position, so a face can extend up to 0.1 m past its cell's polytope.
- No test asserts that a leaf lies inside its cell. There are no orphan leaves: `every_drawable_leaf_covered_exactly_once`.
- A union of a cell's leaf AABBs contains every triangle in the cell, so a cell whose union misses the frustum contributes no depth.

## Index contiguity
- `geometry.rs::build_leaf_ordered_faces` emits faces in cell order, and `extract_geometry` appends indices in that order.
- `face_cut.rs::apply_face_cuts` rebuilds indices but keeps face order and `leaf_index`.
- `bvh_build::flatten` sorts only the leaf array, never the index buffer.
- So a cell's leaves span one contiguous index range, and consecutive cell ids abut.
- `leaf_face_ranges_are_contiguous` pins only face order inside `extract_geometry`. Nothing pins it across face cuts or at load.
- Derive each cell's range from `full.bvh_leaves` through `full.cell_draw_index` spans. Don't use `BspLeafRecord.face_start`: it predates face cuts.

## Shadow depth pipeline
- `full.shadow_depth_pipeline` (`spot_shadow.wgsl`) serves spot and cube.
- Its vertex input is a single `Float32x3` position, with `fragment: None` and one uniform bind group with a dynamic offset.
- `draw_slot_indirect` binds no material, and `visible_span_frame_tests.rs` asserts "depth-only shadow must not bind materials".
- Skinned and rigid occluders (`record_skinned_depth`, `record_kinematic_movers`) set their own pipeline and buffers, cull on the CPU against `cone_frustum_planes`, and share only the light-space bind group.

## Rivals
- **Keep the GPU cull and restrict its draws to reached spans** (the first draft). Rejected:
  - with about 1.5 leaves per cell on the hallway, the cull drops few leaves inside a reached cell;
  - a leaf it zeroes still costs a Metal draw;
  - it keeps the serial walk, about 22 MB of per-region buffers, a CPU/GPU agreement margin, and a stale-slot argument.
- **One install-time indirect buffer holding every leaf's record** (about 165 KB). It keeps shadows on the indirect path, at one draw per leaf rather than per run.
- **A per-region GPU candidate gather.** Rejected: new per-region GPU state.
- **Baked padded cell bounds.** Rejected: a format change for derivable data.

## Deletion
- **Memory:** a region is `total_leaves × 20` bytes rounded up to 256. On the hallway that is 168,960 B × 132 regions ≈ 22.3 MB, plus two status scratch buffers, two all-ones visible-cell buffers, and per-region uniforms.
- **Users of `ShadowCullPipeline`:** construction in `renderer_full_init.rs` and `renderer_resources.rs`, the fields in `renderer_types.rs`, `lib.rs` `mod shadow_cull`, `visible_span_frame_tests.rs`, and the uniform-buffer entry in `renderer_tests/inventory.rs`. Nothing in wireframe, cull-status, capture or diagnostics uses it.
- **Accessors left with no callers:** `ComputeCullPipeline::node_buffer`, `bucket_ranges` and `has_multi_draw_indirect`.
- **Counters that change meaning:**
  - `should_dispatch_{spot,cube}_cull` and `skipped_*_cull_dispatches` in both cache plans;
  - the `cull_dispatch_skips` log counter;
  - `promoted_depth_cache_cull_dispatch_skips`, whose accessor has no callers.
- **Stale comments:** `lighting/src/lib.rs`, `cube_shadow.rs`, `renderer_light_slots.rs`, `renderer_render_frame.rs`, `compute_cull.rs`. Docs: `rendering_pipeline.md` §7.1 step 6.
- **`indirect_contract_tests.rs`:**
  - Remove these shadow pieces: the `INDIRECT` owner `ShadowCullPipeline::new` and its constructor sites; the `draw_slot_indirect` owner and match arm; the reference and import bans; the inventory entries (`INDIRECT` count 2 → 1, `ShadowCullPipeline::new` 4, `draw_slot_indirect` 6); the required install and boot strings.
  - Retarget the nested-binding fixtures to the camera `draw_indirect`.
  - The scanner records only names containing "indirect", so direct draws need no rule.

## Headless capture
- `--capture` skips every `is_dynamic` light (`capture_static_lights_and_shadow_selection`; test `capture_lights_remain_static_only_and_remap_shadow_selection`).
- Movers stay at their spawn pose.
- So no existing capture shows a dynamic-light shadow.
