# bake-parallelism-large-maps

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Progress reporting, §Build Cache · `context/lib/development_guide.md` §1.4 · read at 329fbe07b (`feat/lightmap-cell-blocks`)

> The owner resolved the open questions in a draft session on 2026-09-29. Not yet validated (`/validate-plan`).

## Problem
Owner-raised, from a 5 h 54 m hallway bake. Parallelism was deferred while maps were small (`research.md` §Prior deferrals). `stress-warren-hallway-inspection.map` breaks that assumption. The base SH bake is 56% of the wall time and already runs at the 14-permit cap, so more threads cannot help it; cheaper rays can. The rest of the build averages about 3 busy cores, not 14. The causes are all in code. The lightmap bake is a serial loop over (atlas layer × light), with a fork-join barrier and a serial tail between each pair. Each small cache entry pays a full-device fsync and a create-and-rename in one flat directory, which leaves the delta SH stages idle in syscalls. Stages with no data dependency run one after another. Every ray walks the BVH unbounded and unordered, testing geometry past its segment end or behind its nearest hit. When done, the hallway bake keeps its permits busy outside the base SH bake, every bake does less traversal work per ray, and output bytes are unchanged.

## Decisions
- **Yardstick: the hallway map, under pinned conditions.** The conditions are in `research.md` §Measurement conditions, and every before/after number comes from them. The binary is a cargo `--release` build of `prl-build` running warm (cache enabled); cargo's release profile is not `prl-build --release`, which is the cold, uncached ship bake. Small fixtures prove determinism only; the dev profile's opt-level stays as is.
- **Output bytes do not change, in either mode.** The cold `--release` bake is the ship source of truth (`development_guide.md` §1.4). Output must not depend on thread count (`build_pipeline.md` §Build Cache, Determinism invariant). Every lever reorders, overlaps, or prunes work; none changes the math.
- **Scope: idle cores, serial sections, and per-ray traversal cost; not ray count.** Cutting rays per probe or per texel belongs to the `lighting-scale--*` drafts. Traversal is the only lever that touches the base SH bake's 11,939 s; the rest target the roughly 9,300 s around it. A nearest-hit query returns the same hit it does today, ties included, and byte identity proves it.
- **The governor contract holds.** Each parallel work item enters the governor once, at its outermost boundary. A permitted item never waits on another permitted item. Pause and throttle stay live (`build_pipeline.md` §Progress reporting). Light-axis lightmap parallelism reuses the bounded-window, non-nested pattern from `plans/done/shadowmask-bake-scaling` Task 2, which only a test-reached path keeps since the fused walk moved shadowmask consumption into the serial lightmap loop.
- **Stages may overlap.** `build_pipeline.md` §Progress reporting already lets concurrent lifecycle stages advance together with exactly one foreground stage; the fused Lightmap + ShadowmaskAtlas bake is the precedent. An overlapped stage begins as a background stage, and every stage shares the one global governor.
- **Atlas preparation stays after the SH family.** `build_pipeline.md` §Atlas preparation and SH ordering calls that order load-bearing: Direct SH Delta can clear entity-shadow selection, and deterministic channel assignment must finish before the fused atlas walk.
- **Cold and warm share the structure.** The lightmap's serial (layer × light) loop runs in both modes (`bake_fused_prepared`). A lightmap fix must speed up `--release` too, not only the cached path.
- **Cache writes drop the per-entry `sync_all`; the cache is otherwise unchanged.** `build_pipeline.md` §Build Cache calls the cache disposable, `get` verifies blake3, and the temp file plus atomic rename already turn a killed or torn write into a miss. Only power loss is affected, and it also yields a miss.
- **The default `--cache-max-size` rises so the hallway's live set fits with headroom.** The observed live set is at least 5.2 GB; `build_pipeline.md` §Build Cache is updated where it states the 2 GiB default, in the same change. This supersedes the 2 GiB sizing in `plans/done/lighting-scale--sparse-layer-cache-and-fused-walk`, which measured campaign-test layers only.
- **Builds after `spatial-residency--lightmap-cell-blocks` lands on main.** Code refs were read on `feat/lightmap-cell-blocks`, whose compiler diff against main is large, and byte-identity baselines are taken post-cell-blocks. Cell blocks keeps the bake's layer loops, so lever 1's premise survives.

