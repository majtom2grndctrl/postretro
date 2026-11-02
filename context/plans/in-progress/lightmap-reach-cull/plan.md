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
| 1 | Coverage proof module (oriented edge balance, bipartite matching per sub-span, τ from the tracer) + its unit tests; run it on hallway, hallway-mini and movement-feel post-cut geometry. Unproven spans on the outer shell → block for owner | delegated (Opus), integrated here | — | done: `coverage_proof` 11 tests pass; gate maps 0 unproven (hallway 48,062 edges, 0.07 s; mini 23,914; movement-feel 1,698). End-stub rule (≤ τ at an edge's own ends) awaiting owner ruling |
| 2 | `lightmap_reach` index: full-tree portals kept from `RuntimeCells::of_partition`, ball flood, R, chart slab leaves, per-texel mask | delegated (Opus), integrated here | 1 | |
| 3 | Walk integration: `ExactSkips` reach flag, per-chart mask shared across lights, verify mode + `POSTRETRO_LIGHTMAP_REACH_VERIFY`, stage info line, hole and cull-off warnings, `check_watertight` to info | delegated (Opus), integrated here | 2 | |
| 4 | Fixture suite (two rooms et al.) and coverage-proof fixture rows | delegated, integrated here | 3 | |
| 5 | One counted hallway run (permit side ≥ 40%, border cost) | integrating executor | 3 | |
| 6 | Gate runs: `cmp` set, verify switch, timing, warm, grep | integrating executor | 4, 5 | |
