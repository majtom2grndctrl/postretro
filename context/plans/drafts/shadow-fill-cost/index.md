# shadow-fill-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §5 (incl. Indirect-args invariant), §7.1 steps 6–8, §12 · `context/lib/development_guide.md` §1.4 · read at 2c9a7ca2c · evidence: `research.md`

## Problem
A `visible-span-draws` review finding, confirmed in source: a shadow region that redraws world depth costs as much as the whole map, however little its frustum reaches. Cause: shadow world culling runs on the GPU, but on Metal the CPU still issues a driver draw for every leaf slot that culling might zero. Each region draws whole material buckets, so a region costs L draws, and its cull walks the whole BVH on one GPU thread. The hallway is the one map over budget on the Mac, and on it the lift carries a point light. While that light holds a shadow slot, every moving frame refills all six cube faces. That is about 6 × 8,437 driver draws, which extrapolates to about 2.5 ms of CPU encode (unmeasured, `research.md` §Cost model). When this is done, a region's world draws follow the cells its frustum reaches, at most one draw per run of reached cells. No GPU shadow cull runs, and shadows are unchanged.

## Decisions
- **The CPU is the only culler of shadow world geometry.** The per-region GPU cone cull and its indirect buffers (about 22 MB on the hallway) are deleted. This diverges from `plans/done/shadow-cone-cull`, which chose GPU culling before Metal's per-slot draw cost was known (`research.md` §Cost model). `rendering_pipeline.md` §7.1 steps 6–8 and §5 are updated at promotion.
- **A region draws its reached cells' index ranges with direct draws, merging abutting ranges.**
  - Shadow depth is position-only, with no fragment stage and no material, so material buckets don't matter to it.
  - Per-cell ranges are derived at level install from the loaded leaves.
  - A cell whose leaves don't form one contiguous index range draws each leaf's range instead.
- **A cell is reached when the union of its leaves' AABBs intersects the region's frustum.** The test uses the planes and box test the mesh and mover occluder culls already use. Baked cell bounds are not used, because a leaf can extend past them (`research.md` §Containment). A rejected box's triangles lie wholly outside the frustum, so culling loses nothing.
- **A region's reach comes from its own frustum only,** never from the camera's visible set or fog reach. An occluder the camera can't see still shadows a visible receiver (§7.1 step 6).
- **Reach is computed for exactly the regions that draw world this frame, from the current matrix.** Those are the uncached live draw, the dynamic cold fill and the promoted cold fill. A warm region computes nothing and draws no world.
- **An empty reach draws nothing.** A level with no BVH still draws all world geometry, as today.
- **Shadows leave the indirect path.** The indirect contract's owner rules and inventory shrink to the camera's, and the camera rules don't change. The nested-binding scanner fixtures move to the camera draw, so that coverage stays. Direct draws are range-checked by wgpu in every build. This is still a release-safety review point under §5.
- **Layer: renderer, runtime.** A GPU-free step turns (planes, per-cell boxes, per-cell ranges) into a region's draw list. It is testable without a GPU, and the draw layer only issues the result. The PRL, compiler and bake are unchanged. A test pins the cell-major index order the ranges rely on.
- **Hot-path bound.** After warm-up, building draw lists allocates nothing. Per drawing region it tests each cell box at most once (`development_guide.md` §1.4).
- **Non-goals.**
  - Cache capacity, cache keys, and the promoted cache's over-capacity drop.
  - The camera's GPU cull. It stays, and a CPU-only camera path is a separate question.
  - Dynamic shadows in headless capture. Capture is static-light-only by design, so this brief proves with oracle rows and a playtest instead (`research.md` §Headless capture).
  - The camera fallback's use of baked cell bounds, which is seeded as `camera-fallback-cell-bounds-overhang`.
  - Other backends. Entity and mover occluders, which are already culled on the CPU per region.
  - `bvh-leaf-clustering`. This brief removes its shadow value, because shadow draws now go per cell run. Clustering is re-priced in its own draft, not changed here.

