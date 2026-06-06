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
- [ ] On a map with a spotlight whose cone covers a small fraction of the level, the shadow-depth pass for that light submits fewer indices than the full scene index count (verifiable via a per-slot submitted-index count or `POSTRETRO_GPU_TIMING` `sdf_shadow`/shadow-pass delta — see Open questions on timing label).
- [ ] A world leaf fully outside a light's cone frustum casts no shadow into that light's shadow map; a leaf inside the cone still does. Verified by a unit test on the cone-frustum classification against the light-space matrix planes.
- [ ] Shadow visuals on `content/dev/maps/campaign-test.prl` are unchanged for lights whose cone contains the same visible geometry as before (no missing or newly-appearing shadows for in-view occluders).
- [ ] `rank_lights` selects/rejects candidates by a cone-frustum test, not a camera-in-sphere test. A light whose cone cannot influence any camera-visible region is not ranked into a slot; a light whose cone overlaps the view is. Covered by unit tests replacing the current `is_in_frustum_approx` path.
- [ ] All existing `spot_shadow.rs` ranking tests pass or are updated in step with the new candidate test; no test still asserts sphere-test behavior.
- [ ] `cargo test -p postretro` and `cargo build -p postretro` pass.

## Tasks

### Task 1: Cone frustum primitive
Add a cone-frustum representation derived from a spotlight's light-space view-projection matrix (the matrix `light_space_matrix()` already returns). Provide: (a) plane extraction matching `extract_frustum_planes_for_gpu`'s convention so the same 6-plane layout feeds both CPU and GPU tests, and (b) an AABB-vs-frustum classification on the CPU for rank-time candidate selection. This is the shared geometric core both other tasks consume. Keep it in the lighting module alongside `light_space_matrix()` so the frustum and the matrix that defines it live together.

### Task 2: Cone-frustum shadow light ranking
Replace the `is_in_frustum_approx` (camera-in-sphere) pre-filter in `SpotShadowPool::rank_lights` with the Task 1 cone-frustum test. The current signature takes `camera_position` and `influence_volumes: &[LightInfluence]`; the new test needs each candidate's cone frustum (from `light_space_matrix(light)`) tested against the camera's view region. Decide and pin the candidate-region representation (see Rough sketch). Update the call site in `update_dynamic_light_slots` (`render/mod.rs`) and the `rank_lights` unit tests. The `LightInfluence` slice and `is_in_frustum_approx` may become unused for shadow ranking — if so, drop the parameter and mark the influence method's fate (still used elsewhere? confirm before removing).

### Task 3: Per-slot GPU cone cull in the shadow depth pass
Make each occupied shadow slot's depth pass cull world geometry to the slot's cone before drawing, reusing the BVH traversal + indirect machinery rather than `draw_indexed(0..index_count)`. The existing `ComputeCullPipeline` holds one set of cull/indirect buffers and overwrites them per `dispatch`; the shadow pass needs an indirect draw list per occupied slot within one frame, so the camera cull's single buffer set cannot be shared directly. Decide the buffer-ownership approach (see Rough sketch), dispatch one cone cull per occupied slot before its render pass, and replace the unconditional `draw_indexed` with the indirect depth draw (`draw_indirect(pass, None)` — the depth-only shadow pipeline has no group-1 texture slot, matching the depth pre-pass usage).

## Sequencing

**Phase 1 (sequential):** Task 1 — the cone-frustum primitive; both other tasks consume it.
**Phase 2 (concurrent):** Task 2 (CPU rank-time test) and Task 3 (GPU per-slot cull) — independent consumers of Task 1. Task 2 lives in `spot_shadow.rs` + the rank call site; Task 3 lives in the shadow render-pass recording + cull plumbing. No shared files beyond the Task 1 primitive.

## Rough sketch

**Cone frustum (Task 1).** A spotlight's perspective light-space matrix already *is* a cone-bounding frustum: `light_space_matrix()` builds `perspective_rh(2·cone_angle_outer, 1.0, near, falloff_range) · look_at`. Its 6 clip planes bound the cone's pyramidal approximation. Reuse the exact plane-extraction math in `compute_cull.rs::extract_frustum_planes_for_gpu` (rows of the combined matrix; normalize; `[nx,ny,nz,d]`) so CPU and GPU agree bit-for-bit on convention. The CPU AABB test mirrors the WGSL `is_aabb_outside_frustum` (p-vertex selection per plane). Factor the plane-extraction helper so both `compute_cull.rs` and the new code call one implementation rather than duplicating the row math.

**Rank-time candidate test (Task 2).** Today's test asks "is the camera inside the light's influence sphere." The cone-frustum replacement asks "can this light's cone reach anything the camera sees." Cheapest faithful version: test the light's cone frustum against the **camera frustum** (frustum-vs-frustum), or test the cone-frustum planes against an AABB enclosing the camera's visible region. The camera frustum is available from the per-frame `view_proj`. Pin one representation in the spec body before implementing — frustum-vs-frustum (separating-axis on the two plane sets) is exact but heavier; cone-vs-camera-AABB is cheaper and conservative. Given the pool is 12 slots and candidate counts are small, prefer the exact frustum-vs-frustum or a conservative cone-AABB-vs-camera-frustum — choose one and state it. The camera position currently threaded into `rank_lights` stays for the score heuristic (distance term); only the *visibility* pre-filter changes.

