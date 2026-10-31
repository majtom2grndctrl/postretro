# surface-radiance-cache — plan of record

mode: resumable
status: approved
read at: d2bda6032

Source re-verified at d2bda6032. The only level-compiler commit since the brief's `faaaf8680` is 097b9fa18 (probe-point embedded emitters, the "lands first" dependency). No Decision premise is false.

## Corrections
- Hit → face → patch (Path seam `closest_hit_in_leaf`) → `Hit` carried only `texture_index`, point, normal and distance, and `BvhPrimitive` has no face index. Task 2: `Hit` gains `triangle` (index-buffer offset of the hit triangle), and the face comes from a binary search over `face_index_ranges` (the traversal yields `&BvhPrimitive` without its slice position). `BvhPrimitive` is untouched, so packed BVH bytes and `level-format` do not change.
- Static embedded classification "moves ahead of ShBake" → today it runs inside DirectShBake (`pipeline.rs:954`), after ShBake (837) and DeltaShBake (894). Planning around it by moving the one call to run after the new SH base gate and before Materials/ShBake. The same `EmbeddedEmitters` object still feeds ids 35 (982), 41 (1095), 47/48 (1404) and the lightmap stage (1639).
- The classifier tests sdf-shadowed lights → `has_emitter_sphere` (`texel_footprint/embedded_emitter.rs` ~128) excludes `ShadowType::Sdf`, and `direct_sh_bake::embedded_emitters` (`direct_sh_bake.rs:125-146`) also filters `StaticLightMap`. Planning around it by adding a bounce-family classify entry point that keeps Sdf lights. The probe-point set and its consumers stay byte-identical. With `--no-penumbra-floor` the bounce set is empty, matching the probe set (`direct_sh_bake.rs:130`) and AC 15.
- The sdf-shadowed flag warns "naming the bounce bake" → today's warning (`EmbeddedEmitters::warn`, called from `pipeline/lightmap_stage.rs:153`) names only lightmap/weight-map texels and "the SH and billboard probes it lights". Planning around it this way:
  - A light only the bounce set flags (sdf-shadowed) gets its own once-per-compile line naming the bounce bake.
  - Lights the probe set already flags keep their exact current lines, so the warnings for ids 35/41/47/48 equal the base commit's (AC 3).
- Parity reference → `bounce_parity_tests.rs` (`one_sample_bounce_parity_on_real_maps`, `#[ignore]`) has the BC6H bar, p50/p99/max, mean signed relative error and the `FullSetReseeded` control. Its reference, however, runs with `bounce_materials: None` (grey) and plain `soft_visibility` (`sh_bake/bounce.rs:140`). Planning around it by adding production albedo and the per-light embedded rule to `FullSet`, then swapping the cache path in for `OneSample`.
- cfg(test) shadow-ray / gather-ray counters → none exist. Adding them at `segment_clear`/soft-visibility (shadow) and `closest_hit` (gather), plus patch-lookup and patch-direct evaluation counters.
- Cache I/O outside permits → the `sh_group` precedent (`bake_or_load_group`, `sh_group.rs:499`) does get/put inside the governor permit. The new layer and id-34 entries follow the lightmap partitions precedent (`partitions.rs`) instead.
- Governor enter hook → it is `#[cfg(test)] pub(crate)` (`governor.rs:104`). The stop-mid-patch-shading and pause rows therefore live in the bin target, not `tests/`.
- SH base volume lifetime → `sh_volume_section` lives to the end of `run_after_parsing`, and `sh_ctx` (825) borrows inputs until scatter. The id-34 output must outlive the stage, but the cache (patches, layers, sum, bounce buffers) gets its own scope inside the ShBake stage, dropped before DeltaShBake (894).
- `prl-build --help` warm text → `main.rs:864` (plus the `--working-set-max-size` help at 862, which names only SH-delta and lightmap). Stale comments: `pipeline.rs:844-847, 861, 985`, `main.rs:536, 3359`, `light_namespaces.rs:150`, `sh_bake.rs:158, 418, 583, 1340, 1439`, `lightmap_layer/tests.rs:1520, 1612`.
- Retirement blast radius (task 6):
  - `sh_group.rs` and its `main.rs:65` mod.
  - `cache_cross_bake_tests.rs` (`SH_GROUP_*` imports, :686, :778, :1394).
  - `sh_bake/colour_key_tests.rs` (imports `SH_GROUP_DIM`, `SH_GROUP_STAGE_VERSION`).
  - `tests/oversize_lightmap_maps.rs:282` and `tests/coloured_bounce_cache.rs:38, 43, ~401` (pre-atlas memo lists).
  - `tests/coloured_bounce_source_gates.rs:202` (persistence gate) and :373 (knob gate, exact-equality allowlist).
  - `testing_guide.md:165-176` and `build_pipeline.md:532, 540, 548, 554`.
  - The `#[ignore]` gates `sh_cold_grouped_equals_monolithic_on_fixtures` and `warm_sh_within_tolerance_on_fixtures` retire with the module.
  - `INDIRECT_DELTA_SH_STAGE_VERSION` stays 5 (`cache_cross_bake_tests.rs:1393` pins it).