## Acceptance

### Automated
Reach:
- [ ] A spot region draws the index ranges of exactly the cells whose leaf-bounds box intersects its frustum. Each of a cube light's six faces does the same against its own face frustum.
- [ ] Every leaf whose AABB intersects a region's frustum lies inside that region's drawn ranges, checked by brute force over every leaf. This holds over randomized frusta on synthetic worlds (fixed seed and count) and at the on-demand stress-map probes, including the hallway lift light's faces at several lift heights.
- [ ] A leaf whose geometry extends past its cell's baked bounds, under a frustum that touches only that overhang, has its cell drawn.
- [ ] A cell whose leaves all lie outside the frustum is not drawn. A cell with one leaf inside is drawn.
- [ ] A frustum that reaches no cell issues no world draws and no error, including on the frame after a nonempty reach. A frustum that reaches every cell draws every drawable leaf's indices exactly once.
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
- [ ] After a level install, the first shadow fill uses the new level's boxes and ranges, even when a region's matrix is unchanged.
- [ ] A level with no BVH draws all world geometry in its shadow passes.
- [ ] Skinned and rigid occluders still draw into every region that draws them today (regression guard).

Bounds and contract:
- [ ] After warm-up, building draw lists for any frame allocates nothing.
- [ ] Building a region's draw list tests each cell box at most once.
- [ ] The indirect-contract scanner passes. Its camera rules and inventory are unchanged, its shadow entries are gone, and its nested-binding fixtures exercise the camera draw. This is a diff gate and a release-safety review, not a behavior test.

### Manual
- [ ] Baseline before any change: release build on this Mac, window in front, machine state recorded per §12. Record on campaign-test, and on the hallway at a pose where the lift light holds a slot and the cache log shows uncached cube fills (`research.md` §Cost model: no recorded run has one). For each, record:
  - `[CpuTiming]` `render_submit` and `rec_shadow_depth` medians over at least five windows;
  - a `sample` profile of wgpu-hal Metal `draw_indexed_indirect` time;
  - Metal System Trace GPU time for the shadow-cull and shadow-depth passes.
- [ ] After: on the hallway with the lift moving, the `render_submit` median falls. On campaign-test it does not rise. Report what the reach step itself costs.
- [ ] Metal System Trace after: on both maps, summed GPU time of the shadow-depth passes plus the former shadow-cull pass does not rise. Drawing whole reached cells rasterizes some triangles the per-leaf cull dropped.
- [ ] Report the GPU memory freed on the hallway.
- [ ] Visual: compare against the pre-change build. Watch the hallway lift light through a full cycle, and walk a stress-warren-crates room with more than four point lights in reach. No shadow drops, pops or flickers, including at frustum edges and with a light on a cell boundary.

## Path
- Seams:
  - The cull filter in `ShadowCullPipeline::dispatch_occupied_slots_filtered` (`should_dispatch_*_cull` on both frame plans) is today's "draws world this frame" predicate.
  - The six `draw_slot_indirect` sites in `renderer_dynamic_shadow_passes.rs` become one direct-draw helper.
  - `full.bvh_leaves` and `full.cell_draw_index` give per-cell boxes and index ranges at install. Don't use `BspLeafRecord.face_start`: it predates face cuts.
  - `aabb_intersects_frustum` and `cone_frustum_planes` (`render-data/src/cone_frustum.rs`).
  - `record_skinned_depth` and `record_kinematic_movers` share the uncached pass after the world draw.
- Shape: direct draws per run of reached cells. Rival: one install-time indirect buffer of every leaf's record (about 165 KB). It keeps shadows on the indirect path, at one draw per leaf rather than per run.
- A cube light's faces can first filter cells against the light's range box.
- First slice: per-cell boxes and ranges, the reach step, and the brute-force oracle, on the overhang fixture, the hallway probe and the contiguity pin. The draw layer comes after; this order falsifies the contiguity and reach assumptions first.
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
