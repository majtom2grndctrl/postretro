# shadow-fill-cost — plan of record

mode: compact
status: active
read at: 94cea2ea7

No renderer, render-cpu, render-data, lighting, level-compiler or level-format source changed between the brief's `a6a67bcce` and `94cea2ea7`, so the grounded Decision reads stand.

## Corrections
- Brief (§When a slot draws world depth, O5) lists three world-drawing regions: uncached live, dynamic cold fill, promoted cold fill. → In current source a fourth region draws world. A promoted light whose record the promoted cache drops over capacity (`apply_promoted_cache_layers`) keeps its pool slot and matrix. It has no promoted plan, and it is excluded from the dynamic cache inputs. So `record_*_shadow_depth` sends it down the uncached branch, which redraws world into a live layer that nothing samples, since the dropped record's weight is 0. → Planning around it: a per-slot `slot_promoted` flag beside `slot_entity_eligible` lets the region classifier name this case `Dropped`. A dropped region clears its live layer and keeps today's entity draws, but walks no reach and draws no world. This is the O5 contract as written. It doesn't change when the cache drops a light (non-goal), only what the dropped slot wastes.
- Acceptance "Every Reach, Draw shape and Bounds row is proven by a test that runs without a GPU adapter" vs the Reach rows that say "in a recorded frame" (rows R7, R8; R9's clear) → a recorded frame needs the production recorder and so an adapter. Clarification, same meaning: each of those rows gets an adapter-free proof of the same predicate (planner, classifier and draw-site glue, which take no camera input), plus an adapter-backed recorded-frame proof that runs on this Mac. No row's only proof is a test that can skip.
- Path: "The planner can sit beside `instance_casts_into_cone`" → it gets its own module `render-cpu/src/shadow_reach.rs`, because `mesh_instances.rs` is 1,317 lines. Same crate and layer.
- Path: "`renderer_dynamic_shadow_passes.rs` is near 800 lines; split it first if this grows it" → this change shrinks it: the dispatch block goes and six draw arms collapse into one glue call. Classifier and glue land in a new `render/shadow_world_draws.rs`, so no split commit is needed.

## Delegated answers
- Where the reach step runs — at the existing filter point, `record_spot_shadow_depth` / `record_cube_shadow_depth`. It runs lazily, at each region's world pass, from the same matrix the VS uniform uploaded this frame. The cache plans have already decided which regions draw world by then. Each region's ranges are issued before the next walk, so one renderer-owned scratch serves every region, and its worst-case capacity is one region reaching the whole map. That keeps every frame allocation-free, including a frame with a record reach. Running the step at the end of the light-slot update would hold up to 132 region lists alive at worst-case capacity each.
- Cull-dispatch counters — `should_dispatch_{spot,cube}_cull`, `skipped_{spot,cube}_cull_dispatches`, `promoted_depth_cache_cull_dispatch_skips` and its caller-less accessor are deleted. The record loops read `needs_world_render` through the region classifier instead. `DynamicCacheCounters::cull_dispatch_skips` becomes `reach_walks`, counting every spot region and cube face that walked reach this frame, whatever its tier. The 120-frame log line reads `world-pass skips {:.2}, reach walks {:.2}`. Also, a new CPU substage `rec_shadow_reach` under `rec_shadow_depth` times the walks, which is the surface for "report what the reach walk itself costs".

## AC-to-proof

Unit tests are adapter-free unless marked *adapter*. Adapter tests run on this Mac's Metal adapter.

| AC | Proof | Status |
|---|---|---|
| R1 spot region and each cube face draw exactly the cells owning a leaf in their frustum | `render-cpu` `shadow_reach` tests (spot) · renderer `shadow_world_draws` test over real `cube_face_matrices` | achievable as stated |
| R2 walk == brute force, both directions, randomized synthetic + stress probes incl. lift faces at several heights | `render-cpu` seeded randomized oracle · on-demand `#[ignore]` `shadow_reach_probes` in `postretro` (hallway, crates, campaign; lift light faces via `cube_face_matrices`) | achievable as stated |
| R3 overhang leaf under a frustum touching only the overhang draws its cell | `render-cpu` overhang fixture | achievable as stated |
| R4 all-outside cell not drawn; one-inside cell drawn | `render-cpu` test | achievable as stated |
| R5 empty reach → no draws, no error, incl. after nonempty; nonempty after empty draws in full; all-reach draws every drawable leaf's indices once | `render-cpu` shared-scratch sequence test | achievable as stated |
| R6 cell outside camera set but in frustum drawn; camera-seen cell missed not drawn | renderer adapter-free test contrasting `VisibleSpanRanges` with the region's ranges on one world | achievable as stated |
| R7 recorded frame: out-of-PVS, out-of-fog cell in frustum drawn; camera/fog cell missed not drawn | *adapter* `visible_span_frame_tests` shadow trace · adapter-free half = R6 (glue takes no camera or fog input) | achievable (see Corrections) |
| R8 recorded frame on BVH level draws only reach, never whole index buffer; replaces the whole-bucket assertion | *adapter* rewrite of `headless_level_reinstall_and_occupied_shadow_…` · adapter-free half = R1 | achievable (see Corrections) |
| R9 empty-reach cold fill / uncached clears to far, cache warms, no prior depth survives | *adapter* trace: world pass recorded with `Clear(1.0)` and zero draws, warm next frame · adapter-free: classifier marks the cold fill as drawing world independent of reach | achievable (see Corrections) |
| D1 abutting ranges merge, a 1-index gap splits, ≤ one draw per maximal run | `render-cpu` test | achievable as stated |
| D2 noncontiguous synthetic cell draws each leaf range and nothing else | `render-cpu` test | achievable as stated |
| D3 compiled levels: each cell's leaves one contiguous range; synthetic face-cut fixture routine, stress maps on demand | `level-compiler` (`prl-build` bin) face-cut pin test · on-demand `shadow_reach_probes` contiguity check | achievable as stated |
| D4 no indirect shadow draw, no material bind, no shadow-cull compute pass, no per-region indirect buffer | source gate: `shadow_cull.rs` deleted, indirect-contract inventory, uploads inventory, grep | achievable as stated |
| P1 two disjoint regions in one frame draw their own reach | renderer adapter-free glue test, shared scratch | achievable as stated |
| P2 O1 overlapping regions incl. adjacent cube faces | renderer adapter-free glue test, real face matrices | achievable as stated |
| P3 O6 spot and cube face with the same region number | renderer adapter-free glue test, spot then cube order | achievable as stated |
| P4 O5 mixed frame: reach once per cold fill and the uncached region, nothing else; uncached redraws next frame; cold fill draws once into its cache layer | renderer adapter-free classifier test over real `plan_frame` outputs | achievable as stated |
| P5 O3 freed layer re-tenanted cold-fills from that frame's matrix; slot move with the same matrix stays warm | renderer adapter-free plan + classifier sequence | achievable as stated |
| P6 O2 moving light draws each frame's own reach incl. strict subset | renderer adapter-free glue sequence | achievable as stated |
| P7 cold fill → warm skips reach → re-key / re-light draws again | renderer adapter-free plan + classifier sequence | achievable as stated |
| P8 first fill after install uses the new BVH and ranges, even with an unchanged matrix | *adapter* reinstall test · structural: index rebuilt at install, scratch reset | achievable as stated |
| P9 no-BVH level draws all world geometry | renderer adapter-free glue test (no index → `0..index_count`) | achievable as stated |
| P10 O7 BVH → no-BVH → BVH level switch | *adapter* install sequence test, ranges within installed index buffer | achievable as stated |
| P11 O8 BVH without per-cell draw index draws reach | renderer adapter-free (index built from leaves only) · *adapter* install with the draw index absent | achievable as stated |
| P12 skinned and rigid occluders still draw into every region they do today | existing occluder tests rerun · diff review: every entity branch keeps its gates; a dropped promoted slot keeps today's uncached entity draw | achievable as stated |
| B1 walk never descends below a rejected node; visited-node count unchanged by outside cells | `render-cpu` test | achievable as stated |
| B2 collected and reset cell counts unchanged by outside cells | `render-cpu` test | achievable as stated |
| B3 every Reach, Draw shape, Bounds row has an adapter-free test | this table (see Corrections) | achievable (see Corrections) |
| B4 building draw lists allocates nothing after warm-up, incl. a record reach | renderer `building_draw_lists_after_warmup_allocates_nothing_even_for_a_record_reach` (renderer already installs the counting allocator; render-cpu has none) | achievable as stated |
| B5 indirect-contract scanner passes; camera rules and inventory unchanged; shadow entries gone; nested-binding fixtures on the camera draw | `indirect_contract_tests` + diff review | achievable as stated |
| M1 baseline (pre-change build) on campaign-test and hallway lift pose | owner, in-engine (pre-change build from `main`) | manual |
| M2 hallway `render_submit` falls; campaign does not rise; report walk cost (`rec_shadow_reach`) | owner, in-engine | manual |
| M3 Metal System Trace: shadow-depth + former shadow-cull GPU time does not rise | owner, in-engine | manual |
| M4 Windows GPU-timing handoff | owner, Windows | manual (not a gate) |
| M5 GPU memory freed on the hallway | computed from the deleted buffer sizes + install log | manual |
| M6 forced-promotion headless capture byte-identical before/after | capture A/B | manual |
| M7 visual: hallway lift cycle, crates room with > 4 point lights | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | `render-cpu::shadow_reach`: per-cell leaf groups from loaded leaves, CPU BVH walk with bitset dedupe, merged ranges, brute-force oracle; tests R1–R5, D1, D2, B1, B2 | integrating executor | — | done: 10 tests in `shadow_reach_tests.rs`; mutation (no subtree skip) fails the bound test |
| 2 | `level-compiler` cell-major contiguity pin over a synthetic face-cut fixture (D3 routine) | worker | — | done: `atlas_stage::tests::face_cut_bvh_leaves_keep_each_cell_one_contiguous_index_range` (runs the real cut → `rebuild_face_identity` → BVH rebuild chain) |
| 3 | Renderer: delete `shadow_cull.rs` and its wiring; `shadow_world_draws.rs` classifier + glue; `slot_promoted`; counters, log, `rec_shadow_reach`; install/release; dead accessors; stale comments | integrating executor | 1 | done |
| 4 | Indirect-contract and uploads inventory updates; nested-binding fixtures on the camera draw (D4, B5) | integrating executor | 3 | done: `indirect_contract` 11 tests pass |
| 5 | Renderer adapter-free tests R6, P1–P7, P9, P11 and adapter tests R7–R9, P8, P10–P12 | integrating executor | 3 | done: 10 in `shadow_world_draws_tests.rs`, 4 adapter in `shadow_world_frame_tests.rs` (ran on Metal); P12 = existing occluder tests + diff review (entity gates unchanged) |
| 6 | On-demand stress probes: contiguity + walk == brute force incl. lift faces (R2, D3) | worker | 1 | |
| 7 | Preflight, review panel, fix loop; `rendering_pipeline.md` §12 counter wording | integrating executor | 2–6 | |