## Acceptance
### Automated
- [ ] A cold `--release` fixture bake is byte-identical before and after, at `-j 1` and at the default `-j`.
- [ ] A warm all-miss bake followed by a warm all-hit bake of the same fixture emits identical `.prl` bytes, and both match the pre-change warm bytes.
- [ ] For rays aimed exactly at an edge two triangles share, the nearest-hit query returns the same distance and normal as today's full scan. Occlusion queries give today's answer for segments ending just short of, exactly at, and past a blocker.
- [ ] Lowering permits to 1 mid-lightmap-bake leaves at most one lightmap work item running once in-flight items drain. Pause stops new items within the lightmap bake and within any stage that now overlaps another.
- [ ] Two stages that previously ran one after another now run together: exactly one stage holds foreground throughout, and the background stage's progress advances before the foreground stage finishes.
- [ ] Regression guard for dropping the fsync (passes today): deleting or truncating a cache entry, or killing the build mid-write, gives a miss on the next build, never a wrong hit.
- [ ] Lightmap, shadowmask, delta-SH, and weight-map stage tests pass unchanged.
### Manual
- [ ] Measured finding: hallway per-stage wall time and average busy cores, before and after, for Lightmap Bake, AnimWeightMaps, ShadowmaskAtlas, and each delta SH stage.
- [ ] Measured finding: hallway SH Bake wall time before and after the traversal change alone, so the other levers can neither claim nor hide it.
- [ ] Measured finding: hallway total wall time before and after.
- [ ] Measured finding: busy cores during Direct SH Delta Bake, against the 0.14-core baseline.
- [ ] A hallway build under the new default budget emits no live-set budget warning, and a second unchanged build hits the SH group, Delta SH, and Direct SH Delta entries instead of re-baking them.

## Path
Levers keep their labels. They are listed by the hallway wall time each addresses. Symbols and derivation are in `research.md`.
- **Lever 5: BVH traversal, the base SH bake's 11,939 s plus every other bake's rays.** Every site hands the stock `traverse_iterator` an infinite ray. The one closest-hit site, `sh_bake::closest_hit`, tests every leaf that ray crosses. Each occlusion `segment_clear` exits on its first hit but still visits leaves past the segment end, so a clear segment tests everything behind the light. Shape: a caller-defined query on the same iterator that also rejects nodes entered beyond the segment end, or beyond the best hit so far. Visit order is unchanged, so today's tie winner is too. Rival: `nearest_traverse_iterator`, which reorders visits (ties could resolve differently), allocates a heap per ray, and exposes no node distance to stop on. The saving is unmeasured; profile one hallway SH group first.
- **Lever 1: lightmap light axis, about 7,400 s at 4 or fewer cores.** Bake several lights' partitions for one layer at once, then fold them serially in global light order; `IncrementalLayerAccumulator::fold_partition` documents that order as the determinism guarantee. Keep the per-chart work units. Take the cache put, fold, and shadowmask consume off the ray-work barrier, in the main loop and in the lightmap-hit, shadowmask-miss loop. `bake_shadowmask_atlas_with_window` is a reuse candidate for the bounded window. Cheap companions:
  - cull each light's chart list by the light's bounds; today every chart is walked for every light;
  - build each layer's face list once, not per (layer, light) in `bake_light_layer_controlled`;
  - hoist `probe_indices` per light;
  - compute `atlas_layout_fingerprint` once per bake and the light-only hash parts once per light, and skip the `layer_input_hash` pre-pass when there is no cache.

  Rival: keep lights serial and split charts into texel tiles. That fixes the large-chart tail but not the per-partition serial tail.
- **Lever 2: cache write path, delta SH stages at about 650 s plus tax inside every cached stage.** Beyond the fsync, each put does a create, several unbuffered writes, and a rename in a directory of about 142k entries. Each hit opens the file a second time in `touch_for_lru`. On this map that is the whole cost of Direct SH Delta.
- **Lever 3: overlap independent stages, up to about 1,000 s.** The five SH-family stages overlap one another. Their one join point is Direct SH Delta's post-bake usability check against Direct SH's output; the delta plans and entity-shadow selection come from `plan_delta_bakes` before SH starts. AnimLightChunks and AnimWeightMaps overlap the fused lightmap walk: they read atlas placement and the BVH. Their one read of lightmap output is `lightmap_section.blocks.is_empty()` in `layout_animated_atlas`, which must be derived before the walk or taken at a join after it. Otherwise only an unused `&mut geometry` borrow in `bake_fused_prepared` blocks them. Atlas preparation does not move. Overlap mostly helps the I/O-bound delta stages.
- **Lever 4: serial setup redone per stage.** Candidates:
  - `probe_grid_layout` BSP queries, once per SH stage;
  - affinity decomposition, recomputed by each delta bake after the plan already ran it;
  - a scan over every geometry vertex per delta entry;
  - an O(n²) chunk-overlap assert that runs in release;
  - serial BC6H, BC5, and shadowmask coloring.

  None is measured. Profile before cutting.

First slice: lever 1 on the lightmap bake alone, proven byte-identical, then one hallway lightmap timing. It carries the largest measured idle-core share and the most determinism risk.

## Open questions
- One brief, or split by lever? — **delegated**
- Default `-j` is 14 permits on 8 physical cores and 16 logical. Measure whether a lower count is as fast on hyperthreaded hosts. — **delegated**
- The exact default `--cache-max-size`. — **delegated**: size it from the live set the end-of-build budget warning reports, with headroom.
