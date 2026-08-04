# Lightmap Bake Throughput

## Goal

The lightmap bake casts every shadow ray through a BVH traversal that heap-allocates twice per call, and it casts all of them on one thread while every SH-family bake next door runs under rayon. Remove the per-ray allocation, then parallelize both lightmap bake paths across charts. Cold-build wall clock is the target; output must stay byte-identical.

## Scope

### In scope

- Replace the allocating BVH traversal in the shared shadow-ray primitive with the crate's lazy iterator, preserving its early-out on first hit.
- Apply the same replacement at the remaining bake-time traversal sites.
- Parallelize the monolithic whole-atlas lightmap bake across charts.
- Parallelize the per-light layer bake across charts — the path warm incremental builds actually run.
- Preserve byte-identical output, deterministic across repeated builds and independent of thread count.

### Out of scope

- Lightmap storage, atlas dimensions, direction-atlas format, or per-surface density. Sibling draft `lighting-scale--lightmap-bake-scaling` owns those.
- Compile-time peak RAM and incremental bake-and-flush. Sibling draft `lighting-scale--lightmap-bake-incremental-flush`.
- The SH-family bakes. They are already parallel and stay structurally unchanged; their traversal sites take the allocation fix via Task 2's direct conversions, not through the shared primitive.
- Parallelizing BVH construction. The `bvh` crate offers a rayon executor, but the dependency is declared with default features off and the build stage is documented as fast enough to skip caching. See Open questions.
- Changing the bake cache, its keys, or what it stores.
- Changing sampling: ray counts, stratification, seeds, falloff, or the soft-shadow area-sampling model. Any of these would change output bytes, which the acceptance criteria forbid.

## Acceptance criteria

- [ ] The composited pre-BC6H lightmap atlas is byte-identical before and after this change, on every fixture in `content/dev/maps/`.
- [ ] The existing equivalence gate still passes: the per-light layer composite remains byte-identical to the monolithic bake's pre-BC6H atlas.
- [ ] Two builds of the same map produce byte-identical lightmap output, and output does not vary with the number of worker threads.
- [ ] SH, animated-weight-map, chunk-light-list, and entity-shadow-select outputs are byte-identical before and after — weight maps and entity-shadow-select take the change via Task 1's shared primitive; SH and chunk-light-list via Task 2's direct conversions.
- [ ] The allocation change alone, measured single-threaded, reduces cold lightmap bake wall clock on a heavily-lit fixture; the recorded number is the deliverable, and a regression against baseline fails this criterion.
- [ ] After both changes, cold whole-map lightmap bake wall clock on a heavily-lit fixture drops by at least 4× on a machine with at least 8 physical cores. If Task 5 shows a fixture where the largest chart's serial time alone exceeds a quarter of the pre-change wall clock, 4× is unreachable by chart-level parallelism on that fixture; the recorded largest-chart share plus the row-splitting follow-on justification then substitutes for the 4×.
- [ ] Bake progress reporting stays monotonic, and the published total still equals the units eventually advanced.
- [ ] Pause takes effect within one chart per worker: after `set_paused(true)`, no worker starts a new chart; in-flight charts complete and their progress advances land. Operator quit never leaves a worker parked — the quit path's `set_paused(false)` must wake all workers, now plural — and the detached background bake then runs to completion, matching current quit semantics. There is no cancellation mechanism in the bake control plane; adding one is out of scope. "Starts a new chart" means admitted past the `enter` gate; a worker parked at the gate inside the chart body has not started one.
- [ ] Peak resident memory does not exceed the pre-change peak by more than 25% — measured as whole-process peak RSS of `prl-build` on the same named fixture, same flags, both sides; a recorded-evidence gate.

## Tasks

### Task 1: Non-allocating shadow-ray traversal

