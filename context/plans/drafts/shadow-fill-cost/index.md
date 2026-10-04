# shadow-fill-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §5 (incl. Indirect-args invariant), §7.1 steps 6–8, §12 · `context/lib/development_guide.md` §1.4 · read at 2a4bd9eb3 · evidence: `research.md`

## Problem
A `visible-span-draws` review finding, confirmed in source: a shadow region that redraws world depth costs as much as the whole map, however little its frustum reaches. Cause: both halves of a shadow fill run over every leaf. The world draw covers whole material buckets, which Metal expands into one driver draw per leaf. The cone cull walks the whole BVH on one GPU thread per region. The hallway is the one map over budget on the Mac, and on it the lift carries a point light. On every moving frame that light refills all six cube faces, about 6 × 8,437 driver draws, which extrapolates to about 2.5 ms of CPU encode (unmeasured, `research.md` §Cost model). When this is done, a region's world draws are limited to the cells its frustum reaches. If measurement warrants it, its cull runs one leaf per invocation. Shadows are unchanged.

## Decisions
- **Shadow world draws are limited to reached cells.** Each spot region and cube face that draws world this frame draws only the `CellDrawIndex` spans of the cells its frustum reaches. That covers the uncached live draw, the dynamic cold fill and the promoted cold fill. Spans are coalesced as the camera's are: abutting runs merge, gaps never bridge. This diverges from `rendering_pipeline.md` §5 ("Shadow passes retain whole buckets"), which is updated at promotion.
- **A cell is reached when the union of its leaves' AABBs passes the shadow cull's own test against the same planes.** Baked cell bounds are not used, because a leaf can extend past them (`research.md` §Containment). The CPU may err only toward reaching, with a margin for GPU rounding. So the drawn ranges cover every leaf the cull can submit, which keeps the superset contract (§2, §5).
- **A region's ranges come from its own frustum only,** never from the camera's visible set or fog reach. An occluder the camera can't see still shadows a visible receiver (§7.1 step 6).
- **Ranges are built for exactly the regions whose cull dispatches this frame, from the current matrix.** A warm region builds nothing and draws no world. No region draws ranges built on an earlier frame.
- **An empty reach draws nothing.** It never falls back to whole buckets. A level with no BVH still draws all world geometry, as today.
- **The indirect contract is unchanged.** Offsets and lengths come from load-validated spans, which §5 guarantee 3 already allows. There is no new draw method, and the owner rules and inventory in `indirect_contract_tests` stay as they are. The stale-slot argument holds: a stale slot inside a drawn span is a leaf outside this frame's frustum, and it clips out as it does today.
- **Layer: renderer, runtime.** A GPU-free step turns (planes, per-cell boxes, draw index, bucket ranges) into a region's ranges. It is testable without a GPU, and the draw layer only issues the result. Per-cell boxes are derived from leaf bounds at level install. The PRL, compiler and bake are unchanged.
- **Hot-path bound.** After warm-up, building ranges allocates nothing. Per drawing region it tests each cell box at most once and reads only reached cells' spans (`development_guide.md` §1.4).
- **The flat cull is gated on a measured baseline.**
  - Before any change, take a Metal System Trace of the shadow-cull passes on the hallway with the lift moving.
  - **At 0.3 ms per frame or more:** both pools replace the serial walk with one invocation per leaf. Each invocation writes every leaf slot of a dispatched region, live or zero, and the existing per-region bind groups and per-region dispatch stay.
  - **Below 0.3 ms:** record the number and keep the walk.
- **The spot and cube cull passes get distinct labels and their own GPU-timing entries,** whatever the gate shows. Metal System Trace groups passes by label, and an unmeasured pass is a gap.
- **Non-goals.**
  - Cache capacity, cache keys, and the promoted cache's over-capacity drop.
  - A per-region candidate gather on the GPU (`research.md` §Rivals).
  - Batching regions into one dispatch on z. It reworks the bind layout, so it needs its own plan.
  - Dynamic shadows in headless capture. Capture is static-light-only by design, so this brief proves with superset rows and a playtest instead (`research.md` §Headless capture).
  - The camera fallback's use of baked cell bounds (`research.md` §Adjacent). It is a separate, unverified question.
  - Other backends, where a multi-draw is one native call. Entity and mover occluders, which are already culled on the CPU per region.

## Acceptance

### Automated
Reach and superset:
- [ ] A spot region draws the coalesced spans of exactly the cells whose leaf-bounds box passes its frustum test. Each of a cube light's six faces does the same against its own face frustum.
- [ ] Every leaf the shadow cull can submit for a region lies inside that region's drawn ranges. This holds over randomized frusta on synthetic worlds (fixed seed and count) and at the on-demand stress-map probes, including the hallway lift light's faces at several lift heights.
- [ ] A leaf whose geometry extends past its cell's baked bounds, under a frustum that touches only that overhang, has its cell drawn.
- [ ] A cell whose leaves all fail the frustum test by more than the margin is not drawn. A cell with one passing leaf is drawn.
- [ ] A frustum that reaches no cell issues no world draws and no error, and does not fall back to whole buckets, including on the frame after a nonempty reach. A frustum that reaches every cell draws every drawable leaf, with no more draws than whole buckets.
- [ ] A cell outside the camera's visible set but inside the frustum is drawn. A cell the camera sees but the frustum misses is not.

