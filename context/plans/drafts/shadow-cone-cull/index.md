# Shadow Cone Cull

## Goal

Stop submitting full world geometry to every spotlight shadow-depth pass, and stop ranking shadow lights by a camera-in-sphere test that pretends to be a frustum check. Drive both with the spotlight's cone frustum — the same 6-plane representation the BVH cull pass already consumes. Shadow-map cost should scale with what each cone actually covers, not with total scene index count.

## Scope

### In scope
- Per-slot cone-frustum culling for the spot-shadow depth pass. Each occupied shadow slot renders only world leaves inside that light's cone, via the existing GPU BVH traversal + indirect-draw machinery, instead of one unconditional `draw_indexed(0..index_count)`.
- Replace the sphere-based pre-filter in shadow light ranking (`rank_lights`) with a cone-frustum test built from each light's light-space view-projection matrix.
- Reuse `light_space_matrix()` as the single source of the cone frustum — for both the rank-time candidate test and the per-slot GPU cull frustum.

### Out of scope
- Entity / skinned-mesh culling in the shadow pass. The mesh milestone is early; shadow culling here covers world geometry only. Moving occluders cast no shadows yet (`spot_shadow.rs` already renders world geometry only).
- Per-light precomputed PVS or baked cone-cell sets. Culling stays per-frame.
- Changing shadow-map resolution, pool size, depth format, or slot allocation policy beyond the rank-time eligibility test.
- Soft shadows / penumbra quality. Depth content and sampling are unchanged.
- Parallelizing the BVH traversal compute shader (still one invocation per dispatch).

## Acceptance criteria
- [ ] On a map with a spotlight whose cone covers a small fraction of the level, the shadow-depth pass for that light submits fewer indices than the full scene index count. Verified by a deterministic CPU-side submitted-index counter (the count of indices the cone cull marks visible for that slot), not GPU timing. GPU timing is noisy, hardware-dependent, and gated on `TIMESTAMP_QUERY` adapter support; it may stay as optional observability but is not the gate.
- [ ] A world leaf fully outside a light's cone frustum casts no shadow into that light's shadow map; a leaf inside the cone still does. Verified by a unit test on the cone-frustum classification against the light-space matrix planes.
- [ ] Shadow visuals on `content/dev/maps/campaign-test.prl` are unchanged for lights whose cone contains the same visible geometry as before (no missing or newly-appearing shadows for in-view occluders).
- [ ] `rank_lights` selects/rejects candidates by a cone-frustum test, not a camera-in-sphere test. A light whose cone cannot influence any camera-visible region is not ranked into a slot; a light whose cone overlaps the view is. Covered by unit tests replacing the current `is_in_frustum_approx` path.
- [ ] All existing `spot_shadow.rs` ranking tests pass or are updated in step with the new candidate test; no test still asserts sphere-test behavior.
- [ ] `cargo test -p postretro` and `cargo build -p postretro` pass.

## Tasks

### Task 1: Cone frustum primitive
Add a cone-frustum representation derived from a spotlight's light-space view-projection matrix (the matrix `light_space_matrix()` already returns). Provide: (a) plane extraction matching `extract_frustum_planes_for_gpu`'s convention so the same 6-plane layout feeds both CPU and GPU tests, and (b) an AABB-vs-frustum classification on the CPU for rank-time candidate selection. This is the shared geometric core both other tasks consume. Keep it in the lighting module alongside `light_space_matrix()` so the frustum and the matrix that defines it live together.

### Task 2: Cone-frustum shadow light ranking
Replace the `is_in_frustum_approx` (camera-in-sphere) pre-filter in `SpotShadowPool::rank_lights` with the Task 1 cone-frustum test. The current signature takes `camera_position` and `influence_volumes: &[LightInfluence]`; the new test tests each candidate's cone frustum (from `light_space_matrix(light)`) against the camera's view region. The candidate-region test is the conservative **cone-AABB-vs-camera-frustum** test (see Rough sketch) — not exact frustum-vs-frustum SAT. At a 12-slot pool with small candidate counts, a conservative test can only over-include a candidate, never wrongly reject one (so it never drops a shadow), and the over-inclusion cost is negligible at this scale; exact SAT adds complexity that hurts legibility for no gain. Update the call site in `update_dynamic_light_slots` (`render/mod.rs`) and the `rank_lights` unit tests.

After this task `is_in_frustum_approx` has no callers — it has exactly one today, the `rank_lights` path this task replaces. **Delete the `is_in_frustum_approx` method and drop the `influence_volumes` parameter from `rank_lights`.** Keep the `LightInfluence` struct itself: it is load-bearing elsewhere — PRL section ID 21 loading (`prl.rs`), and SDF light culling / influence-sphere early-outs (`render/mod.rs`, `forward.wgsl`). Leave those paths untouched.

### Task 3: Per-slot GPU cone cull in the shadow depth pass
Make each occupied shadow slot's depth pass cull world geometry to the slot's cone before drawing, reusing the BVH traversal + indirect machinery rather than `draw_indexed(0..index_count)`. The existing `ComputeCullPipeline` holds one set of cull/indirect buffers and overwrites them per `dispatch`; the shadow pass needs an indirect draw list per occupied slot within one frame, so the camera cull's single buffer set cannot be shared directly. Use approach (A) from the Rough sketch — a dedicated shadow cull owner. Its indirect storage is **a single shared indirect buffer carved into `SHADOW_POOL_SIZE` (12) sub-regions by offset, one allocation** — not 12 separate buffer objects. Both layouts cost the same total bytes, so memory was never the deciding axis; the single buffer is one allocation and scales cleanly as community maps raise the leaf-count ceiling. Per occupied slot: write the cone frustum into the uniform, dispatch traversal into that slot's sub-region, then issue the indirect depth draw against that sub-region. Replace the unconditional `draw_indexed` with the indirect depth draw (`draw_indirect(pass, None)` — the depth-only shadow pipeline has no group-1 texture slot, matching the depth pre-pass usage).

