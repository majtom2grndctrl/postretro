# occluder-first-partition: Task 9k re-measure (prisms and pockets)

Recorded 2026-10-28/29 on `claude/occluder-first-partition` at HEAD `d71c82629`, with Task 9i's shadow prisms and Task 9j's one cell per connected pocket in. The method, flags, pose sets and harness are 9g's (`../9g/README.md`), re-rooted to `target/occluder-9k/`. Deviations are listed under Pinning.

No committed source changed. Each variant was a temporary edit that was built, frozen and edited back, and `git status --short` was clean after each restore. A release rebuild after the last restore hashes the same as the frozen cap26 binary (`71fa88a6a140`).

Every table is generated from the raw records by `collect.py`. The full set is in `tables.md`, and the sections below quote it.

## Configurations

| Side | Compiler | Edit | Binary (SHA-1 prefix) | Runs |
|---|---|---|---|---|
| before | the frozen before bakes, `prl-build-8a9a341` | – | – | 9g's HEAD-harness records (byte-identical to the baselines) |
| 9g-cap26 | Task 9g's cap26: no prisms, no pockets | – | `5910f5856f08` | 9g's records, unchanged |
| cap26-noprism | HEAD with an empty prism set | `let prisms: Vec<ShadowPrism> = Vec::new()` after `shadow_prisms` in `pipeline/runtime_cells.rs` (it logs 0 prisms) | `230962836a71` | structure and walks on all five maps: sweep, rooftop, uncapped |
| **cap26** | HEAD as is | – | `71fa88a6a140` | full suite |
| cap19 | HEAD | `CELL_SIZE_CAP_M` = 19.5072 | `e912c05d6d72` | full suite |
| cap26-f1 | HEAD | `SHADOW_PRISM_LENGTH_FACTOR` = 1.0 | `ead69059b494` | street only: structure, sweep, rooftop, uncapped |

cap26-noprism keeps 9j's pockets, so 9g-cap26 → cap26-noprism isolates the pockets, and cap26-noprism → cap26 isolates the prisms.

## Pinning (testing_guide §Resource bounds)

- **Machine.** The baselines' Mac: Intel i9-9980HK, 16 threads. Every row is deterministic CPU work, and wall times are for scale only.
- **Binaries**, frozen in `target/occluder-9k/bin/`:
  - `prl-build-{cap26,cap19,cap26-noprism,cap26-f1}`, built with `cargo build --release -p postretro-level-compiler --bin prl-build`, with `scripts-build` beside them.
  - `prl-build-tests-cap26`: the `prl-build` bin test harness at HEAD, built with `cargo test --release … --no-run`, for the C18 rows.
  - **Deviation: `postretro-tests`, `uncap9g`, `mandatory-reader`, `probe-reach` and `scripts-build` are 9g's binaries, copied rather than rebuilt.** `git diff d2efc11bb HEAD` touches no file outside `crates/level-compiler` and `context/`. The `postretro` bin links the compiler only for its navmesh tests, so the world-pose harness, the uncapped walk and the readers compile from unchanged source. The hashes are `99d11077425c`, `d72069c66bbe`, `1eec152d2bd7`, `325cddd511c1` and `3f5fe03ae91c`.
