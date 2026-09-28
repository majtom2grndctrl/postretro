# animated-lightmap-compact-atlas — plan of record

mode: compact
status: active
read at: 76a6370ac

## Corrections
- Source moved since `f95d476ac` only through PR #531 (merged): `LevelGeometry` gained `cells`, and level install builds SH residency from `geometry.cells` beside `AnimatedLightmapResources::new`. Both are adjacent to, not inside, the cited symbols; planning around them by setting `cells: &[]` in any hand-built `LevelGeometry`.
- Brief: "No vertex is shared across blocks." → clarified as: a vertex referenced by a face that owns a block is referenced by no other face, block-owning or not. A vertex shared with a no-block face would hand that face a block id through the flat varying, so the stronger check is the same guarantee. Same meaning, enforced a little wider.
- Brief: page-size preflight with a placeholder section 22 skips the upper bound. → The "unless the static layer is smaller" clause of the lower bound also references section 22, so it is skipped too; power-of-two and "at least the largest block" still apply. The level then takes today's no-animated-light path either way.
- Brief: "generalizes `ShResidencyReport`". → The row-level ledger types are SH-named (`ShResidencyAllocation`, `ShResidencySource`, `ShResidencyAllocationState`, `ShResidencyAllocationShape`, `ShAllocationLedger`). Planning around it by moving the four row types to neutral `Residency*` names in `render/residency.rs` and building the lightmap-family meter as a second report of the same row model, on the same Data/Dummy/Fallback and section-citation conventions. `ShAllocationLedger` stays SH-named and SH-only: the lightmap meter reads its five rows straight from the bound textures and needs no ledger. `ShResidencyReport` keeps its name and fields.
- Runtime `WorldVertex.lightmap_layer: u32` → splits into `lightmap_layer: u16` + `animated_block: u16`, read as one `Uint16x2` attribute at location 5, matching the Boundary inventory. Stride stays 36.
- Static lightmap layers are square power-of-two (`pack_layers` / `choose_layer_dim`), so "the static layer size" is the section-22 layer width, and the identity layout's pages are `page_size × page_size` at that width.
- `build_pipeline.md` calls the weight-map stage uncached (doc drift, known). Fixed at landing.
- Brief: "When the static lightmap is the placeholder, the level keeps today's no-animated-light path." → Source: a map with no static baked lights compiles a 1×1 placeholder section 22 and `prepare_atlas` never splits vertices or assigns lightmap UVs, yet sections 24/25 are still written. The vertex guards cannot hold there and the runtime never samples the animated atlas, so the compiler still writes the (repacked) sections but skips the vertex guards and stamps no block ids. The block cap still applies there, through section-25 consistency. The loader and renderer treat a 1×1 single-layer section 22 as the placeholder (`LightmapSection::is_placeholder`); the renderer's static-layer resolver (`usable_static_layers`) now returns `None` for it, so the animated atlas takes the dummy path with a warning instead of today's cross-section error. Same outcome: no animated light.
- Brief Path: "`DispatchTile.target_slot`". → Renamed `target_page` in Rust and in `animated_lightmap_compose.wgsl`, and the compose rect fields `atlas_x/y` → `compact_x/y`; compose logic is unchanged.

## Delegated answers
- Dev panel location and log line format — the Performance tab gets a "Lightmap memory" section with the five rows and their total. Each level install logs one `[Renderer] Lightmap residency:` info line naming the five byte counts. Both read the same `LightmapResidencyReport` the capture report serializes, so the three cannot disagree.
- Unload proof — no new unload harness. The meter is rebuilt by every `install_level_geometry`, and `release_level_resources` installs the empty geometry. The meter reads the textures actually bound, so the proof is a self-skipping GPU test on `Renderer::new_offscreen`: install level A, release it, install level B.

