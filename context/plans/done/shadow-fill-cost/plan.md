# shadow-fill-cost — plan of record

mode: compact
status: done (landed 2026-10-04; M7 visual and M4 Windows handoff outstanding, M2 campaign waiver requested)
read at: 94cea2ea7

No renderer, render-cpu, render-data, lighting, level-compiler or level-format source changed between the brief's `a6a67bcce` and `94cea2ea7`, so the grounded Decision reads stand.

## Corrections
- Brief O5 includes "a promoted light dropped over capacity" among the mixed frame's regions. → That light holds no region at all. When `apply_promoted_cache_layers` drops a record it zeroes the light's weight, and `clear_zero_weight_promoted_assignments` then frees its pool slot, before any slot matrix is written. This was already true before this change. → O5's dropped leg holds by construction. `missing_cache_plan_layer_drops_record_and_zeros_weight_before_metadata_pack` now chains drop → zero weight → `NO_SHADOW_SLOT`. *Revised after review:* an earlier version of this Correction claimed the dropped light kept its slot and redrew world on the uncached path. It added a `slot_promoted` flag and a `Dropped` region kind for that case. The review panel's per-frame tracer showed the case never arises, so both were removed. The owner's direction from that discussion still stands (2026-10-04): cutting shadow-pool work further is the right direction when players are unlikely to notice.
- Acceptance "Every Reach, Draw shape and Bounds row is proven by a test that runs without a GPU adapter" vs the Reach rows that say "in a recorded frame" (rows R7, R8; R9's clear) → a recorded frame needs the production recorder and so an adapter. Clarification, same meaning: each of those rows gets an adapter-free proof of the same predicate (planner, classifier and draw-site glue, which take no camera input), plus an adapter-backed recorded-frame proof that runs on this Mac. No row's only proof is a test that can skip.
- Path: "The planner can sit beside `instance_casts_into_cone`" → it gets its own module `render-cpu/src/shadow_reach.rs`, because `mesh_instances.rs` is 1,317 lines. Same crate and layer.
- Path: "`renderer_dynamic_shadow_passes.rs` is near 800 lines; split it first if this grows it" → this change shrinks it: the dispatch block goes and six draw arms collapse into one glue call. Classifier and glue land in a new `render/shadow_world_draws.rs`, so no split commit is needed.

## Delegated answers
- Where the reach step runs — at the existing filter point, `record_spot_shadow_depth` / `record_cube_shadow_depth`. It runs lazily, at each region's world pass, from the same matrix the VS uniform uploaded this frame. The cache plans have already decided which regions draw world by then. Each region's ranges are issued before the next walk, so one renderer-owned scratch serves every region, and its worst-case capacity is one region reaching the whole map. That keeps every frame allocation-free, including a frame with a record reach. Running the step at the end of the light-slot update would hold up to 132 region lists alive at worst-case capacity each.
- Cull-dispatch counters (amended in review: `reach_walks` counts only BVH walks; a no-BVH whole-buffer draw is not a walk and opens no `rec_shadow_reach` scope) — `should_dispatch_{spot,cube}_cull`, `skipped_{spot,cube}_cull_dispatches`, `promoted_depth_cache_cull_dispatch_skips` and its caller-less accessor are deleted. The record loops read `needs_world_render` through the region classifier instead. `DynamicCacheCounters::cull_dispatch_skips` becomes `reach_walks`, counting every spot region and cube face that walked reach this frame, whatever its tier. The 120-frame log line reads `world-pass skips {:.2}, reach walks {:.2}`. Also, a new CPU substage `rec_shadow_reach` under `rec_shadow_depth` times the walks, which is the surface for "report what the reach walk itself costs".

## AC-to-proof

Unit tests are adapter-free unless marked *adapter*. Adapter tests run on this Mac's Metal adapter.

| AC | Proof | Status | Result |
|---|---|---|---|
| R1 spot region and each cube face draw exactly the cells owning a leaf in their frustum | `render-cpu` `shadow_reach` tests (spot) · renderer `shadow_world_draws` test over real `cube_face_matrices` | achievable as stated | pass |
| R2 walk == brute force, both directions, randomized synthetic + stress probes incl. lift faces at several heights | `render-cpu` seeded randomized oracle · on-demand `#[ignore]` `render/shadow_reach_probes.rs` in the renderer (hallway, campaign-test, stress-warren-mini; lift light faces via `cube_face_matrices`) | achievable as stated | pass |
| R3 overhang leaf under a frustum touching only the overhang draws its cell | `render-cpu` overhang fixture | achievable as stated | pass |
| R4 all-outside cell not drawn; one-inside cell drawn | `render-cpu` test | achievable as stated | pass |
| R5 empty reach → no draws, no error, incl. after nonempty; nonempty after empty draws in full; all-reach draws every drawable leaf's indices once | `render-cpu` shared-scratch sequence test | achievable as stated | pass |
| R6 cell outside camera set but in frustum drawn; camera-seen cell missed not drawn | renderer adapter-free test contrasting `VisibleSpanRanges` with the region's ranges on one world | achievable as stated | pass |
| R7 recorded frame: out-of-PVS, out-of-fog cell in frustum drawn; camera/fog cell missed not drawn | *adapter* `shadow_world_frame_tests::recorded_frame_draws_reach_beyond_camera_and_fog_and_only_reach` · adapter-free half = R6 (glue takes no camera or fog input) | achievable (see Corrections) | pass |
| R8 recorded frame on BVH level draws only reach, never whole index buffer; replaces the whole-bucket assertion | *adapter* `recorded_frame_draws_reach_beyond_camera_and_fog_and_only_reach` (`assert_ne!` the whole buffer). The old whole-bucket assertion in `headless_level_reinstall_and_occupied_shadow_…` now asserts direct reach draws with no indirect slots; its fixture's leaves share one box, so reach covers them all. Adapter-free half = R1 | achievable (see Corrections) | pass |
| R9 empty-reach cold fill / uncached clears to far, cache warms, no prior depth survives | *adapter* trace: world pass recorded with `Clear(1.0)` and zero draws, warm next frame · adapter-free: classifier marks the cold fill as drawing world independent of reach | achievable (see Corrections) | pass |
| D1 abutting ranges merge, a 1-index gap splits, ≤ one draw per maximal run | `render-cpu` test | achievable as stated | pass |
| D2 noncontiguous synthetic cell draws each leaf range and nothing else | `render-cpu` test | achievable as stated | pass |
| D3 compiled levels: each cell's leaves one contiguous range; synthetic face-cut fixture routine, stress maps on demand | `level-compiler` (`prl-build` bin) face-cut pin test · on-demand `shadow_reach_probes` contiguity check | achievable as stated | pass |
| D4 no indirect shadow draw, no material bind, no shadow-cull compute pass, no per-region indirect buffer | source gate: `shadow_cull.rs` deleted, indirect-contract inventory, uploads inventory, grep | achievable as stated | pass |
| P1 two disjoint regions in one frame draw their own reach | renderer adapter-free glue test, shared scratch | achievable as stated | pass |
| P2 O1 overlapping regions incl. adjacent cube faces | renderer adapter-free glue test, real face matrices | achievable as stated | pass |
| P3 O6 spot and cube face with the same region number | renderer adapter-free glue test, spot then cube order | achievable as stated | pass |
| P4 O5 mixed frame: reach once per cold fill and the uncached region, nothing else; uncached redraws next frame; cold fill draws once into its cache layer | renderer adapter-free classifier test over real `plan_frame` outputs (the recorder's cache branches call the same `draws_world`) · dropped leg: light-slot test chaining drop → zero weight → no slot | achievable as stated | pass |
| P5 O3 freed layer re-tenanted cold-fills from that frame's matrix; slot move with the same matrix stays warm | renderer adapter-free plan + classifier sequence | achievable as stated | pass |
| P6 O2 moving light draws each frame's own reach incl. strict subset | renderer adapter-free glue sequence | achievable as stated | pass |
| P7 cold fill → warm skips reach → re-key / re-light draws again | renderer adapter-free plan + classifier sequence | achievable as stated | pass |
| P8 first fill after install uses the new BVH and ranges, even with an unchanged matrix | *adapter* reinstall test · structural: index rebuilt at install, scratch reset | achievable as stated | pass |
| P9 no-BVH level draws all world geometry | renderer adapter-free glue test (no index → `0..index_count`) | achievable as stated | pass |
| P10 O7 BVH → no-BVH → BVH level switch | *adapter* install sequence test, ranges within installed index buffer | achievable as stated | pass |
| P11 O8 BVH without per-cell draw index draws reach | renderer adapter-free (index built from leaves only) · *adapter* install with the draw index absent | achievable as stated | pass |
| P12 skinned and rigid occluders still draw into every region they do today | existing occluder tests rerun · diff review: every entity branch keeps its gates and filters | achievable as stated | pass: existing occluder tests + diff review; no new behavior test |
| B1 walk never descends below a rejected node; visited-node count unchanged by outside cells | `render-cpu` test | achievable as stated | pass |
| B2 collected and reset cell counts unchanged by outside cells | `render-cpu` test | achievable as stated | pass |
| B3 every Reach, Draw shape, Bounds row has an adapter-free test | this table (see Corrections) | achievable (see Corrections) | pass: every Reach/Draw/Bounds row has an adapter-free test |
| B4 building draw lists allocates nothing after warm-up, incl. a record reach | renderer `building_draw_lists_after_warmup_allocates_nothing_even_for_a_record_reach` (renderer already installs the counting allocator; render-cpu has none) | achievable as stated | pass |
| B5 indirect-contract scanner passes; camera rules and inventory unchanged; shadow entries gone; nested-binding fixtures on the camera draw | `indirect_contract_tests` + diff review | achievable as stated | pass |
| M1 baseline (pre-change build) on campaign-test and hallway lift pose | owner, in-engine (pre-change build from `main`) | manual | done (`measurements/shadow-fill-cost/`): hallway lift pose 32.18 ms frame, `render_submit` 5.55, `rec_shadow_depth` 0.097, HAL `draw_indexed_indirect` 1.80 ms/frame, shadow GPU 6.57 ms/frame; campaign 17.05 / 3.43 / 0.050 / 0.04 / 0.21; kinematic spawn and station recorded too. The 120-frame cache log needs GPU timing, which this adapter lacks; substitute evidence: 4.4–4.8 cube world fills per frame in the traces, reach walked in 78–94 of 120 frames. Machine state recorded per batch |
| M2 hallway `render_submit` falls; campaign does not rise; report walk cost (`rec_shadow_reach`) | owner, in-engine | manual | hallway pass: `render_submit` 5.55 → 3.86 ms, frame 32.2 → 26.2 ms. Kinematic spawn and station also fall (−0.33 / −0.29 ms). Reach walk 0.047 / 0.025 / 0.014 ms. **Campaign leg not met as worded:** `render_submit` 3.43 → 3.48 ms (+1.4%, runs don't overlap, cause not established) — **owner waiver requested** |
| M3 Metal System Trace: shadow-depth + former shadow-cull GPU time does not rise | owner, in-engine | manual | pass: shadow GPU per frame 6.57 → 0.77 (hallway), 0.21 → 0.16 (campaign), 1.56 → 0.95 and 1.58 → 1.29 (kinematic spawn and station) |
| M4 Windows GPU-timing handoff | owner, Windows | manual (not a gate) | outstanding (Windows handoff, not a gate) |
| M5 GPU memory freed on the hallway | computed from the deleted buffer sizes + install log | manual | computed, not measured: ≈21.4 MiB freed on the hallway (168,960 B × 132 regions of indirect args + 67 KB status scratch + 32 KB all-ones masks + 12.7 KB uniforms, at 8,437 leaves) |
| M6 forced-promotion headless capture byte-identical before/after | capture A/B | manual | pass: spawner-test `alarm_light` with prop receiver, forced w = 0 / 0.5 / 1.0. PNG bytes identical before/after, and the after build repeats exactly. Negative control (reach forced empty) changes 21 px at w = 0.5 and 1, none at w = 0, so the scene does see promoted world depth (`measurements/shadow-fill-cost/capture/`) |
| M7 visual: hallway lift cycle, crates room with > 4 point lights | owner, in-engine | manual | outstanding (owner) |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | `render-cpu::shadow_reach`: per-cell leaf groups from loaded leaves, CPU BVH walk with bitset dedupe, merged ranges, brute-force oracle; tests R1–R5, D1, D2, B1, B2 | integrating executor | — | done: 10 tests in `shadow_reach_tests.rs`; mutation (no subtree skip) fails the bound test |
| 2 | `level-compiler` cell-major contiguity pin over a synthetic face-cut fixture (D3 routine) | worker | — | done: `atlas_stage::tests::face_cut_bvh_leaves_keep_each_cell_one_contiguous_index_range` (runs the real cut → `rebuild_face_identity` → BVH rebuild chain) |
| 3 | Renderer: delete `shadow_cull.rs` and its wiring; `shadow_world_draws.rs` classifier + glue; `slot_promoted`; counters, log, `rec_shadow_reach`; install/release; dead accessors; stale comments | integrating executor | 1 | done |
| 4 | Indirect-contract and uploads inventory updates; nested-binding fixtures on the camera draw (D4, B5) | integrating executor | 3 | done: `indirect_contract` 11 tests pass |
| 5 | Renderer adapter-free tests R6, P1–P7, P9, P11 and adapter tests R7–R9, P8, P10–P12 | integrating executor | 3 | done: 10 in `shadow_world_draws_tests.rs`, 4 adapter in `shadow_world_frame_tests.rs` (ran on Metal); P12 = existing occluder tests + diff review (entity gates unchanged) |
| 6 | On-demand stress probes: contiguity + walk == brute force incl. lift faces (R2, D3) | worker | 1 | done: `render/shadow_reach_probes.rs` (2 `#[ignore]` tests, live in the renderer for `cube_face_matrices`); hallway/campaign/mini: 0 mismatches, 0 contiguity violations; lift light 30–41 draws per moving frame vs 50,622 |
| 7 | Preflight, review panel, fix loop; `rendering_pipeline.md` §12 counter wording | integrating executor | 2–6 | done. Panel 1 (2 tracers, adversarial, contract verifier, hygiene): 0 🔴, 1 🟡 acted on (dead `Dropped` branch from a false Correction). Panel 2 on the fixes and measurement record (tracer, measurement-contract verifier, hygiene): 0 🔴. 🟡 acted on: exact walk-count assertions, GPU frame normalization, M2 waiver wording, M1 substitute evidence, M6 negative control, reproduction pins, runner lock/vacuous-pass holes. Final gate ✓ (fmt, clippy -D warnings, `cargo test --no-fail-fast`, check --release, crate-graph) |

## Follow-ups
- `postretro-tool` `dist::payload::tests::completion_marker_names_the_stage_then_one_relative_level_per_line` is flaky (passed 2 of 3 standalone runs; the marker's temp dir vanished mid-test). It predates this branch, which doesn't touch the crate. File it separately.
- Campaign-test `render_submit` +0.05 ms (M2) awaits an owner waiver; cause not established.
