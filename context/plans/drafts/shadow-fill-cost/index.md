# shadow-fill-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §5 (incl. Indirect-args invariant), §7.1 steps 6–8, §12, §13 · `context/lib/development_guide.md` §1.4, §Workspace · read at a6a67bcce · evidence: `research.md`

## Problem
A `visible-span-draws` review finding, confirmed in source: a shadow region that redraws world depth costs as much as the whole map, however little its frustum reaches. Cause: shadow world culling runs on the GPU, but on Metal the CPU still issues a driver draw for every leaf slot that culling might zero. Each region draws whole material buckets, so a region costs L draws, and its cull walks the whole BVH on one GPU thread. On stress-warren-hallway-inspection the lift carries a point light. While that light holds a shadow slot, every moving frame refills all six cube faces. That is about 6 × 8,437 driver draws, which extrapolates to about 2.5 ms of CPU encode. No recorded run has yet caught a refill (`research.md` §Cost model). When this is done, a region's world culling and draws follow what its frustum reaches, at most one draw per run of reached cells. No GPU shadow cull runs, and shadows are unchanged.

## Decisions
- **The CPU is the only culler of shadow world geometry.** The per-region GPU cone cull and its indirect buffers (about 22 MB on the hallway) are deleted.
  - This diverges from `plans/done/shadow-cone-cull`, which chose GPU culling before Metal's per-slot draw cost was known.
  - It also diverges from `plans/done/runtime-cell-spatial-contract`, which named shadow cone culling as a runtime BVH role.
  - At promotion, `rendering_pipeline.md` §5, §7.1 steps 6–8 and §13 ("GPU-driven culling") are updated so GPU culling is camera-only.
- **Reach comes from walking the baked BVH on the CPU against the region's frustum.**
  - The walk uses the depth-first skip order the GPU walk uses, and the box test the mesh and mover occluder culls already share with the GPU (`aabb_intersects_frustum`).
  - A cell is reached when any of its leaves passes.
  - Leaf AABBs bound their triangles exactly, so baked cell bounds, which leaves can overhang (`research.md` §Containment), are never consulted.
  - A rejected subtree's triangles lie wholly outside the frustum, so culling loses nothing.
- **A region draws its reached cells' index ranges with direct draws, merging abutting ranges.**
  - Shadow depth is position-only, with no fragment stage and no material, so material buckets don't matter to it.
  - Per-cell ranges are derived at level install from the loaded leaves.
  - A cell whose leaves don't form one contiguous index range draws each leaf's range instead.
- **A region's reach comes from its own frustum only,** never from the camera's visible set or fog reach. An occluder the camera can't see still shadows a visible receiver (§7.1 step 6).
- **Reach is computed for exactly the regions that draw world this frame, from the current matrix.** Those are the uncached live draw, the dynamic cold fill and the promoted cold fill. A warm region computes nothing and draws no world.
- **An empty reach draws nothing.** A level with no BVH still draws all world geometry, as today.
- **Shadows leave the indirect path.** The indirect contract's owner rules and inventory shrink to the camera's, and the camera rules don't change. The nested-binding scanner fixtures move to the camera draw, so that coverage stays. Direct draws are range-checked by wgpu in every build. This is still a release-safety review point under §5.
- **Layer.**
  - The reach planner is GPU-free and lives in `render-cpu`, beside `instance_casts_into_cone`. It turns (BVH, planes, per-cell ranges) into a region's draw list.
  - The renderer owns the per-region scratch and only issues the result (`development_guide.md` §Workspace).
  - The PRL, compiler and bake are unchanged. A test pins the cell-major index order the ranges rely on.
- **Hot-path bound.** Reach work follows the BVH nodes the frustum reaches; no step tests every cell of the map. After warm-up, building draw lists allocates nothing (`development_guide.md` §1.4).
- **Other backends pay a consequence, and it is accepted.**
  - On Vulkan and DX12, where §10's desktop performance floor sits, a region trades one native multi-draw per bucket for one direct draw per run of reached cells.
  - The GPU also no longer drops individual leaves inside reached cells.
  - Both costs follow reach, not map size. They are unmeasured, so a Windows row checks them.
- **Non-goals.**
  - Cache capacity, cache keys, and the promoted cache's over-capacity drop.
  - The camera's GPU cull. It stays, and a CPU-only camera path is a separate question.
  - Light-side portal reach (`research.md` §Rivals). It is a later filter, if the GPU-time row fails.
  - Dynamic shadows in headless capture. Capture is static-light-only by design, so this brief proves with oracle rows and a playtest instead (`research.md` §Headless capture).
  - The camera fallback's use of baked cell bounds, which is seeded as `camera-fallback-cell-bounds-overhang`.
  - Entity and mover occluders, which are already culled on the CPU per region.
  - `bvh-leaf-clustering`. This brief removes its shadow value, because shadow draws now go per cell run. Clustering is re-priced in its own draft, not changed here.

## Acceptance