`segment_clear` in `crates/level-compiler/src/lightmap_bake.rs` is the shared shadow-ray primitive: it builds a ray, calls `bvh.traverse(&ray, primitives)`, then walks the returned candidates testing triangles and returning `false` on the first hit within range. In `bvh` 0.11 that `traverse` allocates twice per call — it fills a `Vec` of indices during recursion, then maps that into a second `Vec` of shape references. Switch it to `traverse_iterator`, which yields candidates lazily from the same crate version and needs no cargo feature. This is a double win here: the allocations disappear, and because the function early-returns on the first hit, traversal now stops there instead of first materializing every candidate. Preserve the exact triangle-test order the current code walks — verified: in `bvh` 0.11 `traverse_iterator`'s in-order left-first walk yields candidates in the same order as `traverse`'s recursive collection, so the switch itself preserves order, and `segment_clear` returns only `bool`, which is order-independent regardless. One new failure mode: `BvhTraverseIterator` in `bvh` 0.11 uses a fixed 32-entry traversal stack that panics past depth 32, where the recursive `traverse` has no such bound — add a depth assertion in `build_bvh` (`crates/level-compiler/src/bvh_build.rs`, which returns `Result`, so failing the build is natural) that rejects a tree whose maximum node depth counting the root exceeds 32, with a clear error naming the limit — an over-deep tree then surfaces at build start on any future map rather than as an index panic mid-bake. The shared primitive has four callers: `bake_face_chart` in the monolithic lightmap bake, the per-light layer bake in `lightmap_layer.rs`, `animated_light_weight_maps.rs`, and `entity_shadow_select.rs`; none of their signatures change. `chunk_light_list_bake.rs` does not call this primitive — it holds a private clone, covered by Task 2.

### Task 2: Non-allocating traversal at the remaining bake sites

Three further bake-time traversal sites call the allocating `traverse` without going through the shared primitive: private clones of `segment_clear` in `crates/level-compiler/src/sh_bake.rs` and `crates/level-compiler/src/chunk_light_list_bake.rs` — each file's own copy of the primitive — plus `closest_hit` in `sh_bake.rs`, the one distance-returning consumer, which walks the full candidate set. (A fourth `traverse` in `sh_bake.rs` sits inside the test `build_bvh_traversal_interop` and needs no change.) Convert each to `traverse_iterator`. The ordering caution lands here, on `closest_hit`: a changed candidate order could flip a distance tie or reorder a float accumulation, so the conversion must not reorder the candidate walk — `traverse_iterator` yields the same order as `traverse` in `bvh` 0.11. The `sh_bake.rs` sites run under rayon, so their allocation is also cross-thread allocator contention; the `chunk_light_list_bake.rs` site runs serially from `pipeline.rs` and gains only the per-call allocation removal. Where a site needs the full candidate set rather than an early-out, it may collect into a caller-owned buffer reused across rays instead of allocating per ray — but do not introduce a reused buffer where a lazy walk suffices.

### Task 3: Parallelize the monolithic atlas bake