- **Bakes.** These are 9g's commands and flags (`run9k.sh bake`): `RUST_LOG=info`, `-v`, `--no-tui`, and a fresh private `--cache-dir` per bake, deleted after the bake. The hallway and the arena take `--no-cache`, as in 9g. The hallway took 882–1,024 s per bake, and hallway bakes never overlapped.
- **Before.** No before run was repeated. Every pairing reads 9g's before records (`target/occluder-9g/before/<map>/`): the sweep, the rooftop sweep, the bake gap (replayed frames) and the uncapped walks. 9g showed these are byte-identical to the baselines, and the harness binary is the same. Bake-side before rows (structure, the bake log) come from `target/occluder-baselines/`, and the before readers come from 9c, as in 9g.
- **Rays and walks.** As in 9g: 160×90 rays, 100° HFOV at 16:9, near 0.1 m. The bake gap uses completed reads, L 16 m, pool cap 15, 2000 steps, and 2 frames per pose on the tour, under 9f's pool policy on every side. Pose sets are `../baselines/<map>/<map>.poses` and the 608-pose rooftop set `../9c/stress-street.rooftop.poses`.
- **Uncapped walk.** This is 9g's `uncap9g`: the unchanged walk with `MAX_PORTAL_WALK_STEPS` raised. `xcheck.py` checks it on every config. Wherever the capped walk did not trip, the uncapped portal tests equal the sweep record's `considered`, with 0 differences across every config and both sets. Every tripped pose costs over 5,000 uncapped.
- **C18 map sweep.** `POSTRETRO_LEAK_SWEEP_PRL=<prl> prl-build-tests-cap26 behind_wall_leak_sweep --ignored --nocapture`, run from `crates/level-compiler`. It uses 9h's lattice: 70 poses × 11,538 hidden samples. The fixture row runs with `walk_from_in_front_of_a_laned_wall`, which is no longer ignored.
- **Pilot.** occlusion-test at cap26 ran end to end first: bake, sweep, structure, readers and a paired bake gap. It had 1,217 poses, every pairing was valid and every verdict was PASS. The batch then aborted on any INVALID pairing or missing verdict, and none occurred. Every run has the pose count of its set (1,217 / 4,596 / 1,448 / 5,116 / 2,960, and rooftop 608).
- **Concurrency.** As in 9g, measurements ran in the background while the next bake ran. Counts are deterministic.
- **Disk.** Free space started at 23 GB, and the lowest reading at a bake boundary was 19 GB. Neither `target/debug/incremental` nor any protected record was touched. Each hallway PRL was deleted once its records were written. The other PRLs were deleted once measured, except the four H3 PRLs below.

## Verdicts

| Row | Map / set | cap19 | **cap26** |
|---|---|---|---|
| M1 | occlusion-test, hallway, campaign-test, arena | PASS | PASS |
| M1 | street level (2,960 poses) | **FAIL** 389 trips (288 also before, **101 new**) | **FAIL** 312 trips (245 also before, **67 new**) |
| M1 | rooftop (608 poses) | **FAIL** 189 trips (164 also before, **25 new**) | **FAIL** 168 trips (153 also before, **15 new**) |
| M1 | new drawn misses, every map and set | 0 | 0 |
| M2 | all five maps, all three walks | PASS (0 / 0 / 0) | PASS (0 / 0 / 0) |
| M3 | all five | PASS | PASS |
| M4 | all five | PASS | PASS |
| C18 | occlusion-test map sweep | PASS, 0.00% (0 of 11,538, every pose) | PASS, 0.00% |
| C18 | fixture row | – | PASS, 0.00% over 75 poses × 12,032 |

- **M1, street.** The street-level and rooftop trips are a regression from 9g (19 and 22 at cap26). The prisms cause it: cap26-noprism trips 19 and 27, all also before. The reported culled-opening misses on the street are 9 at cap26 and 10 at cap19, as in 9g. They are not failures.
- **M2.** The street walks trip heavily: 12,588 / 14,214 / 624 trip frames at cap26, and 14,161 / 16,133 / 778 at cap19. So the street's 0 deferred texels is read on truncated sets, as with the before build's (19,554 / 25,858 / 1,036). The other four maps have no trip frames.
- **M3.** Every mover that logged a seal line before still logs at least one: hallway m0–m8, campaign-test m0, arena m0–m5.
- **M4.** There is no leak or unsealed line anywhere.
- **C18.** cap26-noprism also reads 0.00% on the map, which matches 9h. On the map, prisms are not needed; they are what passes the fixture.

