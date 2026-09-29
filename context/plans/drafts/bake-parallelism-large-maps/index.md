# bake-parallelism-large-maps

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Progress reporting, §Build Cache · `context/lib/development_guide.md` §1.4 · read at 4ae37c4af (`feat/lightmap-cell-blocks`)

> Reminder-grade draft. Not validated, not reviewed. Owner picks it up later.

## Problem
Owner-raised, from a 5 h 54 m hallway bake. Parallelism was deferred as YAGNI while maps were small (`research.md` §Prior deferrals). `stress-warren-hallway-inspection.map` breaks that assumption. The base SH bake is 56% of the wall time and already runs at the 14-permit cap, so more threads cannot help it. The rest of the build averages about 3 busy cores, not 14. Three causes, all in code. The lightmap bake is a serial loop over (atlas layer × light), with a fork-join barrier and a serial tail between each pair. Each small cache entry pays a full-device fsync and a create-and-rename in one flat directory, which leaves the delta SH stages idle in syscalls. Stages with no data dependency run one after another. When done, the hallway bake keeps its permits busy outside the base SH bake, and its output bytes are unchanged.

## Decisions
- **Yardstick: the hallway map, under pinned conditions.** The conditions are listed in `research.md` §Measurement conditions. Every before/after number comes from those conditions. Small fixtures prove determinism only; they never prove speed.
- **Output bytes do not change, in either mode.** The cold `--release` bake is the ship source of truth (`development_guide.md` §1.4). Output must not depend on thread count (`build_pipeline.md` §Build Cache, Determinism invariant). Every lever reorders or overlaps work; none changes the math.
- **The governor contract holds.** Each parallel work item enters the governor once, at its outermost boundary. A permitted item never waits on another permitted item. Pause and throttle stay live (`build_pipeline.md` §Progress reporting). Light-axis lightmap parallelism reuses the bounded-window, non-nested pattern designed in `plans/done/shadowmask-bake-scaling` Task 2. That pattern was lost when the fused walk moved shadowmask consumption into the serial lightmap loop.
- **Scope: idle cores and serial sections, not ray count.** Cutting rays per probe or per texel belongs to the `lighting-scale--*` drafts, and BVH traversal cost is an owner question below. This brief does not own the base SH bake's 11,939 s. It owns the roughly 9,300 s around it.
- **Cold and warm share the structure.** The lightmap's serial (layer × light) loop runs in both modes (`bake_fused_prepared`). A lightmap fix must speed up `--release` too, not only the cached path.

## Acceptance
### Automated
- [ ] A `--release` fixture bake is byte-identical before and after, at `-j 1` and at the default `-j`.
- [ ] A warm all-miss bake followed by a warm all-hit bake of the same fixture emits identical `.prl` bytes, and both match the pre-change warm bytes.
- [ ] Lowering permits to 1 mid-lightmap-bake leaves at most one lightmap work item running once in-flight items drain. Pause stops new items within the lightmap bake and within any stage that now overlaps another.
- [ ] Deleting or truncating a cache entry mid-build, or killing the build during a cache write, gives a miss on the next build, never a wrong hit. This guards whatever durability ruling the owner makes.
- [ ] Lightmap, shadowmask, delta-SH, and weight-map stage tests pass unchanged.
### Manual
- [ ] Measured finding: hallway per-stage wall time and average busy cores, before and after, for Lightmap Bake, AnimWeightMaps, ShadowmaskAtlas, and each delta SH stage.
- [ ] Measured finding: hallway total wall time before and after, with the SH Bake line reported separately so its fixed cost does not hide the change.
- [ ] Measured finding: busy cores during Direct SH Delta Bake, against the 0.14-core baseline.

## Path
Ranked by the hallway wall time each lever addresses. Symbols and line numbers are in `research.md`.
1. **Lightmap light axis, about 7,400 s at 4 or fewer cores.** Bake several lights' partitions for one layer at once, then fold them serially in global light order. Keep the per-chart work units. Take the cache put, fold, and shadowmask consume off the ray-work barrier. The cheap companion fixes are:
   - cull each light's chart list by the light's bounds; today every chart is walked for every light;
   - hoist `probe_indices` per light;
   - hoist the layer-invariant part of `layer_input_hash` out of the per-layer loop.
   Rival: keep lights serial and split charts into texel tiles. That fixes the large-chart tail but not the per-partition serial tail.
2. **Cache write path: delta SH stages at about 650 s, plus tax inside every cached stage.** Each put does a create, several unbuffered writes, an `F_FULLFSYNC`, and a rename in a directory of about 142k entries. Each hit also opens the file a second time to touch it. The owner deferred this fsync tax in `perf-parallel-sh-group-bake`; on this map it is the whole cost of Direct SH Delta.
3. **Overlap independent stages, up to about 1,000 s.** AnimLightChunks and AnimWeightMaps need only atlas placement, not lightmap output. They are blocked by an unused `&mut geometry` borrow. The five SH-family stages do not read each other's output. Overlap mostly helps the I/O-bound delta stages.
4. **Serial setup redone per stage.** Candidates:
   - `probe_grid_layout` BSP queries, once per SH stage;
   - affinity decomposition, recomputed by each delta bake after the plan already ran it;
   - a scan over every geometry vertex per delta entry;
   - an O(n²) chunk-overlap assert that runs in release;
   - serial BC6H, BC5, and shadowmask coloring.
   None is measured. Profile before cutting.

First slice: lever 1 on the lightmap bake alone, proven byte-identical, then one hallway lightmap timing. It carries the largest share of wall time and the most determinism risk.

## Open questions
- Cache durability: drop the per-entry fsync (the cache is disposable, and `get` already verifies blake3), batch it once per stage, or keep it? — owner — **blocks build** (lever 2)
- Cache budget: the default 2 GiB is smaller than the hallway's live set. The next build's LRU prune evicts the oldest entries, which are the SH groups and delta entries, so every hallway rebake re-bakes the SH family (about 3.6 h) while later-written lightmap and weight-map entries can still hit. Should the default scale, grow, or stay while the warning gets louder? — owner — **blocks build** for any warm-path claim
- Stage overlap: is concurrent execution of two stages acceptable under a TUI that highlights one foreground stage, and does its progress stay honest? — owner — **blocks build** (lever 3)
- BVH traversal: every ray uses the unordered, unbounded `traverse_iterator`. That is the base SH bake's largest per-ray cost, and it cannot be parallelized away. Own it here, or in its own brief? — owner — **blocks build** only if folded in
- Bake binary profile: the dev profile builds the compiler crate at `opt-level = 1`, and the live rebake ran `target/debug/prl-build`. Pin the yardstick to a release binary, raise the compiler's dev opt-level, or both? — owner — **blocks build** (measurement conditions)
- Should this be one brief or three, split by lever? — owner — **delegated** once the three rulings above land
- Default `-j` is 14 permits on 8 physical cores and 16 logical. Measure whether a lower count is as fast on hyperthreaded hosts — **delegated**