- Layer key folds "its own light's flag and the penumbra-floor setting" (Decisions: Embedded emitters) → task 4 folds the effective flag, flagged only when the floor is on and the light is classified, rather than the floor setting alone. With the floor off every set is empty, so an unflagged light's key is the same in both modes and only flagged lights re-key on a toggle, as AC 12 and P16 require. Folding the setting directly would re-key every layer and fail AC 12. This is a clarification that keeps the meaning.
- Warm path order → task 4 switches the warm id-34 bake from the grouped path to cached per-light layers plus the cold gather. Task 6 adds the whole id-34 entry and deletes `sh_group.rs`, the warm warning and the docs.

## Delegated answers
- Bounce count: provisional 4 later bounces, a code constant. The convergence fixtures (AC 8) and the A/B pick the final count, and changing it bumps the stage version. Reason: with albedo ≤ ~0.6, a fifth bounce adds under ~1/40 of bounce one.
- Patch size and lookup: **0.5 m cells on each face's absolute plane grid, bilinear lookup** (blending up to 4 cell centres within one face, renormalised over present cells). Task 2 evidence (error/bar at p50/p99/max, mean signed): campaign-test 3.2e-5 / 6.4e-4 / 1.30e-2 against a bar of 1.02e-3 / 2.16e-2 / 1.11e-1, mean +0.55%; soft_shadow_test 3.7e-4 / 3.9e-3 / 1.17e-2 against 7.9e-3 / 3.8e-2 / 4.44e-2, mean −0.058%. Bilinear beats nearest on every percentile on every map and costs about 20% more gather time. 0.25 m buys accuracy at 4× patches and is not needed on the required maps. On stress-warren-mini (not an AC map) every configuration fails max only, at best 1.10× the bar (0.5 m bilinear), while p50, p99 and bias pass. The cause: probes about 1 cm from a corner whose rays land inside one patch, where the reference reads centimetre-scale contact shading. **Owner ruling 2026-10-31: proceed and log it.** It is a known outlier from probes hugging walls; the roadmap's SH probe classification and relocation item is the fix. The Acceptance rows stay as written, and the parity test's default map list is the AC maps (warren-mini via `POSTRETRO_BOUNCE_PARITY_MAPS`).
- Back-face hits: per-(patch, side) sparse records. A layer stores a back-side record only where its light reaches the back side. Task 2: zero back-face reads on the AC maps. On warren-mini, 765 of 78.5 M hits at 292 probes, all within the bar. A synthetic below-the-floor probe reads within 2% of the reference.

## Module layout (decided at creation)
- `sh_bake/` split (task 1) along responsibilities in the current file: grid and layout, ray tracing, light evaluation, SH math and packing, probe bakes, section and encode.
- New `surface_radiance/` module:
  - `patches.rs`: patch set over pre-cut static faces, content-derived patch ids, and the hit → patch lookup.
  - `layer.rs`: per-light patch direct, layer key and serialization.
  - `sum.rs`: digest-ordered reduction.
  - `bounce.rs`: later bounces and fill.
  - `gather.rs`: probe gather that reads patches.
  - `stage.rs`: orchestration, cache and lifetime.
  - `working_set.rs`: the SH base gate.
- Key helpers `geometry_content_hash`, `bounce_material_hash` and `fold_bounce_hash` move unchanged into `bake_content_keys.rs`.

