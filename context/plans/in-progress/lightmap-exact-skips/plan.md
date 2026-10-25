# lightmap-exact-skips — plan of record

mode: compact
status: active
read at: 1e84f1dd2

## Corrections
- Brief read at d116329d7 → no file outside `context/` changed through 1e84f1dd2; the Decision reads stand.
- §Margin scale's coordinate rows (research) → the derivation puts coordinate magnitude in the sampler's endpoint arithmetic, per hull and per axis, not in the hit test; a receiver declines past ~315–405 m along its normal axis, not at 1 km. Same Decision meaning: speed lost only at large coordinates; every authored map measured spans ≤ 150 m.
- A6 "the cone test drops those pairs" (research, spot aimed away) → the upward spot sits inside one quad's bounding sphere, so the Decision's inside-sphere guard keeps that one pair; it lights nothing and its bytes match. The fixture asserts exactly which pairs bake.
- Seam "a path through `bake_light_layer_chart_controlled`" → task 3 landed `ExactSkips { chart_cull }` on the partition window only; task 4 adds `shaft_cull` and threads it through the chart walk, where it is first read.
- "In `plan_charts`" (Path, chart cull placement) → the stage's planner is `LayerPartitions::plan_charts` (`pipeline/lightmap_stage/partitions.rs`); `lightmap_bake/charts.rs::plan_charts` is the chart builder the drift research cites. The cull goes in the former, after `light_may_reach`, before its `control.advance`.

## Delegated answers
- Reuse `affinity_grid.rs::direct_light_may_reach_cell` for the cone test? — No, a sibling (`lightmap_layer/chart_cull.rs`): the spot conventions match, but the precedent tests a 0.5 m-padded cell with an angle-space slack through an f32 `acos`, while the lightmap's zero is a cos-space compare at texel centres, so the sibling bounds the texel-centre rectangle's sphere in f64 with a cos-space margin.
- Margin constants — derived in `research.md` §Margin derivation (task 2), reviewed by the integrating executor; copied below.
- Safety factor κ = 2 or smaller? — 2. Its cost is the receiver crossover (a receiver stops clearing past ~315 m along its own normal axis), and the largest authored map measured (`stress-warren`, 5888 units) spans 150 m, the gate maps at most 125 m.

## Margin constants
Source: `research.md` §Margin derivation. u = 2⁻²⁴; a, e1 = fl32(b − a), e2 = fl32(c − a) in index order; N′ = e1 × e2 in f64; ρ = ‖e1‖‖e2‖/‖N′‖; e_T = max(‖e1‖, ‖e2‖); B = hull box, L = diag(B), S_T = ‖a − centre(B)‖ + ½L; per axis ε_i = 10u·R_i + 6u·L, e_H(a) = Σ|a_i|ε_i.
- m_N(T) = κ·[5√2·u·ρ(S_T + L) + u·S_T + e_H(n̂′)] — separation along the triangle's own normal; no angle term.
- m_A(T, a) = κ·[5√2·u·ρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ0 + u(S_T + 5e_T) + e_H(a)] — every other SAT axis.
- κ = 2; θ0 = 2°; ρ_cap = 100 (sizes the candidate query growth only); no absolute floor.
- Candidates: every triangle whose box lies within D*(T) of B (closed form in research); ρ > ρ_cap or N′ = 0 inside the query blocks the tile.
- Evaluate in f64 after rounding e1, e2 to f32; hull corners in f64 from the exact chart formula; light centre `light.origin as f32`; never narrow g or m to f32; non-finite or zero axis gives no certificate.

## AC-to-proof

Fixtures live in the lightmap stage and layer test modules; "on/off" means the culls-off test seam (no flag, env var or key input).

