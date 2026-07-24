# Shadow Cone Cull Parallel Dispatch

## Goal

The shadow cone cull walks the whole BVH serially — one `dispatch_workgroups(1, 1, 1)` per occupied shadow slot, running the stack-free DFS in a single GPU invocation. Replace it with a parallel per-leaf dispatch. The camera path already escapes this walk via the candidate cull; the shadow path never does, and pays it once per occupied spot slot and once per occupied cube-slot face.

## Scope

### In scope

- Add GPU timing instrumentation for the shadow cull pass, which is currently unmeasured.
- Replace the per-slot serial tree walk with a parallel dispatch that tests every leaf AABB against the slot's cone frustum.
- Keep the existing per-slot indirect sub-region layout, bind groups, and 256-byte-aligned region stride.
- Apply the same change to both `ShadowCull` instances — the spot pool and the cube pool.

### Out of scope

- Changing the camera cull path. `candidate_cull.wgsl` and its `CellDrawIndex`-driven gather are untouched.
- Reusing `CellDrawIndex` for shadow slots. The shadow path deliberately binds an all-ones visible-cell buffer, because an occluder outside the camera's portal-visible set can still shadow a visible receiver; there is no per-slot candidate set to gather from.
- A hierarchical or persistent-threads GPU traversal. This plan replaces the traversal, it does not parallelize it.
- Changing which slots get dispatched, the ranking that fills them, or the promoted-depth cache.
- Changing `BvhNode` / `BvhLeaf` layout. Nodes stop being read by this path but stay on disk for the camera fallback.
- Removing `bvh_cull.wgsl`. It remains the camera fallback for `DrawAll`, non-portal `Culled`, and out-of-range visible-cell frames.

## Acceptance criteria

- [ ] The shadow cull pass reports a per-frame GPU time under `POSTRETRO_GPU_TIMING=1`, distinct from the camera `cull` label.
- [ ] On `stress-warren-lit` with the shadow pool saturated, shadow cull GPU time drops by at least 70% versus the serial walk.
- [ ] Rendered shadow output is unchanged: at each checked-in camera probe, every shadow slot's submitted index ranges are identical to the serial walk's.
- [ ] Every leaf's indirect slot in an occupied region is written each frame the region is dispatched — either a live draw or a zeroed entry. No slot retains a prior frame's value.
- [ ] Slots that are not dispatched retain their prior contents, matching current behavior.
- [ ] Summed GPU pass time on a small representative map does not regress by more than 2% over a `POSTRETRO_GPU_TIMING=1` window.
- [ ] A map with no BVH still falls back to the unconditional draw-all-world-geometry path, dropping no shadow.
- [ ] Both the spot pool and the cube pool use the parallel path; neither retains a serial dispatch.

## Tasks

### Task 1: Instrument the shadow cull pass

`ShadowCull::dispatch_occupied_slots_filtered` in `crates/renderer/src/shadow_cull.rs` opens its compute pass with `timestamp_writes: None`, so the pass has never been measured and there is no baseline. Add a timestamp pair for it, following the existing `FrameTiming` helper in `crates/renderer/src/render/frame_timing.rs` — a fixed pass-label list chosen at construction, 120-frame averaging, and the skip-counting behavior that distinguishes "pass didn't run" from "pass ran with anomalous ticks". The spot and cube instances need distinguishable labels. Record baseline numbers for the maps named in the acceptance criteria before Task 2 changes anything.

### Task 2: Parallel per-leaf cull shader

Add a compute shader that tests one leaf per invocation instead of walking the tree. It binds the same group-0 layout the shadow path already uses — per-slot frustum uniform, node array, leaf array, visible-cells buffer, the slot's indirect sub-region, and the cull-status scratch — so the existing per-slot bind groups are reused unchanged, with the node array simply going unread. Each invocation reads its leaf, tests the leaf AABB against the six cone planes, and writes the leaf's indirect entry: the full draw on pass, a zeroed entry on reject. Use a workgroup size of 64 and guard against the tail past `arrayLength(&leaves)`. Because the shadow path binds an all-ones visible-cell buffer, the per-leaf cell test always passes and can be dropped from this shader rather than evaluated.

