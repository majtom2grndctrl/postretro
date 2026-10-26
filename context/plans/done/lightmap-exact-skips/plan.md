# lightmap-exact-skips — plan of record

mode: compact
status: done
read at: 1e84f1dd2

## Corrections
- Brief read at d116329d7 → no file outside `context/` changed through 1e84f1dd2; the Decision reads stand.
- §Margin scale's coordinate rows (research) → the derivation puts coordinate magnitude in the sampler's endpoint arithmetic, per hull and per axis, not in the hit test; a receiver declines past ~315–405 m along its normal axis, not at 1 km. Same Decision meaning: speed lost only at large coordinates; every authored map measured spans ≤ 150 m.
- A6 "the cone test drops those pairs" (research, spot aimed away) → the upward spot sits inside one quad's bounding sphere, so the Decision's inside-sphere guard keeps that one pair; it lights nothing and its bytes match. The fixture asserts exactly which pairs bake.
- Seam "a path through `bake_light_layer_chart_controlled`" → task 3 landed `ExactSkips { chart_cull }` on the partition window only; task 4 adds `shaft_cull` and threads it through the chart walk, where it is first read.
- A8 "two shadowed tiles that share one occluder" → one triangle cannot shadow A and C on the same rows while B between them clears (a separating halfspace holding A's and C's slab points holds B's between them). The fixture's one occluder is a gate (two posts beside B's shaft, a lintel above the light); on each row the left post's triangle is the remembered occluder carried across B, as O1 intends.
- §Hull containment's slab argument for lead-ins → it needs a near-orthonormal chart frame (unit axes within 1e-6, pairwise dots ≤ 1e-3) and a light centre ≥ 5ε above the texel plane after `light_size`; otherwise the tile tests blocked. Speed only: flat brush faces pass; a leaning vertex-normal chart never clears.
- A10 "an occluder just past its last texel column inside the pad" → applies to the floored pad; floor-off's pad is 4ε, so that post is outside the hull and the tile clears, bytes checked at both settings.
- M2 "re-keyed by another `--direction-texel-scale`, it logs a hit on every lightmap layer" → misses every layer on the base binary as well, so the flag reaches layer keys today (doc drift in `build_pipeline.md`, pre-existing); the row's intent, that layer keys did not move, is shown by re-keying the section memo with `--uncompressed-irradiance`.
- A20 reference parity → `mixed_lights` drops no pair, so a sibling test (`reference_parity_fixture_drops_a_pair_and_clears_a_tile_at_each_floor_setting`) adds one light and compares against the same cull-free reference at each floor setting; the original test is unchanged.
- Test seam → `ExactSkips::ON` runs verify mode under `cfg(test)`, so every existing test and `#[ignore]` gate re-traces each clear tile.
- "In `plan_charts`" (Path, chart cull placement) → the stage's planner is `LayerPartitions::plan_charts` (`pipeline/lightmap_stage/partitions.rs`); `lightmap_bake/charts.rs::plan_charts` is the chart builder the drift research cites. The cull goes in the former, after `light_may_reach`, before its `control.advance`.

## Delegated answers
- Reuse `affinity_grid.rs::direct_light_may_reach_cell` for the cone test? — No, a sibling (`lightmap_layer/chart_cull.rs`): the spot conventions match, but the precedent tests a 0.5 m-padded cell with an angle-space slack through an f32 `acos`, while the lightmap's zero is a cos-space compare at texel centres, so the sibling bounds the texel-centre rectangle's sphere in f64 with a cos-space margin.
- Margin constants — derived in `research.md` §Margin derivation (task 2), reviewed by the integrating executor; copied below.
- Safety factor κ = 2 or smaller? — 2. Its cost is the receiver crossover (a receiver stops clearing past ~315 m along its own normal axis), and the largest authored map measured (`stress-warren`, 5888 units) spans 150 m, the gate maps at most 125 m.

## Margin constants
Source: `research.md` §Margin derivation. u = 2⁻²⁴; a, e1 = fl32(b − a), e2 = fl32(c − a) in index order; N′ = e1 × e2 in f64; ρ = ‖e1‖‖e2‖/‖N′‖; e_T = max(‖e1‖, ‖e2‖); B = hull box, L = diag(B), S_T = ‖a − centre(B)‖ + ½L; per axis ε_i = 10u·R_i + 6u·L, e_H(a) = Σ|a_i|ε_i.
- m_N(T) = κ·[5√2·u·ρ(S_T + L) + u·S_T + e_H(n̂′)] — separation along the triangle's own normal; no angle term.
- m_A(T, a) = κ·[5√2·u·ρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ0 + u(S_T + 5e_T) + e_H(a)] — every other SAT axis.
- κ = 2; θ0 = 2°; ρ_cap = 100 (sizes the candidate query growth; a candidate past it blocks the tile, conservatively); no absolute floor.
- Candidates: every triangle whose box lies within D*(T) of B (closed form in research); ρ > ρ_cap or N′ = 0 inside the query blocks the tile.
- Evaluate in f64 after rounding e1, e2 to f32; hull corners in f64 from the exact chart formula; light centre `light.origin as f32`; never narrow g or m to f32; non-finite or zero axis gives no certificate.

## AC-to-proof

Fixtures live in the lightmap stage and layer test modules; "on/off" means the culls-off test seam (no flag, env var or key input).

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 point 1 cm behind dropped, just in front kept; soft light centre behind, sphere crossing, dropped | chart-cull fixture, drop/keep assert + on/off byte equality | achievable as stated | pass — `chart_cull_drops_a_light_behind_the_plane_and_keeps_one_just_in_front` |
| A2 non-orthogonal normal, behind at one corner, in front at another, kept | chart-cull fixture | achievable as stated | pass — `chart_cull_keeps_a_leaning_normal_chart_whose_light_is_behind_at_one_corner_only` |
| A3 spot just outside cone dropped, just inside kept; defaults; outer ≥ π/2 or negative kept; zero aim kept; light inside bounding sphere kept | chart-cull fixtures | achievable as stated | pass — `chart_cull_drops_a_spot_just_outside_…`, `…_culls_a_spot_with_no_authored_cone_…`, `…_keeps_spots_the_cone_test_declines` |
| A4 each falloff model: just past range dropped, exactly at range kept | chart-cull fixture per model | achievable as stated | pass — `chart_cull_drops_a_light_just_past_range_and_keeps_one_at_range_for_each_falloff_model` |
| A5 sun N·L ≤ 0 dropped, just above 0 kept | chart-cull fixture | achievable as stated | pass — `chart_cull_drops_a_sun_with_n_dot_l_at_most_zero_and_keeps_one_just_above` |
| A6 flipped existing range fixtures assert the drop, match unculled, progress completes | `window_tests.rs` fixtures updated | achievable as stated | pass — `…_drops_a_chart_the_light_reaches_only_through_its_padded_bounds`, `…_drops_charts_a_spot_cone_points_away_from` (one inside-sphere pair kept, §Corrections); warm reload of an emptied plan: `chart_cull_emptied_plan_loads_from_the_cache_on_a_warm_rebuild` |
| A7 open floor under soft point clears; with occluder | shaft fixtures, on/off × floor on/off, verify on | achievable as stated | pass — `shaft_cull_clears_an_open_floor_and_matches_the_unculled_walk_beside_an_occluder` (every all-lit tile clears) |
| A8 clear tile between two shadowed tiles sharing one occluder (O1) | shaft fixture | achievable as stated | pass — `shaft_cull_skips_a_clear_tile_between_two_tiles_its_occluder_shadows` (gate occluder, §Corrections) |
| A9 cone or range edge crosses a tile (O3) | shaft fixture | achievable as stated | pass — `shaft_cull_clears_a_tile_whose_first_covered_texel_is_not_its_first` |
| A10 interior not multiple of 8; one-texel chart; occluder past last column inside pad (O4) | shaft fixture | achievable as stated | pass — `shaft_cull_narrow_last_tiles_block_on_an_occluder_inside_the_pad_and_clear_past_it` (floored pad, §Corrections) |
| A11 triangle outside by < margin blocks, > margin clears; each the remembered occluder of a prior shadowed tile (O5) | shaft fixture | achievable as stated | pass — `shaft_cull_blocks_a_triangle_inside_its_margin_and_clears_one_outside_it` (0.949 / 1.049 m_N) |
| A12 near origin: 10° wall, 5° trim triangle, just over margin, tile clears | shaft fixture | achievable as stated (pending task 2 constants) | pass — `shaft_cull_clears_beside_a_grazing_wall_and_a_narrow_trim_just_past_their_margins` (margins within ±25% of an independent recompute: wall ≈ 0.57 mm, trim ≈ 4.8 mm) |
| A13 ordering fixtures record their clear tile | assertions inside A8–A10 | achievable as stated | pass — asserted inside A8–A10 |
| A14 occluder fixture at ≥ 4 km; cull may decline | shaft fixture, bytes only | achievable as stated | pass — `shaft_cull_matches_the_unculled_walk_four_kilometres_out` (declines all 16 tiles) |
| A15 hard point, spot, flagged embedded emitter | shaft fixtures | achievable as stated | pass — `shaft_cull_matches_for_a_hard_point_a_spot_and_an_embedded_emitter` |
| A16 sun bakes with no tile tested | shaft fixture, test counter | achievable as stated | pass — `shaft_cull_tests_no_tile_for_a_sun` |
| A17 cut face: one sub-chart dropped; overlap bit-identical; clear tile beside overlap (O6) | stage fixture | achievable as stated | pass — `cut_face_drops_only_its_dark_sub_chart_and_keeps_overlap_twins_identical_with_the_culls_on` |
| A18 verify mode: clear tile traced too, 1.0 per covered texel, caches equal | verify assertion in every shaft fixture | achievable as stated | pass — verify in every fixture helper; `shaft_cull_verify_mode_traces_every_covered_texel_of_each_clear_tile`; negative control `shaft_cull_verify_mode_fails_a_shadowed_tile_forced_clear` |
| A19 open floor: one test per covered tile, none per uncovered; clear tile traces no shadow, clip or lead-in segment (O2) | counting fixture | achievable as stated | pass — `shaft_cull_tests_each_covered_tile_once_and_traces_nothing_from_a_clear_one` |
| A20 permit side: clear tile at origin and at ~104 m on one long floor triangle; behind-plane drops a pair; reference-parity records clear tile and dropped pair at each floor setting | permit assertions | achievable as stated | pass — `shaft_cull_clears_tiles_on_one_long_floor_triangle_104_metres_out`, `reference_parity_fixture_drops_a_pair_and_clears_a_tile_at_each_floor_setting` |
| A21 existing parity: production = reference per floor setting; embedded alike; worker counts and warm = cold; overlap twins across cuts | existing tests, run unchanged | achievable as stated | pass — existing parity tests in the focused set (292 passed) and the bin target |
| M1 cold whole-PRL `cmp`, base vs new, 6 floored + 2 floor-off maps | integrating executor, prl-build A/B | manual | pass — final code (5213eff12) `cmp`-identical to base on all 8 runs; hallway-mini and `combat-demo` compared against the round-1 base PRLs (the saved base `scripts-build` cannot find its SDK prelude once its worktree is gone; base output is deterministic) |
| M2 warm on base-filled cache: section memo hits, layer hits under re-keyed `--direction-texel-scale`, PRL = cold | integrating executor, logs | manual | pass, re-key restated — new binary over a base-filled cache hits both section memos on both maps, PRL identical to cold. `--direction-texel-scale` re-key misses every layer on the base binary too (pre-existing: that flag reaches the layer keys, contrary to `build_pipeline.md`); re-keyed by `--uncompressed-irradiance` instead, the new binary hits every base-written layer (4 / 6) and the PRL equals a cold base bake |
| M3 any moved byte → stop, report count, largest change, f64 re-test | conditional on M1/M2 | manual | n/a — no gate moved a byte |
| M4 grep gate on diff | integrating executor | manual | pass — no new flag, no env var, no version constant change; atomics only in `#[cfg(test)] mod skip_stats` |
| M5 wall time: hallway-mini ≤ 0.62×, groups-capture median of 3 ≤ 0.65× | owner may launch long runs | manual | pass — hallway-mini Lightmap Bake 375.4 / 372.0 s base → 227.1 s (0.61×; SH control +5%); `shadowmask-groups-capture` median of three 2.40 → 1.35 s (0.56×). Dev profile, 5 workers, one bake at a time |
| M6 AnimWeightMaps and shadowmask-estimate share on one cold animated-light bake; roadmap entry if material | integrating executor | manual | measured — `stress-warren-mini` cold: AnimWeightMaps 13.4 s against Lightmap Bake 133.8 s (10%), total 244 s; `kinematic-platform` 2.4 s against 11.5 s. The stage log does not separate the shadowmask cut-face estimate (ShadowmaskAtlas 12.3 s whole stage). Roadmap entry: owner's call at landing |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split the clipped chart walk out of `lightmap_layer.rs`, behavior-preserving, own commit | delegated (split) | — | done: 50cd0aeb5, `lightmap_layer` 49 passed, clippy clean |
| 2 | Margin derivation in `research.md`; constants into this plan | delegated (derivation), reviewed by integrating executor | — | done: research §Margin derivation, randomized f32 MT checks at ≤ 42% of each bound |
| 3 | Chart coverage cull in `LayerPartitions::plan_charts` + culls-off seam through `bake_light_layer_chart_controlled` + A1–A6 | delegated, integrated by executor | 1 | done: `chart_cull` 13 passed; `lightmap_stage lightmap_layer chart_cull` 112 passed / 4 ignored; stage-path suites 280 passed / 9 ignored; clippy clean |
| 4 | Tile shaft cull + verify mode + test counters in the chart walk + A7–A20 | delegated, integrated by executor | 2, 3 | done: focused 289 passed / 10 ignored; bin target 1880 passed / 21 ignored; clippy clean; margin search 2 × 150k cases, 0 violations |
| 4b | Profile hallway-mini refusals; add proof-safe hull-face / tangent / edge-plane SAT axes, remembered blocker, spine pre-check | delegated, integrated by executor | 4 | done: focused 165 passed / 4 ignored; bin target 1881 passed / 21 ignored; clippy clean |
| R1 | Review panel round 1 (2 tracers, contract verifier, adversarial tester, opus; hygiene/drift, sonnet) on 48060ceed: no 🔴, no byte or key move found. Fixed: test references and the "unculled" partition now bake with `ExactSkips::OFF`; one shared texel-record helper; `ShaftScene` built lazily inside the permit, never for suns; sliver growth test at ρ ≈ ρ_cap; per-candidate recomputation removed; margin derivation sketch in `margin.rs`; sampler keep-in-step notes; range-only `light_may_reach` notes; tightened grazing-wall/trim margin checks (±25% of an independent recompute); random-axis m_A search; verify-mode negative control; warm reload of a cull-emptied plan. Declined: relaxing the lead-in guards (speed only, would change research §Hull containment) | integrating executor | 4b | done: focused 292 passed / 10 ignored; preflight fmt, clippy `--all-targets -D warnings`, release check, crate graph green; workspace `cargo test --no-fail-fast` 12 743 passed, 3–4 failed, all in crates this branch cannot reach (renderer GPU tests; a `fontdb` warning from a malformed system font in the engine's zero-warning ghost test) |
| R2 | Review round 2 on the fix commit (one tracer): no 🔴/🟡; two 🟢 comment notes applied (e50dc4a7b) | integrating executor | R1 | done |
| 5 | Existing parity (A21) in the final gate | integrating executor | 4 | done |
| 6 | Manual gates M1–M6 | integrating executor | 5 | done — see Result column |

## Gate log
- 2026-10-25, a0a5bc312 against base 1e84f1dd2 (dev profile, `--release`, 5 workers): cold `cmp` identical on all 8 M1 runs (floored: hallway-mini, `shadowmask-groups-capture`, `gate-heavily-lit`, `combat-demo`, `kinematic-platform`, `soft_shadow_test`; floor-off: `shadowmask-groups-capture`, `soft_shadow_test`).
- Lightmap Bake, same session: hallway-mini 372.0 → 243.1 s (0.653×; SH Bake control 111.5 → 123.2 s, +10%); `shadowmask-groups-capture` median of three 2.28 → 1.36 s (0.60×). Hallway misses ≤ 0.62× as measured; task 4b profiles the refusals and widens the SAT axis set.
- Task 4b, same session: base 375.4 s; a0a5bc312 232.1 s (0.618×); with hull-face, tangent and edge-plane axes, remembered blocker and spine pre-check 226.9 s (0.604×), SH control 113.5 / 117.3 / 117.4 s; PRL `cmp`-identical to base. groups-capture median of three: base 2.44 s, 4b 1.28 s (0.52×). On hallway-mini the cull clears 91.6% of the covered texels any tile cull could (tiles whose texels all bake 1.0); the rest is mostly walls crossing the padded slab (6.3 M texels), receiver m_N on sliver triangles (3.1 M), m_A-limited (1.8 M), drift (0.7 M). Test cost 4.6% of walk CPU before 4b. AnimWeightMaps 0.00 s on hallway-mini (no baked animated lights) — M6 still owed on an animated-light map.
- Final round (5213eff12/e50dc4a7b): all M1 runs identical; M2 as in the table; sliver gap sized: triangles past ρ_cap = 100 per map — 0 on `soft_shadow_test`, `shadowmask-groups-capture`, `gate-heavily-lit`, `combat-demo`, `stress-warren-mini`, hallway-mini (largest in-cap ρ 74.7); 4 on `kinematic-platform` (worst ρ ≈ 184), whose bytes match.

## Trial notes
- mode: compact
- sessions used: 1 (compacted once)
- delegated implementation slices: 6 (split, derivation, chart cull, shaft cull, axis tuning, review fixes ×6 agents)
- review-panel findings: round 1 — 0 🔴, ~12 🟡/🟢 acted on, 1 declined (lead-in guard relaxation); round 2 — 0 🔴/🟡, 2 🟢 acted on
- Decision premises found false: 0
- Path claims corrected: 2 (planner location; seam path landed in two steps)
- Landed: merged with main at e48c60108 (no conflicts; clippy, release check and the `--ignored` cold-bake determinism gates pass; remaining failures are environment-only). `build_pipeline.md` step 12 gains §Exact skips.