## Sequencing

**Phase 1 (sequential):** Task 1 — the cone-frustum primitive; both other tasks consume it.
**Phase 2 (concurrent):** Task 2 (CPU rank-time test) and Task 3 (GPU per-slot cull) — independent consumers of Task 1. Task 2 lives in `spot_shadow.rs` + the rank call site; Task 3 lives in the shadow render-pass recording + cull plumbing. No shared files beyond the Task 1 primitive.

## Rough sketch

**Cone frustum (Task 1).** A spotlight's perspective light-space matrix already *is* a cone-bounding frustum: `light_space_matrix()` builds `perspective_rh(2·cone_angle_outer, 1.0, near, falloff_range) · look_at`. Its 6 clip planes bound the cone's pyramidal approximation. Reuse the exact plane-extraction math in `compute_cull.rs::extract_frustum_planes_for_gpu` (rows of the combined matrix; normalize; `[nx,ny,nz,d]`) so CPU and GPU agree bit-for-bit on convention. The CPU AABB test mirrors the WGSL `is_aabb_outside_frustum` (p-vertex selection per plane). Factor the plane-extraction helper so both `compute_cull.rs` and the new code call one implementation rather than duplicating the row math.

**Rank-time candidate test (Task 2).** Today's test asks "is the camera inside the light's influence sphere." The cone-frustum replacement asks "can this light's cone reach anything the camera sees." The chosen test is the conservative **cone-AABB-vs-camera-frustum**: enclose the light's cone frustum in an AABB and test it against the camera frustum planes (available from the per-frame `view_proj`). This is conservative — it can only over-include a candidate, never wrongly reject one — so it never drops a shadow that exact frustum-vs-frustum SAT would keep. At a 12-slot pool with small candidate counts the over-inclusion cost is negligible, and the SAT alternative adds plane-set separating-axis complexity that hurts legibility for no real-world gain. The camera position currently threaded into `rank_lights` stays for the score heuristic (distance term); only the *visibility* pre-filter changes.

**Per-slot GPU cull (Task 3).** The blocker: `ComputeCullPipeline` owns a single `indirect_buffer` / `visible_cells_buffer` / `uniform_buffer`, overwritten each `dispatch`, and the camera cull dispatch + camera forward/depth draws already consume them this frame. Two viable approaches, pick one:

- **(A, chosen) Dedicated shadow cull instance with one shared indirect buffer in sub-regions.** A second `ComputeCullPipeline`-like owner sized for the shadow path, holding a **single indirect buffer carved into `SHADOW_POOL_SIZE` sub-regions by offset** — one allocation, not one buffer object per slot. Both layouts cost the same total bytes (`SHADOW_POOL_SIZE · total_leaves · 20`), so memory never decided it; the single buffer is one allocation and scales cleanly as community maps push leaf counts up. Each occupied slot: write the cone frustum into the uniform, dispatch traversal into that slot's sub-region, then the slot's render pass issues the indirect depth draw against that sub-region. Cost: that one shared buffer plus `SHADOW_POOL_SIZE` extra compute dispatches per frame.
- **(B) Serialize per slot against shared buffers.** Reuse the camera cull's buffers, but dispatch the cone cull and the slot's depth draw as an ordered pair per slot (cull → draw → cull → draw …), accepting that the camera cull must re-run afterward to restore camera-visible indirect state before the camera depth/forward passes. Simpler memory, but couples shadow culling to camera-cull ordering and adds a camera re-cull. Likely worse; documented as the rejected fallback.

Chosen: **(A)** — clean ordering, no camera re-cull, bounded extra memory in one allocation. The shadow cull reuses the same `bvh_cull.wgsl` shader and BVH node/leaf buffers (those are read-only and already uploaded once at level load — share them; only the per-slot writable indirect/status and the uniform/visible-cells differ).

**Visible-cells gate for the shadow cull.** `bvh_cull.wgsl` ANDs the frustum test with the `visible_cells` bitmask. For the shadow cull, set the bitmask to the **camera's** visible cell set (same `VisibleCells` the camera cull used this frame) so a leaf is shadow-drawn only when (in cone) AND (camera-visible). This keeps shadow casters limited to geometry the player can actually see receiving or casting — and reuses an already-computed set. A spotlight in a room the camera cannot see contributes nothing the forward pass samples, so gating on camera-visible cells is safe. Pin this in the spec body; do not pass `DrawAll` (that would re-introduce full-scene traversal for the frustum-only case). The shadow shader/cull need no WGSL change — only the frustum planes and the visible-cells buffer contents differ per dispatch.

**Plumbing.** The shadow render-pass recording loop (`render/mod.rs`, the `used_slots` loop) must gain access to: the per-slot cone matrix (already computed and uploaded in `update_dynamic_light_slots`; the same `light_space_matrix(candidate)` value), the camera `VisibleCells` for this frame (already in scope as `visible` in the render path), and the shadow cull owner. The cone matrix per slot is currently only written into GPU buffers inside `update_dynamic_light_slots`; either recompute it in the shadow loop from `shadow_candidate_lights` + `slot_assignment` (cheap, 12 max) or stash a per-slot `Mat4` on the pool when ranking. Prefer stashing a `Vec<(slot, Mat4)>` (or per-slot `Option<Mat4>`) on `SpotShadowPool` during `update_dynamic_light_slots` so the render loop reads it directly without re-deriving which candidate owns each slot.