## Prism cost: cap26-noprism → cap26

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Prisms (length-bounded) | 17 (6) | 1,000 (164) | 11 (2) | 238 (68) | 88 (14) |
| Occluder-tier leaves | 313 → 416 | 5,110 → 6,973 | 559 → 606 | 1,131 → 1,429 | 1,897 → 3,345 |
| Cells (interior) | 484 (107) → 592 (174) | 7,465 (1,516) → 10,021 (3,268) | 832 (129) → 879 (165) | 1,393 (395) → 1,718 (683) | 4,792 (1,607) → 6,510 (3,144) |
| Runtime portals | 241 → 430 | 2,368 → 5,357 | 288 → 402 | 608 → 1,128 | 3,756 → 7,667 |
| PRL faces | 1,057 → 1,113 | 11,370 → 13,804 | 1,022 → 1,081 | 2,899 → 3,362 | 10,441 → 11,101 |
| Interior cells under 0.5 m | 48 → 76 | 11 → 28 | 66 → 75 | 0 → 0 | 1,172 → 1,930 |
| Portals tested, mean / max per pose | 180 / 596 → 317 / 1,146 | 51 / 755 → 104 / 1,614 | 74 / 378 → 100 / 568 | 32 / 350 → 48 / 407 | (uncapped below) |
| Tris in frustum, mean | 398 → 415 | 126 → 137 | 175 → 180 | 93 → 98 | 1,777 → 2,549 |
| Drawn lightmap texels, mean | 7.74 → 7.08 M | 6.72 → 5.36 M | 6.23 → 5.84 M | 0.10 → 0.09 M | 0.37 → 0.39 M |

Uncapped portal tests, street and rooftop:

| Set | Side | mean | p95 | max (worst pose) | over 5,000 | 2× max |
|---|---|---|---|---|---|---|
| street | before | 2,553.7 | 8,891 | 18,513 | 518 | 37,026 |
| street | 9g-cap26 | 1,273.9 | 3,560 | 6,068 | 19 | 12,136 |
| street | cap26-noprism | 1,259.5 | 3,511 | 6,045 (`g16.14.0@90`) | 19 | 12,090 |
| street | **cap26** | 2,150.6 | 6,298 | 11,587 (`g5.3.0@180`) | 312 | **23,174** |
| street | cap19 | 2,303.8 | 6,845 | 12,711 (`g5.3.0@180`) | 389 | 25,422 |
| street | cap26-f1 | 2,671.8 | 8,108 | 13,867 (`g6.3.0@180`) | 615 | 27,734 |
| rooftop | before | 4,354.5 | 13,007 | 17,598 | 212 | 35,196 |
| rooftop | 9g-cap26 | 1,913.8 | 4,552 | 6,234 | 22 | 12,468 |
| rooftop | cap26-noprism | 1,939.7 | 4,696 | 6,466 (`roof:B4.s2.c@180/-15`) | 27 | 12,932 |
| rooftop | **cap26** | 3,418.5 | 8,013 | 11,955 (`roof:B4.s2.c@180/-15`) | 168 | **23,910** |
| rooftop | cap19 | 3,650.7 | 8,445 | 13,092 (`roof:B4.s2.c@180/-15`) | 189 | 26,184 |
| rooftop | cap26-f1 | 4,161.1 | 10,120 | 14,153 (`roof:B4.s2.c@180/-15`) | 231 | 28,306 |

- **Prisms roughly double runtime portals** on the hallway, arena and street, and add 78% on occlusion-test.
- **Walk cost.** Mean walk cost rises 34–104% on the four small maps, and the hallway rises from −22% to +60% against before. Street and rooftop uncapped walks rise 71–76% in mean and 85–92% in max.
- **Lightmaps and the pool.** Drawn texels fall on four maps. The hallway's largest id-51 mandatory set falls from 116.3 M (9g) to 84.2 M, with 97 cells over 62.9 M against 714.
- **Implied step cap by §2's 2× rule** over the worst swept walk: **cap26 ≥ 23,910** (rooftop worst 11,955), **cap19 ≥ 26,184** (13,092), and without prisms ≥ 12,932. By H3's offline rate of 0.155 ms per 1,000 tests (release, in process), a 12,000-test walk is about 1.9 ms in process, or about 2.6 ms live by the unconfirmed 1.4× factor.