- Page-size ties — when two page sizes cost the same bytes, the smaller page wins (finer load-and-evict unit for residency stage 5). "Blocks that fit on one page allocate one page" is read at the chosen page size.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| Parity: identity vs packed within one 8-bit step; forced lights change the frame | GPU test `animated_atlas_parity` (self-skipping `gpu_or_skip` pattern) running the real compose shader and forward's block lookup + sampling helpers | achievable as stated | pass — `animated_atlas_parity_test::identity_and_packed_block_tables_render_the_same_forced_animated_frame` on AMD Radeon Pro 5300M (`POSTRETRO_REQUIRE_GPU=1`); flipped-offset mutation fails it |
| Identity table composes each chunk at its static layer and position; forward samples each face at its static UV | compiler test on identity layout + parity GPU test's identity remap probe | achievable as stated | pass — `identity_block_table_samples_each_face_at_its_static_uv_and_layer_page`, `identity_layout_keeps_static_positions_one_page_per_occupied_layer` |
| Repack changes only where chunks sit; weights and light lists byte-identical; block spans full placement; chunk offset in block unchanged (P4) | compiler unit test | achievable as stated | pass — `repack_rewrites_positions_only_and_keeps_chunk_offsets_inside_blocks` |
| No-animated level: no sections, all ids 0, renders as before, meter at placeholder (P5) | compiler test + meter test | achievable as stated | pass — `cull_drops_every_chunk_when_none_is_lit`, pipeline emits neither section; meter placeholder in on-demand `offscreen_renderer_meter_returns_to_placeholders_after_unload` (level A has no animated sections) |
| Fully culled face: no block, ids 0; all dropped → no sections (P3, P5) | compiler test | achievable as stated | pass — `cull_drops_unlit_chunk_and_rebases_every_parallel_table`, `cull_drops_every_chunk_when_none_is_lit`, `stamping_names_block_n_as_n_plus_one_and_leaves_other_faces_zero` |
| Several chunks some culled → one block; culled texels stay zero (P4) | compiler test + parity GPU test samples a culled region | achievable as stated | pass — `cull_keeps_one_block_for_a_face_with_some_culled_chunks`; parity test's culled probes read zero |
| Shared vertex fails the build | compiler test | achievable as stated | pass — `a_vertex_shared_with_a_block_face_fails_the_build` |
| Footprint past placement fails; just inside passes | compiler test | achievable as stated | pass — `footprint_past_the_placement_fails_the_build`, `footprint_just_inside_the_placement_passes` |
| Block count over cap fails naming cap and count; at cap passes | compiler test | achievable as stated | pass — `block_count_over_the_cap_fails_naming_cap_and_count_and_at_the_cap_passes`; also enforced by section-25 consistency on every path (review fix) |
| Compiler cap == shader table capacity; fits requested uniform size; ≤ u16 id (P10) | renderer test parsing forward.wgsl | achievable as stated | pass — `block_table_capacity_matches_forward_wgsl_and_the_compiler_cap`, `block_cap_fills_the_requested_uniform_and_fits_a_u16_vertex_id` |
| v4 round-trips; v2 and v3 rejected with recompile error | level-format + loader tests | achievable as stated | pass — `v4_round_trips_blocks_pages_and_compact_chunks`, `v2_and_v3_sections_are_rejected_with_a_recompile_error` |
| Loader rejects non-power-of-two or out-of-bounds page size (P14) | loader test | achievable as stated | pass — `page_size_that_is_not_a_power_of_two_is_rejected_with_a_recompile_error`, `page_size_outside_the_bounds_is_rejected` |
| Placeholder static lightmap loads with no animated light; upper bound unchecked (P14) | loader test + renderer dummy-path test | achievable as stated | pass — `placeholder_static_lightmap_skips_the_upper_bound`, `placeholder_static_lightmap_offers_no_static_layer_to_the_animated_atlas` |
| Identity layout allocates today's slot count | compiler test | achievable as stated | pass — `identity_layout_keeps_static_positions_one_page_per_occupied_layer`; campaign-test identity = 3 pages = today's 3 slots |
| Warm build over pre-change cache re-bakes the weight-map stage | compiler `stage_version_bump_*` test | achievable as stated | pass — `stage_version_bump_changes_cache_key`, `stage_version_bump_misses_then_hits` (STAGE_VERSION 7) |
| Editing only an animated light leaves the SDF atlas cached (P1) | compiler integration test | achievable as stated | pass — `editing_only_an_animated_light_keeps_the_sdf_atlas_cached` (on-demand, `-- --ignored`) |
| Warm cache-hit rebuild writes sections 25 and 17 byte-identical to cold (P2) | compiler integration test | achievable as stated | pass — `warm_weight_map_cache_hit_writes_the_cold_sections_25_and_17` (on-demand, `-- --ignored`) |
| No-block vertices carry 0 and take today's path | compiler test + forward shader pin | achievable as stated | pass — stamping test + `forward_shader_samples_animated_arrays_through_the_block_table` |
| Mismatch, debug or `dev-tools`: load fails with recompile error (P11) | loader test (debug) | achievable as stated | pass — `block_id_mismatch_fails_the_load_with_a_recompile_error_in_debug_or_dev_tools` |
| Mismatch, player release without `dev-tools`: loads with no animated light, one error logged (P11) | loader test under `cargo test --release` | achievable as stated | pass — `block_id_mismatch_loads_without_animated_light_and_logs_once_in_player_release` under `cargo test --release -p postretro-level-loader` |
| Placeholder or nothing to compose → every vertex resolves to no block (P6) | renderer test on installed block table | achievable as stated | pass — `inactive_resource_installs_an_empty_block_table_for_a_valid_section`, `empty_block_table_names_no_block` |
| No two blocks overlap on a page; every block and chunk inside its page | compiler test + loader validation test | achievable as stated | pass — level-format `consistency_check_*` tests; compiler bails on an inconsistent layout |
| Vertex stays 36 B; forward storage/sampled counts unchanged; table FRAGMENT-only | existing geometry/BGL/budget tests, rewritten | achievable as stated | pass — `animated_block_round_trips_in_the_former_pad_bytes`, `lightmap_layer_and_animated_block_serialize_as_u16x2_at_byte_offset_32`, `bgl_entries_pin_sampler_split`, pipeline budget tests |
| Page size power of two within bounds | compiler test | achievable as stated | pass — `page_size_is_a_power_of_two_within_the_decided_bounds` |
| Fewest pages in cell order, no empty page | compiler test | achievable as stated | pass — `page_count_is_the_fewest_the_packer_fills_in_cell_order`, `overflowing_blocks_spill_into_a_second_page_with_no_empty_page` |
| Blocks fitting one page → one page regardless of static layers | compiler test | achievable as stated | pass — `blocks_that_fit_one_page_allocate_one_page_across_many_static_layers` (at the chosen page size; see Delegated answers) |
| Overflow spills to a second page; compose and forward agree (P9) | renderer test + parity GPU test with two pages | achievable as stated | pass — `compose_and_forward_resolve_every_block_to_the_same_page_and_offset`; parity test's packed layout uses page 1 |
| Page-sized block alone at origin; next block starts a new page (P8) | compiler test | achievable as stated | pass — `a_page_sized_block_packs_alone_at_the_origin_and_the_next_starts_a_new_page` |
| Animated bytes never exceed identity; oversize packing ships identity | compiler test + fixture sweep | achievable as stated | pass — `packing_that_would_exceed_the_identity_layout_ships_identity`, `a_packed_layout_is_never_larger_than_identity`; real content: campaign-test 48 MiB vs 144 MiB identity, stress-warren-mini 96 MiB vs 432 MiB |
| campaign-test capture: animated bytes = pages × page bytes, below full-layer meter (P13) | capture run on rebuilt campaign-test vs the T1 before reading | achievable as stated | pass — rebuilt campaign-test capture: animated irr 33,554,432 B = 4 × 1024² × 8, dir 16,777,216 B = 4 × 1024² × 4; below 100,663,296 / 50,331,648 |
| Load log reports five rows; dev panel and capture show the same numbers | meter tests + capture JSON test | achievable as stated | pass — `report_keeps_the_five_family_rows_in_order_and_totals_them`, `report_serializes_lightmap_family_rows_with_the_meter_bytes`; one stored report feeds log, dev panel, capture |
| After unload every count returns to placeholder (P12) | meter test | achievable as stated | pass — on-demand `offscreen_renderer_meter_returns_to_placeholders_after_unload` (static rows exercised; the animated pair was already placeholder in level A) |
| Fallback reports placeholder bytes, not the rejected atlas's (P7) | meter test on construction-failure path | achievable as stated | pass — `a_rejected_atlas_meters_its_placeholder_not_the_rejected_size` |
| Visual: animated lights unchanged on campaign-test, occlusion-test, closet-reveal | owner, in-engine | manual | pass (owner, 2026-09-28): animated lights look unchanged in play on all three maps; stale v3 PRLs refuse to load with the recompile error |
| Resource: meter bytes before/after on campaign-test, occlusion-test, stress-warren-mini | owner, in-engine (builder records its own readings too) | manual | pass for campaign-test (owner dev-panel reading 32.00 / 16.00 MiB animated, total 104.00 MiB, matches the capture); builder readings below (stress-warren-mini before reading is not comparable: the map now bakes 2048² static layers) |