`bake_monolithic_atlas_controlled` in `crates/level-compiler/src/lightmap_bake.rs` loops placements sequentially, calling `bake_face_chart` with `&mut` slices of the whole atlas — `irradiance`, `direction`, and `coverage` — which each chart indexes through its own layer offset. Charts occupy disjoint atlas rectangles by construction — the MaxRects packing in `pack_layers` (`lightmap_bake.rs`) is what establishes it — so the writes never overlap, but the shared `&mut` slices block direct parallelization. Restructure so each chart bakes into small per-chart local buffers and is scattered into the atlas as it completes, then dropped — do not collect all chart buffers before scattering, which would hold a second copy of the whole uncompressed working set and threaten AC 9's memory bound. Scatter order cannot affect output bytes: chart rectangles are disjoint, so the scatter is non-overlapping memcpys whose order is immaterial — determinism here needs per-chart isolation, not an ordered merge. Use a `for_each` over placements with the scatter serialized under a short lock. Do not use a channel with the consumer running as a rayon pool task: under `set_permits(1)`, blocked `enter` waiters do not steal work, so the queued consumer never runs, and a producer blocking on a full channel while holding a permit deadlocks the bake; an unbounded channel drained after the `for_each` returns just re-creates collect-then-scatter. With the short-lock form, peak memory scales with in-flight worker count, as the Risks section assumes. The scatter is a memcpy per chart against the ray-casting cost it replaces. Per-texel determinism is already independent of iteration order because the sample-lattice seed is derived from atlas coordinates rather than a sequential counter, so no seed threading is needed. Gate each parallel work item per the `Governor` contract (`governor.rs`: serial loops call `checkpoint`; parallel work items call `enter` exactly once at their outermost boundary): take `let _permit = control.governor().enter();` at the top of the per-chart body, matching the existing parallel bakes in `direct_sh_bake.rs` and `sh_bake.rs`, and call `control.advance(1)` after the chart's work while the permit is held. `checkpoint` honors pause but ignores the permit cap, so a checkpoint-only body would bypass the `-j` throttle — the same permit mechanism the Risks section's memory mitigation relies on. A permitted item must never wait for another permitted item's completion or permit release (the Governor's nested-wait rule); a bounded wait on the scatter lock is exempt — the holder performs a finite memcpy and never parks while holding it. Degenerate-UV charts must still take a permit, contribute a no-op scatter, and advance progress, matching the current early-return in `bake_face_chart` plus advance. Progress units mean charts baked; stage completion remains `finish_stage`, and dilation runs un-gated after the parallel region drains, as it does today. Run the dilation pass after the scatter, unchanged and sequential. Rayon is already a dependency of this crate.

### Task 4: Parallelize the per-light layer bake

`bake_light_layer_controlled` in `crates/level-compiler/src/lightmap_layer.rs` mirrors the monolithic bake's per-texel structure for a single light and is the path warm incremental builds run, so it matters at least as much as Task 3. It loops placements sequentially and pushes `LayerTexel` records onto one growing `Vec`, which makes push order the output order. Restructure as a rayon parallel map over placements producing one `Vec<LayerTexel>` per chart, then concatenate in chart order — rayon's indexed parallel `collect` preserves input order, which is what keeps the output byte-identical rather than merely equivalent. Charts with a degenerate UV extent must still advance progress and contribute an empty vector, matching the current skip-and-advance behavior. Gate each chart with `Governor::enter` and advance progress per chart as in Task 3. One caller family sits outside the lightmap stage: `shadowmask_bake.rs` calls the `bake_light_layer` wrapper with `BakeControl::unrestricted()` (permits unbounded, never paused), which today is harmless because the bake is serial — after this task it would become full-width parallel work that ignores the `-j` cap and pause. The shadowmask stage has no `BakeControl` today — construct one for it in `pipeline.rs` (sharing the governor `Arc`, with its own `StageProgress` or an unregistered indeterminate one — do NOT reuse `lightmap_control`, whose stage is already finished and whose published total the shadowmask advances would overrun, violating AC 7) and thread it through `bake_shadowmask_atlas_cached` → `bake_shadowmask_atlas` → the `bake_light_layer` call sites; their signatures change, which does not conflict with Task 1's no-signature-change promise (that covers only the shared ray primitive's callers). Do not leave an unrestricted-control caller of the now-parallel function.

### Task 5: Determinism gates and measurement