## Pocket cost: 9g-cap26 → cap26-noprism

Portals barely move, because 9j's pockets split cells but rarely add tier portals.
- **Street.** Interior cells go from 850 to 1,607, mostly window slabs, one cell per window. Runtime portals go from 3,755 to 3,756, and cells under 0.5 m from 422 to 1,172. Street trips stay at 19. Rooftop trips go from 22 to 27, all of them also tripped before, and the max uncapped walk from 6,234 to 6,466. Tris in frustum fall from 2,046 to 1,777.
- **Hallway.** Interior cells go from 1,490 to 1,516, and cells under 0.5 m from 3 to 11.
- **Other maps.** occlusion-test and campaign-test are unchanged. The arena gains 4 multi-pocket tier leaves.

## Prism length factor 1 vs 2 (street, cap26)

| | factor 2 (HEAD) | factor 1 |
|---|---|---|
| Prisms (length-bounded) | 88 (14) | 88 (45) |
| Tier leaves / cells / interior | 3,345 / 6,510 / 3,144 | 3,735 / 7,007 / 3,546 |
| Runtime portals | 7,667 | 8,998 |
| Faces / cells under 0.5 m | 11,101 / 1,930 | 11,368 / 2,120 |
| Street trips (also before / new) | 312 (245 / 67) | 615 (369 / 246) |
| Rooftop trips (also before / new) | 168 (153 / 15) | 231 (192 / 39) |
| Uncapped street mean / max | 2,150.6 / 11,587 | 2,671.8 / 13,867 |
| Uncapped rooftop mean / max | 3,418.5 / 11,955 | 4,161.1 / 14,153 |

Factor 1 is worse on every row. Shorter prisms end at the length bound more often (45 against 14), and each truncated prism adds a far plane. C18 was not run at factor 1.

## M2 pool rows (new policy)

| Map | Side | Peak layers, random / far / tour | Active MiB at peak (max) | first → peak, growths |
|---|---|---|---|---|
| hallway | before | 34 / 38 / 34 | 546 | 15→34, 12 / 15→38, 6 / 15→34, 3 |
| hallway | 9g-cap26 | 37 / 44 / 30 | 630 | 15→37, 7 / 15→44, 10 / 15→30, 2 |
| hallway | **cap26** | 23 / **32** / 20 | **462** | 15→23, 8 / 15→32, 8 / 15→20, 2 |
| hallway | cap19 | 22 / **29** / 19 | **420** | 15→22, 7 / 15→29, 9 / 15→19, 2 |
| campaign-test | before / cap26 / cap19 | peak 16 / 16 / 17 | 238 / 238 / 252 | far tour 1 / 1 / 2 growths, others 0 |
| occlusion-test | before / cap26 / cap19 | 10 / 10 / 9 | 154 / 154 / 140 | ≤ 1 growth |
| arena, street | all | 1–3 | ≤ 56 | 0 |

- **Hallway pool peak** is 32 layers (462 MiB active) at cap26 and 29 layers (420 MiB) at cap19. Both are now under before's 38 layers (546 MiB), against 9g cap26's 44 layers (630 MiB). The headroom cost is 0 on every hallway walk at both caps.
- **Largest id-51 mandatory set** (L 16 m, max per camera cell): hallway before 97.3 M (177 cells over 62.9 M), 9g-cap26 116.3 M (714), cap26 84.2 M (97), cap19 80.2 M (95).
- The retiring-generation transient is still not printed.