**Per-slot GPU cull (Task 3).** The blocker: `ComputeCullPipeline` owns a single `indirect_buffer` / `visible_cells_buffer` / `uniform_buffer`, overwritten each `dispatch`, and the camera cull dispatch + camera forward/depth draws already consume them this frame. Two viable approaches, pick one:

- **(A) Dedicated shadow cull instance with per-slot indirect buffers.** A second `ComputeCullPipeline`-like owner sized for the shadow path, holding `SHADOW_POOL_SIZE` indirect-buffer regions (or one buffer with per-slot offsets). Each occupied slot: write the cone frustum into the uniform, dispatch traversal into that slot's indirect region, then the slot's render pass issues the indirect depth draw against that region. Cost: more storage-buffer memory (one indirect buffer per slot, each `total_leaves · 20` bytes) and `SHADOW_POOL_SIZE` extra compute dispatches per frame.
- **(B) Serialize per slot against shared buffers.** Reuse the camera cull's buffers, but dispatch the cone cull and the slot's depth draw as an ordered pair per slot (cull → draw → cull → draw …), accepting that the camera cull must re-run afterward to restore camera-visible indirect state before the camera depth/forward passes. Simpler memory, but couples shadow culling to camera-cull ordering and adds a camera re-cull. Likely worse; documented as the rejected fallback.

Default recommendation: **(A)** — clean ordering, no camera re-cull, bounded extra memory. The shadow cull reuses the same `bvh_cull.wgsl` shader and BVH node/leaf buffers (those are read-only and already uploaded once at level load — share them; only the per-slot writable indirect/status and the uniform/visible-cells differ).

**Visible-cells gate for the shadow cull.** `bvh_cull.wgsl` ANDs the frustum test with the `visible_cells` bitmask. For the shadow cull, set the bitmask to the **camera's** visible cell set (same `VisibleCells` the camera cull used this frame) so a leaf is shadow-drawn only when (in cone) AND (camera-visible). This keeps shadow casters limited to geometry the player can actually see receiving or casting — and reuses an already-computed set. A spotlight in a room the camera cannot see contributes nothing the forward pass samples, so gating on camera-visible cells is safe. Pin this in the spec body; do not pass `DrawAll` (that would re-introduce full-scene traversal for the frustum-only case). The shadow shader/cull need no WGSL change — only the frustum planes and the visible-cells buffer contents differ per dispatch.

**Plumbing.** The shadow render-pass recording loop (`render/mod.rs`, the `used_slots` loop) must gain access to: the per-slot cone matrix (already computed and uploaded in `update_dynamic_light_slots`; the same `light_space_matrix(candidate)` value), the camera `VisibleCells` for this frame (already in scope as `visible` in the render path), and the shadow cull owner. The cone matrix per slot is currently only written into GPU buffers inside `update_dynamic_light_slots`; either recompute it in the shadow loop from `shadow_candidate_lights` + `slot_assignment` (cheap, 12 max) or stash a per-slot `Mat4` on the pool when ranking. Prefer stashing a `Vec<(slot, Mat4)>` (or per-slot `Option<Mat4>`) on `SpotShadowPool` during `update_dynamic_light_slots` so the render loop reads it directly without re-deriving which candidate owns each slot.

## Open questions
- **GPU timing label.** Is the spot-shadow depth pass currently timestamp-bracketed? `rendering_pipeline.md` §12 lists `sdf_shadow` but not a spot-shadow pass label. If AC needs a timing-based verification, a `shadow` timing pair may need adding — or verify via a CPU-side submitted-index counter instead. Decide which during implementation; the per-leaf classification unit test (AC 2) is the primary correctness gate regardless.
- **Approach A memory.** `SHADOW_POOL_SIZE` (12) × `total_leaves` × 20 bytes of indirect buffer. For a 4096-leaf map that is ~1 MB total — acceptable. Confirm against the largest target map; if a map pushes leaf counts high, consider a single shared indirect buffer with 12 sub-regions (same total size, fewer buffer objects). State the chosen layout in the spec body.
- **`LightInfluence` after Task 2.** If shadow ranking stops using `is_in_frustum_approx`, confirm whether that method or the `influence_volumes` parameter has other callers before deleting (it is `#[allow(dead_code)]` today, suggesting it may already be unused outside ranking). Drop or keep accordingly; do not leave a half-wired parameter.
- **Frustum-vs-frustum exactness (Task 2).** Pin the chosen camera-region test (exact frustum-vs-frustum SAT vs conservative cone-AABB-vs-camera-frustum) before implementing. The conservative test may keep a few extra candidates ranked; with a 12-slot pool and small candidate counts that is harmless, so the cheaper conservative test is acceptable unless a stress map shows pool thrash.