## AC-to-proof

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Parity p50/p99/max ≤ BC6H encode error; control beside | `bounce_parity_on_real_maps` (cache vs reference), campaign-test + fixture room (`#[ignore]`, run on demand) | achievable as stated |
| 2 | Mean signed rel. error ±1%; back-face hits within bound | same harness + back-face fixture test | achievable as stated |
| 3 | Embedded fixture: flagged point + flagged sdf light match no-early-out ref; ids 35/41/47/48 + keys + warnings = base; sdf flag warns once naming bounce | bin fixture test + `cmp` vs base-commit PRL + log capture | achievable as stated |
| 4 | Gather traces no shadow ray; spacing/rays change leaves count unchanged | cfg(test) counters + grep gate on gather path | achievable as stated |
| 5 | Doubling lights leaves gather/lookup counts; patch-direct evals = Σ reach | cfg(test) counters | achievable as stated |
| 6 | Bounce-lit room brighter; beyond-reach probe nonzero at fill 1, zero at 0; open-space change < bound | bin fixture tests | achievable as stated |
| 7 | Energy per bounce decreases (albedo < 1); closed albedo-1 room finite, non-rising | bin fixture tests, area-weighted patch energy | achievable as stated |
| 8 | Fill 0 = single-bounce bytes; fill 1 = unscaled sum; between, per channel | bin tests | achievable as stated |
| 9 | Warm id 34 = cold across the listed edit sequence; probe beyond 16 m reach | CLI warm/cold `cmp` suite (`tests/`) over a purpose-built fixture | achievable as stated |
| 10 | One light edit re-shades only its layer; re-gather no shadow ray; reorder/no edit hits all | bin counters + cache access log | achievable as stated |
| 11 | Remove/insert/duplicate/unique-reach/flip-to-animated: unchanged layers hit, byte-equal cold | bin tests | achievable as stated |
| 12 | Flagged light into/out of wall; floor toggle; sample-count change | bin tests (cache access by stage) | achievable as stated |
| 13 | Reversed order → identical id-34 tiles + moments; duplicate doubles radiance; warm duplicate misses id-34 entry | bin tests | achievable as stated |
| 14 | Geometry/texture-mean/saturation re-key all; restore hits; `_bounce_fill` re-shades no layer | bin tests (precedent `colour_key_tests.rs`) | achievable as stated |
| 15 | Animated-only edit re-shades no layer; descriptors/slot table = cold | bin tests | achievable as stated |
| 16 | Layer removed/corrupted/unfinished (enter hook) re-shades alone; A-B-A revert; eviction moves hits, not bytes | bin tests with governor enter hook + cache tamper | achievable as stated |
| 17 | Empty cache / after `--release` only → misses, release bytes; first build after change misses old id-34 | CLI tests + stage-version bump | achievable as stated |
| 18 | Light with no patch in reach: no layer, id 34 unchanged | bin test | achievable as stated |
| 19 | `_bounce_fill` in FGD + `docs/level_design.md`; invalid → default + warn once, cold/warm/hit | parse tests + CLI log capture + doc grep | achievable as stated |
| 20 | Knob gate admits `_bounce_fill` only; no bounce/albedo/saturation flag | `tests/coloured_bounce_source_gates.rs` amended | achievable as stated |
| 21 | No SH-approx warning; help/docs no longer describe approximate warm SH | CLI log test + doc grep gate | achievable as stated |
| 22 | Cold id 34 identical: 1 vs many workers, repeats, populated cache dir, governor pause | bin determinism tests | achievable as stated |
| 23 | Density/scale-region change → identical id 34; warm hits all; memo lists + persistence gate renamed | `tests/oversize_lightmap_maps.rs`, `tests/coloured_bounce_cache.rs`, source gate | achievable as stated |
| 24 | Fixed gather direction set across lights/fill; no direction reads cache radiance | test + grep gate on direction sources | achievable as stated |
| 25 | Layer + id-34 get/put outside governor permits | bin test (hook asserts no permit held during cache I/O) | achievable as stated |
| 26 | Depth moments byte-identical to one-sample bake | bin test | achievable as stated |
| 27 | id 27 bytes = base; first warm build hits pre-change 27/35/47/48/chunk-list entries | `cmp` of `--release` PRLs (stress-warren-mini + animated fixture) vs base build; warm-over-old-cache CLI run | achievable as stated |
| 28 | Zero static / emissive-only → zero irradiance, no layer; all-miss probe black | bin tests | achievable as stated |
| 29 | No diff under level-format, runtime crates or shaders; no new `SectionId` | diff + grep gate | achievable as stated |
| 30 | Forced-low budget refuses by SH base gate name before any shading/ray/embedded test/texture read; N admits, N−1 refuses; delta and lightmap boundaries hold | `tests/lightmap_working_set_contract.rs` pattern, cold + warm | achievable as stated |
| 31 | Gate patch set = baked patch set; predicted ≥ measured peak across fixtures/fill/workers/cache mode | bin handoff assertion + cfg(test) peak accounting | achievable as stated |
| 32 | Cache freed before delta SH stage begins | bin test (drop marker observed before DeltaShBake entry) | achievable as stated |
| 33 | Patch count = static faces after culling; mover + exterior-culled face get none | bin fixture test | achievable as stated |
| 34 | Measure: warm one-light edit on full hallway, phases timed apart | owner-launched run (`measure-hallway.ps1` pattern) | measurement |
| 35 | Measure: light-count sweep N/2N/4N | synthetic map, cold, one bake at a time | measurement |
| 36 | Measure: cold `--release --no-cache` Build Summary before/after on 4 maps | before = base-commit binary | measurement |
| 37 | Measure: hallway cache peak, layer + id-34 entry bytes vs `--cache-max-size` | owner-launched run | measurement |
| 38 | Owner A/B at campaign-test stations, fill 0 / mid / 1 + white room; owner picks default | owner, in-engine | manual |
| 39 | Entities and fog: no patch-scale blotches or new leaks | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split `sh_bake.rs` along its responsibility clusters, behaviour-preserving, in its own commit (no output change; `cargo test -p postretro-level-compiler --bin prl-build sh_bake` count unchanged) | worker (Opus), reviewed by integrating executor | — | done: 8 source modules (root 310 lines, max 257); 10 subject test files + `test_fixtures.rs`; `--bin prl-build` 2087 tests listed before and after (2064 pass, 23 ignored), `sh_bake` filter 125 passed + 1 ignored before and after; token-diff review shows only import changes; clippy clean |
| 2 | Thin slice, testing the riskiest premise. Add the patch set on pre-cut static faces, the primitive → face table, single-bounce summed patch direct in memory (no layers or cache yet), and a gather that reads patches. Upgrade the parity reference (production albedo, per-light embedded rule) and run AC 1–2 on campaign-test and a fixture room. Settle patch size, lookup and back-face handling from the result. Production still bakes one-sample. | integrating executor | 1 | done: `surface_radiance/` (patches, layer, sum, gather), the `RadianceSource` seam, the bounce-family classifier (sdf-inclusive) and a parity reference with production albedo and the per-light rule. AC 1–2 pass on campaign-test and soft_shadow_test at 0.5 m bilinear (Delegated answers). `--bin prl-build` 2073 passed, 23 ignored. No map flags an embedded light; AC 3 exercises the rule in task 3. |
| 3 | Cold id-34 path switches to the patch cache (single bounce; the warm grouped path stays until task 6, so tasks 3–5 prove against the live cold bake). SH base working-set gate (named error, prediction priced before any ray or texture read) placed before Materials. Move static embedded classification after it. Add the sdf-inclusive bounce-family classify and the bounce warning line. Scope the cache lifetime inside ShBake. (AC 3, 30, 32, 33) | integrating executor | 2 | done: cold id 34 bakes through `bake_sh_base`; SH base gate checked cheapest-first with the delta gate (campaign-test 414.9 MB, soft_shadow_test 2.8 MB); both embedded sets classified after the gates; bounce-only warning added; cache drops before Delta SH. `--bin prl-build` 2083 passed / 23 ignored; working-set contract 4 passed. Base-binary PRL comparison: cold differs only in ids 34 and 50, warm identical, id-35/scatter/chunk-list entries hit. Cold-vs-warm id-54 equality and warm = cold id 34 are relaxed until task 6 restores them. |
| 4 | Per-light sparse layers: content-keyed digest folding its own flag and the floor setting, seeds by patch id and light digest, digest-ordered sum, and a cache entry per layer written as it finishes, outside permits. Add cost counters. (AC 4, 5, 10–13, 16, 18, 25) | integrating executor | 3 | done: `"sh_patch_layer"` v1 keyed on the light content digest (no index or identity fields), geometry hash, bounce hash, patch constants and the effective flag; reach-bounded shading; content-ordered sum with occurrence counts; get and streamed put outside permits; warm id 34 now bakes through `bake_sh_base`, so warm = cold, with the stale warning left for task 6; cfg(test) counters in `bake_counters.rs`. AC 4, 5, 10–13, 16, 18 and 25 have tests (AC 11 drives `_animated`, not `setLightAnimation`). `--bin prl-build` 2097 passed / 23 ignored; parity still passes. |
| 5 | Multi-bounce patch-to-patch gather over every static face. Add `_bounce_fill`: parse with default + warn, FGD, `docs/level_design.md`, knob-gate amendment. Add the convergence fixtures; fix the bounce count. (AC 6–8, 19, 20) | integrating executor; FGD/docs/knob-gate slice to a Sonnet worker | 4 | done: `surface_radiance/bounce.rs`: 4 later bounces of 64 fixed cosine rays per patch front side, re-traced each bounce with no stored links; shipped = bounce one + fill × later; `_bounce_fill` parsed with default 1.0 (provisional) and a warning; FGD, `docs/level_design.md` and the knob-gate allowlist updated; gate prices 3 bounce buffers. Energy ratios per bounce: grey 0.43–0.44, so 4 later bounces suffice; albedo-1 room ≈1.00, finite. Fill 1 moves campaign-test probes +39% median, +242% p99, which is A/B input. campaign-test cost 3.3 s extra. Parity at fill 0 unchanged. `--bin prl-build` 2110 passed / 23 ignored; working-set contract 4 passed (made order-agnostic between the two SH gates). |
| 6 | One id-34 path: whole-volume gather cached as one entry over every gather input, stage-version bump. Production switches to it. Retire `sh_group.rs`, the grouped path, `build_shell` and the warm warning. Move the key helpers unchanged. Update the memo lists, persistence gate, help text and the `build_pipeline.md`/`testing_guide.md` text. (AC 9, 14, 15, 17, 21, 23, 26, 27, 28) | integrating executor | 5 | done: one `"sh_base_volume"` entry (v1) keyed on the layer plan (digest, flag, count), geometry, bounce materials, grid and validity, normalised fill and the bounce/gather constants; a hit reads no layer and marks layers used. `sh_group.rs`, the grouped path and the warm warning are gone (18 tests retired; the `build_shell` contract moved to the animated-only test). Key helpers moved unchanged to `bake_content_keys.rs`, with pre-move fingerprints pinned. Memo lists and the persistence gate renamed; `build_pipeline.md` and `testing_guide.md` rewritten with a doc grep gate; layer key now folds the grey fallback albedo. New CLI suite `tests/sh_base_warm_cold.rs` covers the 12-step AC 9 sequence. Compiler `--tests`: 2370 passed, 0 failed, 58 ignored. |
| 7 | Determinism and bounds sweep: worker-count, pause and repeat gates; gate-set = baked-set handoff assertion; predicted ≥ measured peak across fixtures; fixed-direction and no-visibility-in-gather grep gates; diff gate. (AC 22, 24, 29, 31) | integrating executor | 6 | done: AC 22 (12 fixtures × fill 0/1 × 1/2/8 workers × repeats; governor pause in all three phases; cold ignores a poisoned cache dir); AC 24 (direction recording + grep gate); AC 29 (`SectionId` pin + `#[ignore]` git diff gate); AC 31 (handoff `assert_eq!`; cfg(test) live-bytes ledger, 192 configurations, predicted ≥ measured, gate now the peak over six phases, worst ratio 3.25× on a 48-light fixture); AC 27 (`--release` id 27 identical to base on stress-warren-mini and `test_animated_weight_maps_mixed`; a warm build over the base binary's cache hits every id-27, 35, 47/48 and chunk-list entry); AC 11 `setLightAnimation` CLI case. Fixed the AC 9 suite, which had compared an empty volume at 4 m spacing; it now runs at 1 m and asserts stored tiles > 0. `--tests` 2380 passed / 62 ignored. |
| 8 | Measurements: light-count sweep, Build Summary before/after on hallway-mini, `stress-warren-mini` and `campaign-test`. The owner launches the full-hallway warm and cold runs (AC 34–37). Prepare the A/B builds at fill 0 / mid / 1 plus the white room. | worker (Sonnet) for scripted runs; owner for hallway and A/B | 7 | scripted runs done (`measurements/surface-radiance-cache/`). AC 36 cold SH Bake before → after: campaign-test 1.64 → 4.36 s (2.7×, 11 lights), stress-warren-mini 25.1 → 23.2 s, hallway-mini 74.8 → 49.9 s; totals flat or lower. AC 35 sweep, 402k-patch hall at 16 → 256 lights: SH Bake flat at 13.9–14.6 s; the base binary grows 4.6 → 31.2 s and crosses between 64 and 128 lights. A/B PRLs built (`ab/AB.md`). Owner rows AC 34 and 37 have a procedure (`warm-edit.sh`, `cache-window.py`) but no results yet. |

Hot paths touched: bake only; no per-frame path. Bake constraints: patch shading bounded to each light's reach, gather and later bounces bounded to static world faces, working set gated before work, cache I/O outside permits.


## Notes for preflight
- The worker reports these ignored tests fail at c1b1daba4 too, before task 3, so they are not caused by this branch:
  - `coloured_bounce_cache::cold_and_warm_builds_cache_entries_only_for_pre_change_stages`
  - both `coloured_bounce_oracle` comparisons
  - `animated_weight_maps_fixtures::editing_only_lightmap_density_keeps_the_sdf_atlas_and_residency_set_cached`
  Confirm against `main` before the review loop.
- `shade_light_layer` still visits every patch for each light, with only a per-patch range early-out. Task 4 bounds it with `PatchReachIndex` (AC 5).
- `build_pipeline.md` step 9, the Embedded emitters bullet and the `--working-set-max-size` row are stale. Update them at land.
- AC 11: `setLightAnimation` flips are not exercised yet; add a CLI case in task 7.
- Ignored tests still naming `sh_group` warm entries (`tests/oversize_lightmap_maps.rs:282`, `coloured_bounce_cache.rs:38, ~401`) are task 6's.
- Gate does not price yet: the reach index, the in-reach lists, and a cache hit's raw payload beside its decoded layer. Task 7's predicted-vs-peak check must count them.
- AC 19: the warning when a warm id-34 entry hits needs a CLI test once task 6 lands the entry.
- The id-34 entry key must fold the fill with -0.0 normalised to 0.0, plus `LATER_BOUNCE_COUNT` and `BOUNCE_RAYS_PER_PATCH`, or a stage-version bump when either changes. AC 24's grep gate must cover `cosine_directions` and `patch_frame`.
- The gate sums layers and bounce buffers although they never coexist. It is conservative now; tighten it in task 7 if predicted vs measured shows a large gap.
- Ignored `oversize_lightmap_maps::density_edit_that_moves_a_cut…` and `coloured_bounce_cache::cold_and_warm_builds_cache_entries_only_for_pre_change_stages` now fail only on a missing `cell_residency_set` cache line; their SH checks pass. Confirm on `main`, likely the same residency-set cause as the `animated_weight_maps_fixtures` failure.
- AC 9 step 10 (validity-only brush edit) is proven in-process; in the CLI suite it is a brush in the void, since a sealed map cannot express a validity-only edit.
- Base-commit comparison binary for AC 27 and task 8: `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/bff9e759-cf43-4be0-9557-5b1392586530/scratchpad/bin/prl-build-base` (cargo release, d2bda6032).
- The `SectionId` pin and the git diff gate (AC 29) are branch-scoped and fail once `main` adds a section. Retire both at land; their doc comments say so.
- The ledger does not count the per-face `BTreeMap` in `PatchSet::build`, `Vec` realloc copies or cache file I/O internals (testing_guide §3: a seam proves only what routes through it).
- id 41 (and id 34's own stored-density levels) can shrink as id 34 brightens. Their coarsening judges relative error against composed brightness, id 34 + id 35 + the deltas (`sh_coarsen.rs`). On the 256-light sweep id 41 was 264.9 MB against the base's 328.7 MB, so the base binary hit `--sh-delta-max-size` and the new one did not. This is expected downstream coupling, not an id-41 estimator change; AC 3 and AC 27 hold as written.
- Measurement gaps: no timed log line splits patch shading, later bounces and gather. The sweep inferred patch-direct from the progress-bar plateau, and later bounces + gather are reported together.