### Automated
Reach:
- [ ] A spot region draws the index ranges of exactly the cells that own at least one leaf whose AABB intersects its frustum. Each of a cube light's six faces does the same against its own face frustum.
- [ ] The walk's reached-cell set equals a brute-force test over every leaf, in both directions. This holds over randomized frusta on synthetic worlds (fixed seed and count) and at the on-demand stress-map probes, including the hallway lift light's faces at several lift heights.
- [ ] A leaf whose geometry extends past its cell's baked bounds, under a frustum that touches only that overhang, has its cell drawn.
- [ ] A cell whose leaves all lie outside the frustum is not drawn. A cell with one leaf inside is drawn.
- [ ] A frustum that reaches no leaf issues no world draws and no error, including on the frame after a nonempty reach. A frustum that reaches every leaf draws every drawable leaf's indices exactly once.
- [ ] A cell outside the camera's visible set but inside the frustum is drawn. A cell the camera sees but the frustum misses is not.

Draw shape:
- [ ] Abutting reached ranges go out as one draw, and a gap of even one index splits them. A region issues at most one draw per maximal run of reached ranges.
- [ ] A synthetic cell whose leaves aren't contiguous draws each leaf's range, and no index outside its leaves.
- [ ] On compiled levels, face-cut faces included, each cell's leaves form one contiguous index range.
- [ ] Shadow passes issue no indirect draw and bind no material. No frame records a shadow-cull compute pass, and level install creates no per-region shadow indirect buffer.

Per-region state and ordering:
- [ ] Two regions that draw world in one frame with disjoint frusta each draw their own reach.
- [ ] Across consecutive frames with a moving light, each frame draws the reach of that frame's matrix.
- [ ] A cold fill, dynamic or promoted, draws its reach into its cache layer. The next warm frame computes no reach and issues no world draw. A re-key after warm frames draws reach again.
- [ ] After a level install, the first shadow fill uses the new level's BVH and ranges, even when a region's matrix is unchanged.
- [ ] A level with no BVH draws all world geometry in its shadow passes.
- [ ] Skinned and rigid occluders still draw into every region that draws them today (regression guard).

Bounds and contract:
- [ ] The walk never descends below a node whose bounds miss the frustum. On a synthetic world, adding cells outside a fixed frustum leaves the walk's visited-node count unchanged once the tree has the same shape inside the frustum.
- [ ] After warm-up, building draw lists for any frame allocates nothing.
- [ ] The indirect-contract scanner passes. Its camera rules and inventory are unchanged, its shadow entries are gone, and its nested-binding fixtures exercise the camera draw. This is a diff gate and a release-safety review, not a behavior test.

### Manual
- [ ] Baseline before any change: release build on this Mac, window in front, machine state recorded per §12. Record on campaign-test, and on the hallway at a pose where the lift light holds a slot and the cache log shows uncached cube fills. For each, record:
  - `[CpuTiming]` `render_submit` and `rec_shadow_depth` medians over at least five windows;
  - a `sample` profile of wgpu-hal Metal `draw_indexed_indirect` time;
  - Metal System Trace GPU time for the shadow-cull and shadow-depth passes.
- [ ] After: on the hallway pose, the `render_submit` median falls. On campaign-test it does not rise. Report what the reach walk itself costs.
- [ ] Metal System Trace after: on both maps, summed GPU time of the shadow-depth passes plus the former shadow-cull pass does not rise.
- [ ] Windows handoff, not a gate: under GPU timing, the shadow-depth spans on the same poses do not rise against the pre-change build.
- [ ] Report the GPU memory freed on the hallway.
- [ ] Visual: compare against the pre-change build. Watch the hallway lift light through a full cycle, and walk a stress-warren-crates room with more than four point lights in reach. No shadow drops, pops or flickers, including at frustum edges and with a light on a cell boundary.

## Path
- Seams:
  - The cull filter in `ShadowCullPipeline::dispatch_occupied_slots_filtered` (`should_dispatch_*_cull` on both frame plans) is today's "draws world this frame" predicate.
  - The six `draw_slot_indirect` sites in `renderer_dynamic_shadow_passes.rs` become one direct-draw helper.
  - `BvhTree` (`render-data`). The renderer already keeps CPU clones of its nodes and leaves in `ComputeCullPipeline`, and each leaf carries `cell_id`, `index_offset` and `index_count`.
  - `bvh_cull.wgsl` `cull_main` is the traversal the CPU walk mirrors.
  - `aabb_intersects_frustum` and `cone_frustum_planes` (`render-data/src/cone_frustum.rs`).
  - `record_skinned_depth` and `record_kinematic_movers` share the uncached pass after the world draw.
- Shape: a hierarchical CPU walk marks reached cells, which draw as merged direct runs. Rival: a flat test of every cell's leaf-union box, which is O(cells) per region (`research.md` §Rivals).
- Reached cells can be collected without scanning every cell: dedupe with a bitset, and reset only the bits the walk set.
- First slice: per-cell ranges with the contiguity pin, the CPU walk, and the brute-force oracle on the overhang fixture and the hallway probe. The draw layer comes after.
- Deletion fallout (`research.md` §Deletion):
  - Accessors left with no callers.
  - The cache plans' cull-dispatch skip counters, which become world-fill counters.
  - Stale comments.
  - The scanner's shadow arms.
- Files:
  - `renderer_dynamic_shadow_passes.rs` is near 800 lines; split it first, in its own commit, if this grows it.
  - Keep new logic out of `renderer_light_slots.rs` and `compute_cull.rs`, which are both past the threshold.
- Metal System Trace capture and cleanup: `rendering_pipeline.md` §12 (Without timestamp support).

## Open questions
- Where the reach step runs: the existing filter point, or the end of the light-slot update — **delegated**.
- What the cache plans' cull-dispatch counters become, and what the log says — **delegated**.