## Before readings (meter, full-layer build, main-tree PRLs of 2026-09-28)

Capture measurement runs (`POSTRETRO_SH_STREAMING=sync-proof`), load-log `Lightmap residency` line:

| Map | static irr | static dir | shadowmask | animated irr | animated dir |
|---|---|---|---|---|---|
| campaign-test | 16,777,216 | 8,388,608 | 33,554,432 | 100,663,296 | 50,331,648 |
| stress-warren-mini | 442,368 | 221,184 | 8 | 3,538,944 | 1,769,472 |

occlusion-test and closet-reveal PRLs are stale in section 34 and do not load; their before readings need a rebuild with the pre-change compiler (owner's manual Resource row).

## After readings (meter, compact atlas, PRLs rebuilt in the worktree)

| Map | static irr | static dir | shadowmask | animated irr | animated dir | layout |
|---|---|---|---|---|---|---|
| campaign-test | 16,777,216 | 8,388,608 | 33,554,432 | 33,554,432 | 16,777,216 | 154 blocks, 4 pages of 1024² (identity: 3 of 2048², 144 MiB) |
| stress-warren-mini | 104,857,600 | 52,428,800 | 209,715,200 | 67,108,864 | 33,554,432 | 100 blocks, 2 pages of 2048² (identity: 9 of 2048², 432 MiB) |

campaign-test lands at a third of today's animated bytes (48 MiB vs 144 MiB), not the Problem's "near a quarter": cell-order MaxRects fills 4 pages where the area bound is 3. No Acceptance row depends on the quarter. stress-warren-mini's static layers are now 2048², so its old 128²-layer before reading is not a like-for-like comparison.

## Follow-ups
- `fixture_keeps_script_and_kvp_animated_prl_slots_distinct` (ignored compiler fixture test) asserts chunk light indices `0` and `2`, but section 24 uses the animated namespace (`0`, `1`, per `slot_for_map_light = [0, NONE, 1]`), so it fails. Not verified against main, but this branch does not touch chunk light indices or the namespaces.
- Pre-existing clippy findings under `--all-targets` / the `capture` feature (e.g. `measurement_report` argument count, `positional_io.rs` saturating add, `map_data.rs` dead fields); outside the preflight gate.
- The weight-map cache-hit check runs full section-25 consistency, block cap included, on the pre-cull identity bake. A level with more than 8,190 animated faces before the cull (but at most 8,190 after it) would re-bake that stage on every warm build. Latent: current content peaks at 977 blocks. Fix by exempting the cap on the cached entry.
- A player-release block-id mismatch drops section 25 in the loader, so the meter shows the animated pair as Dummy (uncited) rather than Fallback citing 25.
- `renderer/src/render/animated_lightmap.rs` and `renderer/src/lighting/lightmap.rs` are past the §2.1 size guidance; split on the next significant addition.

## Trial notes
- mode: compact
- sessions used: 1
- delegated implementation slices: 0 (review panel: 10 read-only reviewers + 1 fix verifier)
- review-panel findings: 2 🔴 (block cap unchecked at runtime; unbounded page-table allocation), 2 🟡, ~10 🟢 — all acted on except the tie-break (recorded as a delegated answer) and the file-size note
- Decision premises found false: 0
- Path claims corrected: 3 (placeholder static lightmap at compile, `target_slot` rename, residency ledger naming)

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Lightmap-family byte meter on today's full-layer atlas (renderer ledger generalization, load log, dev panel, capture JSON) and before readings | integrating executor | — | done: `residency`, `lightmap_residency`, capture `report_serializes_lightmap_family_rows_with_the_meter_bytes`; before readings below |
| 2 | Section 25 v4 types, shared block cap and page bounds (level-format); vertex pad → `animated_block` | integrating executor | — | done: `animated_light_weight_maps::tests`, `animated_lightmap_atlas::tests`, `animated_block_round_trips_in_the_former_pad_bytes` |
| 3 | Compiler: identity-layout bake, cull with blocks, cell-order MaxRects repack, guards, vertex stamping after the SDF key, budget on pages, stage bump, golden rebaseline | integrating executor | 2 | done except golden rebaseline (task 7): `animated_atlas_layout::tests`, `animated_block_ids::tests`, `animated_light_weight_maps::tests`; full `-p postretro-level-compiler` green |
| 4 | Loader / render-cpu: page-size preflight, v4 cross-section validation, block-id cross-check with mismatch policy | integrating executor | 2 | done: `prl_animated_atlas::tests`, render-cpu `validate_cross_section_*`; release-build mismatch test runs in task 8 |
| 5 | Renderer: `Uint16x2` vertex attribute, binding-7 block table, forward remap, page-sized atlas and page-targeted tiles, installed-table fallback | integrating executor | 2, 4 | done: renderer lib 716 green incl. `compose_and_forward_resolve_every_block_to_the_same_page_and_offset`, `block_table_capacity_matches_forward_wgsl_and_the_compiler_cap` |
| 6 | Identity-vs-packed parity GPU test (riskiest premise: zero gutters give pixel parity) | integrating executor | 5 | done: `animated_atlas_parity_test` (2 tests, ran on AMD Radeon Pro 5300M under `POSTRETRO_REQUIRE_GPU=1`; a flipped offset sign fails it); meter GPU tests `lightmap_residency_test` (fallback P7, data rows, on-demand offscreen-renderer unload P5/P12) |
| 7 | SDF-cache and warm/cold integration tests; rebuild stale PRLs; after readings and campaign-test capture | integrating executor | 3, 5 | done: `warm_weight_map_cache_hit_writes_the_cold_sections_25_and_17`, `editing_only_an_animated_light_keeps_the_sdf_atlas_cached` (on-demand, `-- --ignored`); golden rebaselined; campaign-test rebuilt and captured (after readings below) |
| 8 | Preflight, review panel, fixes | integrating executor | 1–7 | |