Per-region state and ordering:
- [ ] Two regions that draw world in one frame with disjoint frusta each draw their own ranges. Recording either one leaves the other's ranges and the camera list unchanged.
- [ ] Across consecutive frames with a moving light, each frame draws ranges built from that frame's matrix.
- [ ] A cold fill, dynamic or promoted, draws restricted ranges into its cache layer. The next warm frame builds no ranges and issues no world draw. A re-key after warm frames draws restricted ranges again.
- [ ] After a level install, the first shadow fill uses the new level's boxes and draw index, even when a region's matrix is unchanged.
- [ ] A level with no BVH draws all world geometry in its shadow passes.
- [ ] Without multi-draw-indirect, the per-draw fallback issues the same ranges one slot at a time.

Bounds and contract:
- [ ] After warm-up, building ranges for any frame allocates nothing.
- [ ] Building a region's ranges tests each cell box at most once, and reads no span of an unreached cell.
- [ ] The indirect-contract scanner passes with its owner rules and inventory unchanged. This is a diff gate, not a behavior test.
- [ ] GPU timing lists distinct spot and cube shadow-cull entries, separate from the camera cull.

Flat cull, only if the gate passes:
- [ ] Per region, the flat cull submits exactly the leaves the serial walk submits, at every probe pose and synthetic frustum, in both pools.
- [ ] After a dispatch, every leaf slot in the region holds a live record or zero, and none keeps an earlier frame's value. Undispatched regions keep their contents.
- [ ] Neither pool still dispatches the serial walk.

### Manual
- [ ] Baseline before any change: release build on this Mac, window in front, machine state recorded per §12. On the hallway with the lift moving, and on campaign-test, record:
  - `[CpuTiming]` `render_submit` and `rec_shadow_depth` medians over at least five windows;
  - a `sample` profile of wgpu-hal Metal `draw_indexed_indirect` time;
  - Metal System Trace GPU time for the spot and cube shadow-cull passes.
- [ ] After span draws, on the hallway with the lift moving: `draw_indexed_indirect` time and the `render_submit` median fall. On campaign-test, `render_submit` does not rise. Report what the reach test itself costs.
- [ ] Gate: record the shadow-cull number and the decision. If the flat cull is built, hallway shadow-cull GPU time falls by at least 70%, and summed GPU pass time on campaign-test rises by no more than 2%.
- [ ] Visual: compare against the pre-change build. Watch the hallway lift light through a full cycle, and walk a stress-warren-crates room with more than four point lights in reach. No shadow drops, pops or flickers, including at frustum edges and with a light on a cell boundary.

## Path
- Seams:
  - `ShadowCullPipeline::dispatch_occupied_slots_filtered`: its filter loop already holds each region's matrix and planes.
  - `ShadowCullPipeline::draw_slot_indirect`: it passes `self.bucket_ranges` today. Per-region range scratch inside the pipeline leaves the six pinned call sites alone.
  - `VisibleSpanRanges::rebuild`: it accepts `VisibleCells::Culled`.
  - `aabb_intersects_frustum` and `cone_frustum_planes` (`render-data/src/cone_frustum.rs`): the CPU test the mover and mesh occluder culls already share with the GPU uniform.
  - Leaf bounds and cell ids reach `install_level_geometry`.
- Shape: flat CPU box tests over per-cell leaf unions. A cube light's faces can first filter cells against the light's range box. Rival: a per-region GPU candidate gather, rejected in `research.md` §Rivals.
- First slice: the reach step and a superset oracle on the overhang fixture and the hallway probe, before touching the draw layer. This falsifies the riskiest assumption, that the CPU never under-reaches the GPU.
- Proof harness: `candidate_cull_probes.rs` (on-demand, `#[ignore]`) gains shadow regions. The flat-cull equivalence needs a CPU mirror of the serial walk, beside `candidate_cull_mirror.rs`.
- Files:
  - `renderer_dynamic_shadow_passes.rs` is near 800 lines; split it first, in its own commit, if this grows it.
  - Keep new logic out of `renderer_light_slots.rs` and `compute_cull.rs`, which are both past the threshold.
- Metal System Trace capture and cleanup: `rendering_pipeline.md` §12 (Without timestamp support).

## Open questions
- The reach-test margin size — **delegated**: the smallest margin that covers fused multiply-add rounding, reported in the plan of record.
- Where the reach step runs: the cull filter loop, or the end of the light-slot update — **delegated**.
