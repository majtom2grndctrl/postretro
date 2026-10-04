# shadow-fill-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §5 (incl. Indirect-args invariant), §7.1 steps 6–8, §12, §13 · `context/lib/development_guide.md` §1.4, §Workspace · read at a6a67bcce · evidence: `research.md`

## Problem
A `visible-span-draws` review finding, confirmed in source: a shadow region that redraws world depth costs as much as the whole map, however little its frustum reaches. Cause: shadow world culling runs on the GPU, but on Metal the CPU still issues a driver draw for every leaf slot that culling might zero. Each region draws whole material buckets, so a region costs one draw per leaf, and its cull walks the whole BVH on one GPU thread. On stress-warren-hallway-inspection the lift carries a point light. While that light holds a shadow slot, every moving frame refills all six cube faces, about 2.5 ms of extrapolated CPU encode. No recorded run has yet caught a refill (`research.md` §Cost model). When this is done, a region's world culling and draws follow what its frustum reaches, at most one draw per run of reached cells. No GPU shadow cull runs, and shadows are unchanged.

## Decisions
- **The CPU is the only culler of shadow world geometry.** The per-region GPU cone cull and its indirect buffers (`research.md` §Deletion) are deleted.
  - This diverges from `plans/done/shadow-cone-cull`, which chose GPU culling before Metal's per-slot draw cost was known.
  - It narrows `plans/done/runtime-cell-spatial-contract`: the runtime BVH still accelerates shadow culling, walked on the CPU instead of uploaded for a GPU walk.
  - At promotion, `rendering_pipeline.md` §5, §7.1 steps 6–8 and §13 ("GPU-driven culling") are updated so GPU culling is camera-only.
- **Reach comes from walking the baked BVH on the CPU against the region's frustum.**
  - A leaf is reached when its AABB passes the plane test the mesh and mover occluder culls use. A cell is reached when any of its leaves is.
  - Leaf AABBs bound their triangles exactly, so baked cell bounds, which leaves can overhang (`research.md` §Containment), are never consulted.
  - A rejected subtree's triangles lie wholly outside the frustum, so culling loses nothing.
- **A region draws the index ranges of its reached cells' leaves with direct draws, merging abutting ranges.**
  - Shadow depth is position-only, with no fragment stage and no material, so material buckets don't matter to it.
  - Each cell's leaves are grouped at level install from the loaded leaves. Geometry is cell-major, so a reached cell's leaves merge into one draw, and consecutive reached cells merge further. A cell that isn't contiguous costs extra draws, never wrong geometry, so there is no fallback branch.
- **A region's reach comes from its own frustum only,** never from the camera's visible set or fog reach. An occluder the camera can't see still shadows a visible receiver (§7.1 step 6).
- **Reach is computed for exactly the regions that draw world this frame, from the current matrix.** Those are the uncached live draw, the dynamic cold fill and the promoted cold fill. A warm region computes nothing and draws no world.
- **An empty reach draws no world; the region's clear still runs.** A level with no BVH still draws all world geometry, as today.
- **Shadows leave the indirect path.** The indirect contract's owner rules and inventory shrink to the camera's, and the camera rules don't change. The nested-binding scanner fixtures move to the camera draw, so that coverage stays.
- **Layer.**
  - The reach planner is GPU-free and lives in `render-cpu`.
  - The renderer owns the per-region scratch and only issues the result (`development_guide.md` §Workspace).
  - The PRL, compiler and bake are unchanged. A test pins the cell-major index order the ranges rely on.
