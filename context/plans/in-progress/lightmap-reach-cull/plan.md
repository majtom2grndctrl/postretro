# lightmap-reach-cull — plan of record

mode: compact
status: active
read at: b6fc0c4ee

Worktree: `.claude/worktrees/lightmap-reach-cull`, branch `feat/lightmap-reach-cull`. The main checkout is on another session's branch; do not switch it.

## Corrections
- Brief says the prerequisites "land first" → both are on `main` (`done/face-extract-split-epsilon`, `done/watertight-tracer`); the baseline is `main` at b6fc0c4ee.
- `ray_traversal.rs` → now the directory `ray_traversal/` (`hit_test.rs` holds `TRACER_TOLERANCE_METERS`, re-exported from `ray_traversal`); the coverage proof reads it there.
- `context/lib/build_pipeline.md` step 3 and step 12 already describe the reach cull and coverage proof as "decided, not yet built"; landing rewrites those to built, not adds them.
- Spike prototypes live on local branch `spike/lightmap-flood-cull-sizing` (not on `origin`): `git show 'spike/lightmap-flood-cull-sizing:crates/level-compiler/src/lm_flood_sizing.rs'`.
- `pipeline/lightmap_stage.rs` is 671 lines: new stage wiring (proof switch, stage line, verify switch) goes in new files under `pipeline/lightmap_stage/`, not that file.
- Far-crack AC row ("past about 190 m"): the parity weld merges a 0.535 mm crack's lines only at 320 m (both direction and anchor cells must coincide); the test sits at 320 m and asserts `check_watertight` reports none there.
- Path "return full-tree portals from `RuntimeCells::of_partition`" → the shipped full-tree portals drop pieces under `MIN_PORTAL_AREA_M2` (0.1 m²) and narrower than `PORTAL_EPSILON` (1 cm), so a 30 cm vent or a 7.6 mm slit disconnects two open leaves light passes between (builder experiment). The reach index generates its own full-tree portals with no area floor and a 1e-6 m clip tolerance through a parameterised `generate_portals`; the exterior flood, `of_partition` and runtime cells keep today's portals, so no byte or key moves. A piece narrower than 1e-6 m is bounded by open/solid boundaries whose faces (precondition, coverage proof) lie within τ of any ray through it. Fixture: two rooms joined by a sub-0.1 m² vent within R is kept.
- Slab (clarification, same meaning): a stored 0 is exact when every shadow trace is blocked, so the chart and per-texel slabs hold every point a shadow trace starts from, the lifted origin plus the skipped `RAY_EPSILON`: chart-frame height [ε, 2ε] plus rounding slack, in-plane radius·(1+1e-5) + ε·(1+skew) + slack. A slab reaching height 0 would touch the brush under every face and skip nothing. A pair is culled only when the emitter sphere clears the chart's frame plane by about 2ε with a near-orthonormal frame (as `shaft_cull.rs` checks); otherwise kept.
- AC rows built with meaning-preserving adjustments: row "brush top overhanging that brush's solid" — the [ε, 2ε] slab of a convex brush top never overhangs its own brush, so the crate sits in the far room's side wall and its top slab overhangs the wall's solid; drift row — real drift (~3e-5 rad) is too small to fixture, so the prepared chart frame is tilted 0.007 rad with a hard light, and a face-plane slab seam is the negative control; P4 row — a single-brush wall leaves the occluders equal, so the wall is a 4×4 brush grid; holes-past-cap warm rebuild runs verify off (a true warm build, as the brief's warm gate rows do).
- Gate-map geometry harness to reuse for the hallway proof run: `ray_traversal/census_tests.rs` `map_geometries` (pre- and post-cut geometry of a gate map).

## Delegated answers
- R's footprint term — per light: the largest footprint radius among the charts its range-only reach admits, the bound the layer key's influence AABB already widens by; tight per light and already exact.
- Index storage — per-light leaf bitset (`Vec<u64>` over full-tree leaves); O(1) query, ~1.2 KB per light on the hallway.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Two rooms, solid wall: culled; wall-touching texels traced, rest skipped | `lightmap_reach` fixture test | achievable as stated |
| Doorway portal within R kept; just past R culled | fixture test | achievable as stated |
| Centres in unreached leaf, slab reaches reached leaf: kept | fixture test | achievable as stated |
| Brush-top chart overhanging solid: culled (relaxed) | fixture test | achievable as stated |
| Long oblique drifted chart: bytes equal; fails with face-plane slab | fixture test with plane-slab negative control | achievable as stated |
| Traced texel after skipped run: occluders differ, record = unculled = brute force | fixture test (P4) | achievable as stated |
| Emitter sphere straddling portal seeds both leaves | unit test | achievable as stated |
| `kinematic_mover` brush between light and chart: kept | fixture test | achievable as stated |
| Sun / origin-in-solid: none culled; embedded emitter by floor setting | fixture test | achievable as stated |
| Proof: closed brush with T-junction + overlap zero spans; 2 cm strip removed → spans only there | `coverage_proof` test | achievable as stated |
| Crease moved just under τ proven + culls; just over τ unproven, culls nothing, warns | fixture test (P6) | achievable as stated |
| Crack under τ at quarter points, over between: unproven, culls nothing | fixture test | achievable as stated |
| Crack just over τ past ~190 m: `check_watertight` silent, proof unproven, no cull | fixture test | achievable as stated |
| Hinge: four faces any order zero spans; one removed / two same-direction → unproven | test (P8) | achievable as stated |
| `check_watertight` odd spans but proof proves all: culls, info logs, no hole warning | log-capture test | achievable as stated |
| Faces removed past sample cap: no cull, capped warning + total + cull-off; warm same; unremoved culls; sun swap keeps hole warning, no cull-off line | log-capture stage test | achievable as stated |
| Planted fault (`force` cull) fails verify naming texel and both values | test | achievable as stated |
| Verify re-walk = kept-pair walk: shaft + clip on, caches empty; shaft tests tally equal reach-off walk (P1) | test | achievable as stated |
| One governor permit: bakes; one permit per culled pair walk + re-walk + mask; progress once per pair (P2) | test | achievable as stated |
| Existing parity: oracle per floor, warm = cold, worker counts, overlap texels; two-rooms parity 1 vs many workers (P3) | existing suites + new two-rooms row | achievable as stated |
| Gate: whole-PRL `cmp` on the nine named bakes | gate run, verify on | achievable as stated |
| Gate: verify switch in release binary (1/0/unset/malformed; re-walked = culled cold and warm) | gate run | achievable as stated |
| Gate: proof zero unproven spans on hallway, mini, movement-feel; cull runs; watertight at info | gate run | achievable as stated |
| Gate: hallway Lightmap Bake ≤ 0.65×; SH control; proof time; culled ≥ 40% of walked | timed gate run | achievable as stated |
| Gate: border cost ≤ 10% of skipped | gate run (stage line) | achievable as stated |
| Gate: movement-feel, kinematic-platform within noise | timed gate run | achievable as stated |
| Gate: warm over baseline-filled cache hits section and layers; PRL = cold; stage line says cached | gate run | achievable as stated |
| Gate: grep gate on the diff | grep script | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Coverage proof module (oriented edge balance, bipartite matching per sub-span, τ from the tracer) + its unit tests; run it on hallway, hallway-mini and movement-feel post-cut geometry. Unproven spans on the outer shell → block for owner | delegated (Opus), integrated here | — | done: `coverage_proof` 11 tests pass; gate maps 0 unproven (hallway 48,062 edges, 0.07 s; mini 23,914; movement-feel 1,698). End-stub rule (≤ τ at an edge's own ends) found unsound by independent review (sub-τ stub chains at a hinge; axis-projected stub length) — see task 1b |
| 1b | Replace the end-stub rule (owner ruling 2026-11-01): every edge enters the matching; stand-ins (consumed) stay; the only excuse is a seam overlap ≤ τ in true 3D length between two same-direction edge ends, explained by an opposed pair of turning edges within τ of each other. First log every stub the old rule excuses on the three gate maps. Add both review counterexamples as fixtures that must read unproven. Gate maps must still read zero, else stop and report | delegated (Opus), integrated here | 1, after 2–3 builder finishes | seam-overlap rule built (`seams.rs`, `matching.rs`); review counterexamples are regression fixtures (14 proof tests pass). Gate maps: hallway 0, mini 0, movement-feel **4** unproven (29 µm; three-face corners closing through one face, outside condition (c)). Owner ruling: extend (c) through a closing corner fan (c′, ≤ 4 faces, each link an opposed τ-pair; "turns off the line" = far end > 2τ off the line, not an angle). All three gate maps 0 unproven; 16 proof tests pass. Pending: fresh adversarial review of seam + fan |
| 2 | `lightmap_reach` index: own full-tree portals (no area floor, 1e-6 clip), ball flood, R, chart slab leaves, per-texel mask | delegated (Opus), integrated here | 1 | done: `lightmap_reach::tests` 6 pass (wall, far-room cull + border traces, doorway, 30 cm vent, 2.5 mm slit, sun / light in solid) |
| 3 | Walk integration: `ExactSkips` reach flag, per-chart mask shared across lights, verify mode + `POSTRETRO_LIGHTMAP_REACH_VERIFY`, stage info line, hole and cull-off warnings, `check_watertight` to info | delegated (Opus), integrated here | 2 | done: `reach_cull_tests` 3 pass (two-rooms bytes = unculled at both floors, 1 and 4 workers; planted fault; switch resolver); bin suite 2195 pass; clippy clean. Two cache-counting whole-pipeline tests run with verify off via test-only `with_reach_verify` (they count cache reads verify bypasses) |
| 4 | Fixture suite (two rooms et al.) and coverage-proof fixture rows | delegated, integrated here | 3 | done: all 13 remaining automated rows pass; bin suite 2214 pass, clippy clean. Production changes are `cfg(test)` seams only (`face_removal_seam.rs` acts before proof and BVH, P7) |
| 5 | One counted hallway run (permit side ≥ 40%, border cost) | integrating executor | 3 | done (5f7c5c4ea, cold `--release`, verify off, machine busy so untimed): 45,360 of 83,022 pairs culled (54.6%); 1,732.1 M texels skipped, 13.36 M traced inside culled pairs (0.77%); proof 48,062 edges, 0 unproven, 0.072 s. Lightmap Bake 722 s, SH 125 s under load: timing gate needs idle-machine baseline + new runs |
| 6 | Gate runs: `cmp` set, verify switch, timing, warm, grep | integrating executor | 4, 5 | |