## Headline guardrail (per-pose means against before)

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Portals tested, cap26 / cap19 | −32 / −31% | +60 / +66% | −7 / +19% | −19 / +2% | −3 / +2% (truncated by trips) |
| Tris in frustum, cap26 / cap19 | +7 / +8% | +71 / +73% | +78 / +83% | +50 / +58% | +31 / +43% |
| Drawn lightmap texels, cap26 / cap19 | −25 / −25% | −13 / −19% | +3 / −14% | −25 / −30% | −39 / −39% |
| PRL faces, cap26 / cap19 | +9.3 / +9.8% | +63.2 / +68.0% | +33.8 / +48.6% | +50.2 / +62.3% | +45.0 / +47.6% |
| Runtime portals, before → cap26 / cap19 | 693 → 430 / 444 | 4,722 → 5,357 / 5,798 | 524 → 402 / 570 | 1,333 → 1,128 / 1,401 | 6,288 → 7,667 / 8,484 |

- **Reversals.** Against 9g cap26, the hallway walks more than before (+60%, was −18%), and the street's runtime portals exceed before's (7,667 against 6,288).
- **Faces** are up 9–68% on all five maps.
- **M7.** No riser or tread plane bounds a cell (stairs box: 6 cells, 7 portals at every after config). The arena box holds 86 cells and 193 portals at cap26, against 74 and 166 in 9g.
- **SH.** The street's SH mandatory clusters per camera cell reach a median of 467 at cap26 (9g 161, before 326).

## PRLs kept for the live frame-cost run (H3)

- `target/occluder-9k/cap26/occlusion-test/occlusion-test.prl`
- `target/occluder-9k/cap26/stress-street/stress-street.prl`
- `target/occluder-9k/cap19/occlusion-test/occlusion-test.prl`
- `target/occluder-9k/cap19/stress-street/stress-street.prl`

The before PRLs remain in `target/occluder-9g/before/` (hard links to the baselines). Every other 9k PRL was deleted.

## Owner rulings needed

1. **Prisms vs the step cap.** As built, prisms pass C18 on the fixture, but they push the street back to 312 trips at street level and 168 on the rooftop at cap26. Of those, 67 and 15 are new against before, and the 2× re-size figure rises from about 12,500 to 23,910. On the map, C18 passes without prisms. The options include:
   - re-size the step cap to ≥ 23,910 (cap26), if H3 shows such walks are cheap;
   - narrow which faces get a prism;
   - accept the fixture as a known-open guard (9h's option (a)).
2. **Cap value.** cap26 beats cap19 on every walk and trip row: 312/168 against 389/189 trips, and a 2× figure of 23,910 against 26,184. cap19 draws fewer texels and holds a lower hallway pool (29 against 32 layers). Both pass M2, M3, M4 and C18.
3. **Length factor.** Keep 2. Factor 1 is worse on every row.

## Files

- `README.md` (this file) and `tables.md` (`python3 collect.py 9g-cap26 cap26-noprism cap26 cap19 cap26-f1 > tables.md`).
- `<config>/<map>/` holds the small records:
  - `sweep.out`, `structure.out`, `diff-sweep.out`, `bake.time` and `bake-extract.txt`;
  - for the full configs, also `bakegap.out`, `diff-bakegap.out`, `mandatory.out` and `shreach.out` (summary only);
  - the arena's `region-*.out`;
  - on the street, `rooftop-sweep.out` and `uncapped-{street,rooftop}.top.txt` (summary plus the 40 costliest poses).
- `c18/` holds the map sweeps (`sweep-cap26.out`, `sweep-cap19.out`, `sweep-cap26-noprism.out`) and the fixture row (`fixture.out`).
- `run9k.sh`, `batch.sh`, `copy-records.sh`, `collect.py` and `xcheck.py` are adapted from 9g. The uncapped tool is 9g's binary (`../9g/tools/`).
- The raw `.sweep` and `.bakegap` records, the full uncapped per-pose lists, the bake logs and the kept PRLs stay in `target/occluder-9k/`, which is gitignored.