### Task 3: Switch both pools to the parallel dispatch

In `ShadowCull::new`, build the pipeline from the Task 2 shader. In `dispatch_occupied_slots_filtered`, replace the per-slot `dispatch_workgroups(1, 1, 1)` with a dispatch sized to `ceil(total_leaves / 64)` workgroups, keeping the existing loop that sets each occupied slot's bind group. The early return when `total_leaves == 0` and the uniform writes ordered before the compute pass both stay as they are. Both constructed instances — the spot pool and the cube pool — pick this up from the shared constructor.

### Task 4: Equivalence verification

Add a CPU mirror of the per-leaf cone test alongside the existing `candidate_cull_mirror` helpers in `crates/postretro/src/candidate_cull_mirror.rs`, and extend the probe harness in `crates/postretro/src/candidate_cull_probes.rs` to assert that for each probe camera and each occupied shadow slot, the set of leaves the parallel path submits matches the set the serial tree walk submits. The tree walk rejects a leaf when any ancestor AABB fails the frustum test; the flat path tests only the leaf AABB. Since an ancestor AABB contains its leaves, ancestor rejection implies leaf rejection, so the flat path is a superset by construction — the assertion must confirm the two sets are in fact equal at every probe, and any divergence is a bug in the plane extraction rather than an expected difference.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the baseline every performance criterion is measured against.
**Phase 2 (sequential):** Task 2 — Task 3 consumes the shader it adds.
**Phase 3 (concurrent):** Task 3, Task 4 — Task 4's CPU mirror is written against the shader contract from Task 2, not against Task 3's wiring.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A shadow is never dropped | Pre-existing: no-BVH maps fall back to an unconditional draw of all world geometry | Task 3 must not alter the `total_leaves == 0` early return that routes to the fallback | AC 7 |
| Every leaf slot in a dispatched region is written each frame | Task 2 (one invocation per leaf, writes on both branches) | A tail guard that returns early without writing would leave stale slots | AC 4 |
| Undispatched slots keep prior contents | Pre-existing: leaves outside a cone clip out against the slot's projection and contribute no depth | Task 3 changing the dispatch loop's skip conditions | AC 5 |
| Submitted geometry per slot is unchanged | Task 2 (leaf AABB test matches the tree walk's leaf test) | Cone plane extraction must stay bit-identical between the two paths | AC 3, AC 8 |

## Rough sketch

The serial walk exists because the camera path needed hierarchical rejection before the candidate cull was built. The shadow path inherited it, but it cannot benefit from the hierarchy the way the camera path did: with an all-ones visible-cell mask there is no portal narrowing, so the only work the tree saves is skipping subtrees whose AABB misses the cone. That saving is real but it is bounded by tree quality, and it is bought at the cost of running the entire traversal on one GPU thread. A flat test does strictly more arithmetic and finishes far sooner, because it does that arithmetic across the whole machine instead of one lane.

The cull-status scratch buffer is shared across slots and already overwritten by every dispatch in the pass; the shadow path has no wireframe overlay reading it. Parallel dispatch does not make that worse, but do not start reading it for shadow slots without giving each slot its own region.

## Open questions

- Batching all occupied slots into a single dispatch — slot index on the `z` dimension, per-slot planes read from an array rather than a per-slot uniform — would remove the per-slot bind-group set and dispatch overhead. That is a larger change to the bind-group layout than this plan takes on, and it is only worth it if Task 1's baseline shows per-dispatch overhead is material at 96 spot regions and 36 cube regions.
- If Task 1 shows the shadow cull is already a negligible share of frame GPU time on the target maps, this plan should stop after Task 1 and the measurement should be recorded here rather than shipped around.