- **Hot-path bound.** Reach work follows the BVH nodes the frustum reaches; no step tests every cell of the map. After warm-up, building draw lists allocates nothing (`development_guide.md` §1.4).
- **Accepted on Vulkan and DX12 (§10's desktop floor):** one direct draw per reached run replaces one multi-draw per bucket, and per-leaf GPU culling inside reached cells is lost. Both follow reach, not map size; the Windows row checks them.
- **Non-goals.**
  - Cache capacity, cache keys, and the promoted cache's over-capacity drop. This brief changes what a fill costs, not when one happens.
  - The camera's GPU cull. It stays: its CPU side measured 0.03 ms (`research.md` §Cost model), and no draft owns a CPU-only camera path.
  - Light-side portal reach (`research.md` §Rivals). It is a later filter, if the GPU-time row fails.
  - The camera fallback's use of baked cell bounds, which is seeded as `camera-fallback-cell-bounds-overhang`.
  - `bvh-leaf-clustering`. This brief removes its shadow value, because shadow draws now go per cell run. Clustering is re-priced in its own draft, not changed here.

## Acceptance

### Automated
Reach:
- [ ] A spot region draws the index ranges of exactly the cells that own at least one leaf whose AABB intersects its frustum. Each of a cube light's six faces does the same against its own face frustum.
- [ ] The walk's reached-cell set equals a brute-force test over every leaf, in both directions. This holds over randomized frusta on synthetic worlds (fixed seed and count) and at the on-demand stress-map probes, including the hallway lift light's faces at several lift heights, built from the same face matrices the cube pool uploads.
- [ ] A leaf whose geometry extends past its cell's baked bounds, under a frustum that touches only that overhang, has its cell drawn.
- [ ] A cell whose leaves all lie outside the frustum is not drawn. A cell with one leaf inside is drawn.
- [ ] A frustum that reaches no leaf issues no world draws and no error, including on the frame after a nonempty reach. A nonempty reach on the frame after an empty one draws in full. A frustum that reaches every leaf draws every drawable leaf's indices exactly once.
- [ ] A cell outside the camera's visible set but inside the frustum is drawn. A cell the camera sees but the frustum misses is not.
- [ ] In a recorded frame, a cell outside the camera's visible set and fog reach but inside the frustum is drawn. A cell the camera sees or fog reaches, but the frustum misses, is not.
- [ ] In a recorded frame on a BVH level, a region whose frustum misses part of the world issues world draws covering only its reach, never the whole index buffer. The adapter proof that today asserts whole-bucket shadow draws asserts this instead.
- [ ] A cold fill or uncached live region whose reach is empty still clears its layer to far depth, and a cache layer then becomes warm. No depth from the layer's previous tenant survives (O4).

Draw shape:
- [ ] Abutting reached ranges go out as one draw, and a gap of even one index splits them. A region issues at most one draw per maximal run of reached ranges.
- [ ] A synthetic cell whose leaves aren't contiguous draws each leaf's range, and no index outside its leaves.
- [ ] On compiled levels, face-cut faces included, each cell's leaves form one contiguous index range. A synthetic face-cut fixture proves it in routine tests, and the compiled stress maps prove it on demand.
- [ ] Shadow passes issue no indirect draw and bind no material. No frame records a shadow-cull compute pass, and level install creates no per-region shadow indirect buffer. This is a source and inventory gate, not a behavior test.

Per-region state and ordering:
- [ ] Two regions that draw world in one frame with disjoint frusta each draw their own reach.
- [ ] Two regions whose frusta overlap in one frame, including adjacent faces of one cube light, each draw every cell they share. Neither draws a cell that only the other reached (O1).
- [ ] A spot region and a cube face that share a region number in one frame each draw their own reach (O6).
- [ ] Take one frame holding warm promoted and dynamic regions, a dynamic and a promoted cold fill, an uncached live region whose matrix is unchanged since the last frame, and a promoted light dropped over capacity. Reach is computed exactly once for each cold fill and for the uncached region, and for nothing else. The uncached region draws its reach again on the next frame. A cold fill draws world once, into its cache layer (O5).
- [ ] A cache layer freed and given to a new tenant in the same frame cold-fills with the new tenant's reach, from that frame's matrix. A dynamic light that keeps its matrix but changes pool slot stays warm: it computes no reach and issues no world draw (O3).
- [ ] Across consecutive frames with a moving light, each frame draws the reach of that frame's matrix, including a frame whose reach is a strict subset of the frame before's (O2).
- [ ] A cold fill, dynamic or promoted, draws its reach into its cache layer. The next warm frame computes no reach and issues no world draw. A re-key after warm frames, or a light re-lit after dropping below the brightness gate, draws reach again.
- [ ] After a level install, the first shadow fill uses the new level's BVH and ranges, even when a region's matrix is unchanged.
- [ ] A level with no BVH draws all world geometry in its shadow passes.
- [ ] Installing a level with no BVH after one with a BVH draws all of the new level's world geometry and no range of the old one. A BVH level installed after that draws reach on its first fill. Every range a region issues lies within the installed index buffer (O7).
- [ ] A level installed with a BVH but no per-cell draw index draws each region's reach, as one with the index does (O8).
- [ ] Skinned and rigid occluders still draw into every region that draws them today (regression guard).

Bounds and contract:
- [ ] The walk never descends below a node whose bounds miss the frustum. On a synthetic world, adding cells outside a fixed frustum leaves the walk's visited-node count unchanged once the tree has the same shape inside the frustum.
- [ ] On the same synthetic world, adding cells outside the frustum leaves unchanged the number of cells a region collects and resets after its walk.
- [ ] Every Reach, Draw shape and Bounds row is proven by a test that runs without a GPU adapter. None passes by skipping.
- [ ] After warm-up, building draw lists for any frame allocates nothing, including a frame whose reach is larger than any earlier frame's.
- [ ] The indirect-contract scanner passes. Its camera rules and inventory are unchanged, its shadow entries are gone, and its nested-binding fixtures exercise the camera draw. This is a diff gate and a release-safety review, not a behavior test.

### Manual
Headless capture shows no dynamic-light shadow (`research.md` §Headless capture), so dynamic fills are proved by the oracle rows and a playtest. Its forced-promotion mode does exercise the promoted cold fill.
- [ ] Baseline before any change: release build on this Mac, window in front, machine state recorded per §12. Record on campaign-test, and on the hallway at a pose where the lift light holds a slot and the cache log shows uncached cube fills. For each, record:
  - `[CpuTiming]` `render_submit` and `rec_shadow_depth` medians over at least five windows;
  - a `sample` profile of wgpu-hal Metal `draw_indexed_indirect` time;
  - Metal System Trace GPU time for the shadow-cull and shadow-depth passes.
- [ ] After: on the hallway pose, the `render_submit` median falls. On campaign-test it does not rise. Report what the reach walk itself costs.
- [ ] Metal System Trace after: on both maps, summed GPU time of the shadow-depth passes plus the former shadow-cull pass does not rise.
- [ ] Windows handoff, not a gate: under GPU timing, the shadow-depth spans on the same poses do not rise against the pre-change build.
- [ ] Report the GPU memory freed on the hallway.
- [ ] A forced-promotion headless capture, with an entity receiver in the promoted light's reach, is byte-identical before and after.
- [ ] Visual: compare against the pre-change build. Watch the hallway lift light through a full cycle, and walk a stress-warren-crates room with more than four point lights in reach. No shadow drops, pops or flickers, including at frustum edges and with a light on a cell boundary.

## Path
- Seams: `research.md` §When a slot draws world depth (the filter predicate and the six draw sites), §Rivals (the CPU `BvhTree` clones in `ComputeCullPipeline`), and §Shadow depth pipeline (entity occluders share the pass).
- The walk mirrors `bvh_cull.wgsl` `cull_main`'s depth-first skip order. The box test is `aabb_intersects_frustum` in `render-data/src/cone_frustum.rs`.
- The planner can sit beside `instance_casts_into_cone` in `render-cpu`.
- Reached cells can be collected without scanning every cell: dedupe with a bitset, and reset only the bits the walk set.
- First slice: per-cell leaf groups with the contiguity pin, the CPU walk, and the brute-force oracle on the overhang fixture and the hallway probe. The draw layer comes after.
- Deletion fallout: `research.md` §Deletion.
- Files:
  - `renderer_dynamic_shadow_passes.rs` is near 800 lines; split it first, in its own commit, if this grows it.
  - Keep new logic out of `renderer_light_slots.rs` and `compute_cull.rs`, which are both past the threshold.
- Metal System Trace capture and cleanup: `rendering_pipeline.md` §12 (Without timestamp support).

## Open questions
- Where the reach step runs: the existing filter point, or the end of the light-slot update — **delegated**.
- What the cache plans' cull-dispatch counters become, and what the log says — **delegated**.