| AC | Proof | Status |
|---|---|---|
| A1 point 1 cm behind dropped, just in front kept; soft light centre behind, sphere crossing, dropped | chart-cull fixture, drop/keep assert + on/off byte equality | achievable as stated |
| A2 non-orthogonal normal, behind at one corner, in front at another, kept | chart-cull fixture | achievable as stated |
| A3 spot just outside cone dropped, just inside kept; defaults; outer ≥ π/2 or negative kept; zero aim kept; light inside bounding sphere kept | chart-cull fixtures | achievable as stated |
| A4 each falloff model: just past range dropped, exactly at range kept | chart-cull fixture per model | achievable as stated |
| A5 sun N·L ≤ 0 dropped, just above 0 kept | chart-cull fixture | achievable as stated |
| A6 flipped existing range fixtures assert the drop, match unculled, progress completes | `window_tests.rs` fixtures updated | achievable as stated |
| A7 open floor under soft point clears; with occluder | shaft fixtures, on/off × floor on/off, verify on | achievable as stated |
| A8 clear tile between two shadowed tiles sharing one occluder (O1) | shaft fixture | achievable as stated |
| A9 cone or range edge crosses a tile (O3) | shaft fixture | achievable as stated |
| A10 interior not multiple of 8; one-texel chart; occluder past last column inside pad (O4) | shaft fixture | achievable as stated |
| A11 triangle outside by < margin blocks, > margin clears; each the remembered occluder of a prior shadowed tile (O5) | shaft fixture | achievable as stated |
| A12 near origin: 10° wall, 5° trim triangle, just over margin, tile clears | shaft fixture | achievable as stated (pending task 2 constants) |
| A13 ordering fixtures record their clear tile | assertions inside A8–A10 | achievable as stated |
| A14 occluder fixture at ≥ 4 km; cull may decline | shaft fixture, bytes only | achievable as stated |
| A15 hard point, spot, flagged embedded emitter | shaft fixtures | achievable as stated |
| A16 sun bakes with no tile tested | shaft fixture, test counter | achievable as stated |
| A17 cut face: one sub-chart dropped; overlap bit-identical; clear tile beside overlap (O6) | stage fixture | achievable as stated |
| A18 verify mode: clear tile traced too, 1.0 per covered texel, caches equal | verify assertion in every shaft fixture | achievable as stated |
| A19 open floor: one test per covered tile, none per uncovered; clear tile traces no shadow, clip or lead-in segment (O2) | counting fixture | achievable as stated |
| A20 permit side: clear tile at origin and at ~104 m on one long floor triangle; behind-plane drops a pair; reference-parity records clear tile and dropped pair at each floor setting | permit assertions | achievable as stated |
| A21 existing parity: production = reference per floor setting; embedded alike; worker counts and warm = cold; overlap twins across cuts | existing tests, run unchanged | achievable as stated |
| M1 cold whole-PRL `cmp`, base vs new, 6 floored + 2 floor-off maps | integrating executor, prl-build A/B | manual |
| M2 warm on base-filled cache: section memo hits, layer hits under re-keyed `--direction-texel-scale`, PRL = cold | integrating executor, logs | manual |
| M3 any moved byte → stop, report count, largest change, f64 re-test | conditional on M1/M2 | manual |
| M4 grep gate on diff | integrating executor | manual |
| M5 wall time: hallway-mini ≤ 0.62×, groups-capture median of 3 ≤ 0.65× | owner may launch long runs | manual |
| M6 AnimWeightMaps and shadowmask-estimate share on one cold animated-light bake; roadmap entry if material | integrating executor | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split the clipped chart walk out of `lightmap_layer.rs`, behavior-preserving, own commit | delegated (split) | — | done: 50cd0aeb5, `lightmap_layer` 49 passed, clippy clean |
| 2 | Margin derivation in `research.md`; constants into this plan | delegated (derivation), reviewed by integrating executor | — | done: research §Margin derivation, randomized f32 MT checks at ≤ 42% of each bound |
| 3 | Chart coverage cull in `LayerPartitions::plan_charts` + culls-off seam through `bake_light_layer_chart_controlled` + A1–A6 | delegated, integrated by executor | 1 | done: `chart_cull` 13 passed; `lightmap_stage lightmap_layer chart_cull` 112 passed / 4 ignored; stage-path suites 280 passed / 9 ignored; clippy clean |
| 4 | Tile shaft cull + verify mode + test counters in the chart walk + A7–A19 | delegated, integrated by executor | 2, 3 | in progress |
| 5 | Permit side and reference parity (A20, A21) | integrating executor | 4 | |
| 6 | Manual gates M1–M4, M6; M5 baseline + runs | integrating executor / owner | 5 | |