Run the existing equivalence tests that compare the per-light layer composite against the monolithic bake's pre-BC6H atlas — they live in the `lightmap_layer` test module and already assert bit-for-bit equality — and confirm they pass unchanged. Before Phase 1 lands — or, if this task runs after the fact, from the pre-Phase-1 and Phase-1-complete commits via git checkout — compile every fixture in `content/dev/maps/` and keep the emitted `.prl` files as the baseline artifacts, using each fixture's documented flags, identical on both sides of every diff — README-documented where present; `stress-warren-overflow` uses `--lightmap-density 0.06 --sh-probe-spacing 10.0` (documented only in `tools/gen_stress_map.py`); `stress-warren-maze-crates` has no documented flags, so pin the family's coarse preset `--lightmap-density 0.25 --sh-probe-spacing 10.0` (the `stress-warren` family does not compile sensibly at defaults): a whole-`.prl` byte diff against a post-change build is the cross-change check for AC 1 and AC 4 at once, since it covers the lightmap section and every other baked section (SH, animated weight maps, chunk light lists, entity shadow select) in one comparison — no per-family capture plumbing needed. If a diff fails, the existing pre-BC6H equivalence tests localize a lightmap divergence. Run each cross-change diff twice: once cold (`--no-cache`), which is the only mode that executes the monolithic path Task 3 restructured, and once cache-enabled against a brand-new empty cache directory, where the first build is the compared artifact — that first build is what executes Task 4's layer path; a warm build against a surviving pre-change cache serves pre-change bytes and proves nothing. Add a run-twice check: two builds of the same map, same thread count, byte-identical output. Check AC 7 by asserting at bake return that the sum of progress advances equals the published total — extend the existing reporter assertions or add a test. Add a check that a bake run with a single worker thread and a bake run with many produce identical bytes, so thread count is proven not to influence output — vary both knobs: the rayon pool width (`RAYON_NUM_THREADS`, or `ThreadPoolBuilder::install` in a test) and the governor permit cap (`-j` / `Governor::new`). Record, in this plan, cold lightmap bake wall clock at three points: before any change, after Tasks 1 and 2 measured single-threaded, and after Tasks 3 and 4; plus peak resident memory before and after, and the core count and CPU of the measuring machine. Separating the allocation and parallelism measurements is what makes the two changes independently justifiable. The ordering and progress scenarios to pin as tests are enumerated in the Ordering pins section of this plan's `index.md` — read it from the plan folder `lightmap-bake-throughput` under `context/plans/` (whichever stage subfolder it currently occupies); write the thread-count, pause, and degenerate-chart checks from those rows rather than restating them. Of AC 8's clauses, "no worker starts a new chart after pause" is deterministically testable (hold the sole permit so workers park at `enter`, pause, release the permit, assert `completed` frozen, resume, assert completion — the pattern `governor.rs`'s own tests use); the in-flight-charts-complete clause and the operator-quit path are review gates — do not write timing-window tests for them.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — different call sites, no shared edits; both are pure allocation changes.
**Phase 2 (concurrent):** Task 3, Task 4 — different files, independent bake paths, both built on the Phase 1 primitive.
**Phase 3 (sequential):** Task 5 — measures and gates the finished state. Two of its capture points cannot wait for Phase 3: the baseline `.prl` capture and the before-any-change wall-clock/RSS numbers are taken before Phase 1 lands, and the allocation-only measurement is taken at the Phase-1-complete commit, single-threaded — once Phase 2 restructures the loops, "the allocation change alone" is no longer measurable.
**Cross-plan:** this plan lands before `lighting-scale--lightmap-bake-incremental-flush`, whose Task 1 restructures the same monolithic loop into a per-layer bake-encode-drop; landing this plan first lets the flush plan wrap its per-layer loop around an already-parallel chart map.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Lightmap output is byte-identical to the pre-change bake | Task 1 (traversal order preserved), Tasks 3–4 (write order preserved) | Any change to sampling, seeding, or accumulation order; float addition is not associative, so per-texel accumulation order must not change | AC 1, AC 4 |
| The layer composite equals the monolithic bake bit-for-bit | Pre-existing gate | Each of Tasks 3 and 4 must independently preserve its path's bytes; because both sides are individually byte-stable, the gate holds at every intermediate commit in either landing order. A commit in which either task changed its own path's bytes fails the gate, whether or not the other has landed | AC 2 |
| Output is independent of thread count | Tasks 3–4 (per-chart isolation, order-preserving merge) | An unordered collect, or shared mutable accumulation, silently reintroduces dependence | AC 3 |
| Chart atlas rectangles are disjoint | Pre-existing packer guarantee (`pack_layers`, MaxRects) | Task 3's scatter assumes it; a packer change that allowed overlap would corrupt output rather than fail loudly | AC 1 |
| Progress, pause, and the permit throttle stay correct | Tasks 3–4 (permit and advance kept per chart) | Moving progress calls outside the parallel body would make totals wrong or pause unresponsive | AC 7, AC 8 |

## Ordering pins

Scenarios the parallel restructure must leave true, each concrete enough to write a test from. Task 5's checks reference these rows.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| 1 | Pause mid-parallel bake | `set_paused(true)` lands while K workers are mid-chart | Each in-flight chart completes and its `advance(1)` lands; no new chart starts; `completed` grows by at most K after the pause, then freezes |
| 2 | Resume wakes all workers | `set_paused(false)` after pin 1 | All parked workers wake; every placement is baked exactly once; final bytes identical to an unpaused run |
| 3 | Operator quit with workers parked | Pause, workers parked at the per-chart gate, quit; `tui_worker` runs `governor.set_paused(false)` then joins | All workers wake, bake runs to completion in the background, join returns; no worker left parked |
| 4 | Throttle honored by lightmap stage | `set_permits(1)` during the parallel bake | After in-flight charts drain, at most one chart bakes concurrently — fails if the body uses `checkpoint` instead of `enter` |
| 5 | Degenerate chart, monolithic path | A placement with zero UV extent baked inside the parallel body | Empty local buffer, no-op scatter, `advance(1)` still counted; final `completed` equals placement count |
| 6 | Degenerate chart, layer path | The same chart in `bake_light_layer_controlled`'s parallel map | Empty `Vec<LayerTexel>` occupies its slot of the ordered concat; output bytes identical to the sequential skip-and-advance |
| 7 | Batched completion, monotonic reads | N charts finish in one instant; the TUI samples `completed` concurrently | Every sampled value is non-decreasing; jumps of N are legal; at bake return, advances sum exactly to the published total |
| 8 | Reader never sees completed > total | `publish_total` precedes worker spawn; a reader samples throughout | `completed <= total` at every sample point, cold and warm |
| 9 | Counter is not a completion signal | Last `advance` lands while dilation is still pending | Legal transient; stage completion is `finish_stage` only; atlas bytes are undefined to observers until the bake function returns |
| 10 | Pause after the parallel region drains | `set_paused(true)` lands before dilation | Dilation completes un-gated, matching today; pause takes effect at the next stage's gate |
| 11 | Worker panic mid-chart | One chart's bake panics inside the parallel body | Permit released during unwind; panic propagates out of the parallel call; join reports the panic; a later `enter()` on the same governor does not hang |
| 12 | Tasks 3 and 4 land in either order | One task's commit lands without the other | The `lightmap_layer` equivalence gate passes at both intermediate commits; a gate failure indicts the landed task, not the missing one |
| 13 | Thread-count independence includes one | Same map baked with one rayon thread and with many | Byte-identical output, and identical to the pre-change sequential baseline |
| 14 | Scatter lock under a permit cap | `set_permits(1)`; the one permitted worker takes the scatter lock | Lock hold is a bounded memcpy, never a park; no other worker holds a permit; the bake completes |

## Rough sketch

The two changes are independent but compound. Removing the per-ray allocation is worth doing on its own — it is a few lines in a shared primitive and it lifts every bake stage, including the ones already parallel. Doing it first also matters for the parallel work: handing eight threads a function that mallocs twice per ray converts a latency problem into an allocator contention problem, and much of the parallel gain would be spent there.

The parallel structure is similar on both paths: chart-level work into per-chart local output, then a merge. The layer path needs an ordered merge — concatenation in chart order via rayon's indexed collect. The monolithic path needs only disjointness: charts scatter into non-overlapping atlas rectangles as they complete, in any order. Determinism survives because the per-texel seed is a function of atlas coordinates, not of visit order (`texel_seed` in `lightmap_bake.rs`, an FNV hash of atlas x, y) — that property already exists and is the reason this is a tractable change rather than a rewrite.

The measured baseline to beat: the shipped plan `plans/done/incremental-bake-per-element` records a cold build at 228 s with the SH bake's 631 s since moved behind a cache, so the lightmap bake is a large share of what cold builds still pay.

## Risks

Not open questions — each has a decided response.

- **Chart imbalance.** The 4× target assumes chart count greatly exceeds core count and that charts are roughly balanced. Boxy level geometry works against that: one wall of a large room is a single chart whose texel count dwarfs a dozen small ones, and the bake finishes when that chart finishes. If Task 5 finds a fixture where the largest chart alone exceeds the target wall clock, the fix is splitting within a chart by texel rows — a change to the parallel unit, not to this plan's structure — and it becomes its own plan rather than expanding this one. The follow-on must respect the Governor's nested-wait rule: a permitted chart must not wait on row sub-tasks that themselves call `enter`; the permit moves to the row level, or rows run un-permitted under the chart's permit. Task 5 records the largest-chart share of total bake time so the follow-on is either justified or ruled out on evidence.
- **Peak memory.** Memory rises with worker count, since each worker holds per-chart buffers. If the 25% acceptance bound proves tight on large atlases, cap the worker count for this stage rather than restructuring the merge; a bake that uses six cores instead of eight still delivers most of the win, and bounded memory matters more than the last 25% of throughput on a compile step authors run on their own machines.

## Task 5 evidence (2026-08-04)

### Scope and setup

The user explicitly narrowed the cross-change artifact capture from every development map to `gate-heavily-lit`, `campaign-test`, and `occlusion-test`. This is an evidence-scope exception to AC 1/AC 4's original every-fixture wording, not a claim that the unrun fixtures passed. The stopped exhaustive pre-Phase-1 capture at `/private/tmp/postretro-lightmap-bake-baseline-prephase1.C4YJjp` was preserved and was not resumed.

All three maps use their default flags. Cold comparisons used `--no-cache`; cache-enabled comparisons used a distinct, initially empty `--cache-dir` for each compiler side and map, and the first build was the compared artifact. Thus cold runs exercise the monolithic path and fresh cache-enabled runs exercise the layer path. The measuring host was an Intel Core i9-9980HK (8 physical / 16 logical cores). Finished-state commands used the compiler default of 14 jobs (two logical cores of headroom); the Phase-1 allocation-only measurement pinned `RAYON_NUM_THREADS=1`.

Artifacts and binaries were deliberately isolated:

- Preserved pre-Phase-1 artifacts: `/private/tmp/postretro-lightmap-bake-baseline-prephase1.C4YJjp`; its compiler was `/Users/dhiester/Projects/Personal/postretro/target/debug/prl-build` and was never rebuilt or overwritten.
- Phase-1 compiler: commit `a7a25d4f`, worktree `/private/tmp/postretro-lightmap-bake-phase1`, target `/private/tmp/postretro-lightmap-bake-phase1-target`, binary `debug/prl-build`.
- Finished-state compiler: target `/private/tmp/postretro-lightmap-bake-phase3-target.53JRTt`, binary `debug/prl-build`.
- New comparison artifacts and fresh caches: `/private/tmp/postretro-lightmap-bake-phase3-artifacts.FnQkWR`.

The finished-state target was built with `CARGO_TARGET_DIR=/private/tmp/postretro-lightmap-bake-phase3-target.53JRTt cargo build -p postretro-level-compiler --bin prl-build`. `campaign-test` additionally required `CARGO_TARGET_DIR=/private/tmp/postretro-lightmap-bake-phase3-target.53JRTt cargo build -p postretro-script-compiler --bin scripts-build`; the first attempted post-change campaign run failed before producing an artifact because that isolated target lacked `scripts-build`, then the recorded rerun succeeded. The Phase-1 target was built with `CARGO_TARGET_DIR=/private/tmp/postretro-lightmap-bake-phase1-target cargo build -p postretro-level-compiler --bin prl-build`.

The artifact commands were, for each map in the named three-map scope (with the literal map name substituted in every path):

```text
# cold
target/debug/prl-build content/dev/maps/<map>.map --no-cache -o <baseline>/cold/<map>.prl
/private/tmp/postretro-lightmap-bake-phase3-target.53JRTt/debug/prl-build content/dev/maps/<map>.map --no-cache -o <artifacts>/post-cold/<map>.prl

# first build against a fresh, map-specific cache directory
target/debug/prl-build content/dev/maps/<map>.map --cache-dir <artifacts>/pre-cache/cache-<map> -o <artifacts>/pre-cache/outputs/<map>.prl
/private/tmp/postretro-lightmap-bake-phase3-target.53JRTt/debug/prl-build content/dev/maps/<map>.map --cache-dir <artifacts>/post-cache/cache-<map> -o <artifacts>/post-cache/outputs/<map>.prl

# allocation-only checkpoint
RAYON_NUM_THREADS=1 /usr/bin/time -l /private/tmp/postretro-lightmap-bake-phase1-target/debug/prl-build content/dev/maps/gate-heavily-lit.map --no-cache -o <artifacts>/phase1-cold/gate-heavily-lit.prl
```

`<baseline>` is the preserved baseline root above and `<artifacts>` is the Phase-3 artifact root above. SHA-256 was computed with `shasum -a 256` for every pair.

### Output evidence

| Mode | Map | Pre-change SHA-256 | Finished-state SHA-256 | Result |
|---|---|---|---|---|
| `--no-cache` | `gate-heavily-lit` | `2a799e3a66eefd5910df8fc6804c9c0620766f71b75b64e8511c5524a74f5c13` | `2a799e3a66eefd5910df8fc6804c9c0620766f71b75b64e8511c5524a74f5c13` | identical |
| `--no-cache` | `campaign-test` | `722bd98747bcbe6c4e08fe3eccd6594c711abb9e62b1db177c79bdcb628a2e27` | `bdf97b551ee1fa0fe65b26480808fd025276aa178767dc29586e6b8a9665105e` | differs; see below |
| `--no-cache` | `occlusion-test` | `d7ca787a6ca3a96f7f8524c39879685f95a90329fe2a41857e4ac0583386004c` | `d7ca787a6ca3a96f7f8524c39879685f95a90329fe2a41857e4ac0583386004c` | identical |
| fresh cache | `gate-heavily-lit` | `7c8e210f110299212cb3b61d06759169a5311e901cfa852298808e5096411850` | `7c8e210f110299212cb3b61d06759169a5311e901cfa852298808e5096411850` | identical |
| fresh cache | `campaign-test` | `81a4d880fad6b3644fbf63747d7c368366b7fb2c2b5df316c2010ddf034f36c2` | `08ade6a10e6e949e4c2d7eb568547cd839f4bcbd8ed252228fad91f554080252` | differs; see below |
| fresh cache | `occlusion-test` | `10284278372237fc40dea6844ae25c48c2b6b74e016ec43474e9db65b26ecc09` | `10284278372237fc40dea6844ae25c48c2b6b74e016ec43474e9db65b26ecc09` | identical |

The Phase-1 gate artifact is also byte-identical to its pre-Phase-1 counterpart: `2a799e3a66eefd5910df8fc6804c9c0620766f71b75b64e8511c5524a74f5c13`.

`campaign-test` is the sole mismatch in this narrowed sample. For the cold pair, a section-level read-only comparison found that every section except `MapEntity` was identical, including Lightmap, SH, Direct SH, Delta SH, animated direct SH, Shadowmask, Chunk, Entity Shadow, and DataScript. The first differing byte is 261,522,361, in `MapEntity` (section id 29; section-relative offset 405). The fresh-cache pair has the same first differing byte. This cannot be attributed to the lightmap-path changes from the observed artifacts; it remains a whole-PRL mismatch to investigate, particularly because the baseline and isolated post targets used separately built script compilers.

### Determinism, progress, and equivalence gates

- `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build monolithic_ -- --nocapture`: passed (5 tests; 2 ignored fixture gates).
- `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build layer_bake_ -- --nocapture`: passed (4 tests).
- `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build lightmap_bake_produces_byte_identical_output_on_repeated_runs -- --nocapture`: passed.
- `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build lightmap_composite_equals_monolithic_on_fixtures -- --ignored --nocapture`: passed (33.39 s).
- `cargo check -p postretro-level-compiler`: passed.

The added checks cover monolithic and layer one-v-many Rayon worker equivalence, repeated output, degenerate charts, `completed == total` at bake return, and the deterministic pause-before-admission case (hold the sole permit, pause, release it, verify no chart advances until resume). Timing-window tests for in-flight completion and quit remain intentionally omitted per Task 5.

### Performance and RSS evidence

The controlled heavily-lit cold measurements are below. “Pre rerun” and “finished state” were each one `--no-cache` run on the current host using `/usr/bin/time -l`; that makes them the fairest before/after pair. The historical baseline provides an additional prior pre-change observation. Peak RSS is whole-process maximum resident set size.

| State | Lightmap bake | Total | Peak RSS | Notes |
|---|---:|---:|---:|---|
| pre-change historical baseline | 84.44 s | 163.02 s | unavailable | original capture; sandboxed `time -lp` failed only while querying clock rate after the compiler wrote the artifact |
| pre-change rerun | 92.30 s | 171.37 s | 1,354,825,728 B | same old compiler and flags, current-host pair |
| Phase 1 (`a7a25d4f`, one Rayon worker) | 38.81 s | 236.97 s | 1,148,891,136 B | allocation-only checkpoint; output hash unchanged |
| finished state (default 14 jobs) | 7.95 s | 67.07 s | 1,272,582,144 B | parallel monolithic bake; output hash unchanged |

The finished-state cold lightmap bake is **11.61×** faster than the fair pre-change rerun (92.30 / 7.95) and **10.62×** faster than the historical capture (84.44 / 7.95), exceeding the 4× requirement. Its peak RSS is 82,243,584 B lower (6.07%) than the fair pre-change run, within the 25% bound. The allocation-only checkpoint improved the lightmap phase relative to the pre-change rerun; because the old monolithic path was serial, the single Rayon-worker setting isolates the allocation revision from the later chart parallelism.

For context, fresh-cache first-build times were: `gate-heavily-lit` pre 73.05 s lightmap / 105.38 s total (1,293,365,248 B RSS), finished 11.08 s / 23.72 s total (23.82 s process real); `campaign-test` pre 225.78 s / 413.81 s total (413.95 s real), finished 162.95 s process real; `occlusion-test` pre 38.01 s / 189.07 s total (189.45 s real), finished 11.28 s / 140.91 s total (141.13 s real). The retained campaign terminal output did not include its finished-state stage summary, so only its whole-process real time is recorded.

These are single-run measurements on a shared host, not a benchmark distribution; host load varied between captures, and only the named fixtures were run. The Phase-1 run is the requested single-worker allocation checkpoint but was not paired with a fresh single-worker pre-change rerun. No largest-chart instrumentation was added because the measured 4× cold target passed, so the imbalance fallback is not invoked. These limitations, plus the unresolved `campaign-test` `MapEntity` diff, mean the evidence records the successful scoped gates and performance result without claiming that the original exhaustive AC 1/AC 4 capture is complete.

## Open questions

None.

The one item that read as open — whether to enable the `bvh` crate's rayon feature for parallel tree construction, currently off via `default-features = false` — is decided as no. `traverse_iterator` needs no feature, so this plan is unaffected either way, and the build pipeline documents the BVH stage as fast enough that it is not even cached. Enabling a default feature to speed up a stage nobody has measured as slow is the wrong trade against the lean-dependency goal.
