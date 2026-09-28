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
- Brief: "When the static lightmap is the placeholder, the level keeps today's no-animated-light path." → Source: a map with no static baked lights compiles a 1×1 placeholder section 22 and `prepare_atlas` never splits vertices or assigns lightmap UVs, yet sections 24/25 are still written. The vertex guards cannot hold there and the runtime never samples the animated atlas, so the compiler still writes the (repacked) sections but skips the vertex guards and stamps no block ids. The loader and renderer treat a 1×1 single-layer section 22 as the placeholder (`LightmapSection::is_placeholder`); the renderer's static-layer resolver (`usable_static_layers`) now returns `None` for it, so the animated atlas takes the dummy path with a warning instead of today's cross-section error. Same outcome: no animated light.
- Brief Path: "`DispatchTile.target_slot`". → Renamed `target_page` in Rust and in `animated_lightmap_compose.wgsl`, and the compose rect fields `atlas_x/y` → `compact_x/y`; compose logic is unchanged.

## Delegated answers
- Dev panel location and log line format — the Performance tab gets a "Lightmap memory" section with the five rows and their total. Each level install logs one `[Renderer] Lightmap residency:` info line naming the five byte counts. Both read the same `LightmapResidencyReport` the capture report serializes, so the three cannot disagree.
- Unload proof — no new unload harness. The meter is rebuilt by every `install_level_geometry`, and `release_level_resources` installs the empty geometry. The meter reads the textures actually bound, so the proof is a self-skipping GPU test on `Renderer::new_offscreen`: install level A, release it, install level B.

- Page-size ties — when two page sizes cost the same bytes, the smaller page wins (finer load-and-evict unit for residency stage 5). "Blocks that fit on one page allocate one page" is read at the chosen page size.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Parity: identity vs packed within one 8-bit step; forced lights change the frame | GPU test `animated_atlas_parity` (self-skipping `gpu_or_skip` pattern) running the real compose shader and forward's block lookup + sampling helpers | achievable as stated |
| Identity table composes each chunk at its static layer and position; forward samples each face at its static UV | compiler test on identity layout + parity GPU test's identity remap probe | achievable as stated |
| Repack changes only where chunks sit; weights and light lists byte-identical; block spans full placement; chunk offset in block unchanged (P4) | compiler unit test | achievable as stated |
| No-animated level: no sections, all ids 0, renders as before, meter at placeholder (P5) | compiler test + meter test | achievable as stated |
| Fully culled face: no block, ids 0; all dropped → no sections (P3, P5) | compiler test | achievable as stated |
| Several chunks some culled → one block; culled texels stay zero (P4) | compiler test + parity GPU test samples a culled region | achievable as stated |
| Shared vertex fails the build | compiler test | achievable as stated |
| Footprint past placement fails; just inside passes | compiler test | achievable as stated |
| Block count over cap fails naming cap and count; at cap passes | compiler test | achievable as stated |
| Compiler cap == shader table capacity; fits requested uniform size; ≤ u16 id (P10) | renderer test parsing forward.wgsl | achievable as stated |
| v4 round-trips; v2 and v3 rejected with recompile error | level-format + loader tests | achievable as stated |
| Loader rejects non-power-of-two or out-of-bounds page size (P14) | loader test | achievable as stated |
| Placeholder static lightmap loads with no animated light; upper bound unchecked (P14) | loader test + renderer dummy-path test | achievable as stated |
| Identity layout allocates today's slot count | compiler test | achievable as stated |
| Warm build over pre-change cache re-bakes the weight-map stage | compiler `stage_version_bump_*` test | achievable as stated |
| Editing only an animated light leaves the SDF atlas cached (P1) | compiler integration test | achievable as stated |
| Warm cache-hit rebuild writes sections 25 and 17 byte-identical to cold (P2) | compiler integration test | achievable as stated |
| No-block vertices carry 0 and take today's path | compiler test + forward shader pin | achievable as stated |
| Mismatch, debug or `dev-tools`: load fails with recompile error (P11) | loader test (debug) | achievable as stated |
| Mismatch, player release without `dev-tools`: loads with no animated light, one error logged (P11) | loader test under `cargo test --release` | achievable as stated |
| Placeholder or nothing to compose → every vertex resolves to no block (P6) | renderer test on installed block table | achievable as stated |
| No two blocks overlap on a page; every block and chunk inside its page | compiler test + loader validation test | achievable as stated |
| Vertex stays 36 B; forward storage/sampled counts unchanged; table FRAGMENT-only | existing geometry/BGL/budget tests, rewritten | achievable as stated |
| Page size power of two within bounds | compiler test | achievable as stated |
| Fewest pages in cell order, no empty page | compiler test | achievable as stated |
| Blocks fitting one page → one page regardless of static layers | compiler test | achievable as stated |
| Overflow spills to a second page; compose and forward agree (P9) | renderer test + parity GPU test with two pages | achievable as stated |
| Page-sized block alone at origin; next block starts a new page (P8) | compiler test | achievable as stated |
| Animated bytes never exceed identity; oversize packing ships identity | compiler test + fixture sweep | achievable as stated |
| campaign-test capture: animated bytes = pages × page bytes, below full-layer meter (P13) | capture run on rebuilt campaign-test vs the T1 before reading | achievable as stated |
| Load log reports five rows; dev panel and capture show the same numbers | meter tests + capture JSON test | achievable as stated |
| After unload every count returns to placeholder (P12) | meter test | achievable as stated |
| Fallback reports placeholder bytes, not the rejected atlas's (P7) | meter test on construction-failure path | achievable as stated |
| Visual: animated lights unchanged on campaign-test, occlusion-test, closet-reveal | owner, in-engine | manual |
| Resource: meter bytes before/after on campaign-test, occlusion-test, stress-warren-mini | owner, in-engine (builder records its own readings too) | manual |

## Before readings (meter, full-layer build, main-tree PRLs of 2026-09-28)

Capture measurement runs (`POSTRETRO_SH_STREAMING=sync-proof`), load-log `Lightmap residency` line:

| Map | static irr | static dir | shadowmask | animated irr | animated dir |
|---|---|---|---|---|---|
| campaign-test | 16,777,216 | 8,388,608 | 33,554,432 | 100,663,296 | 50,331,648 |
| stress-warren-mini | 442,368 | 221,184 | 8 | 3,538,944 | 1,769,472 |

occlusion-test and closet-reveal PRLs are stale in section 34 and do not load; their before readings need a rebuild with the pre-change compiler (owner's manual Resource row).

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
