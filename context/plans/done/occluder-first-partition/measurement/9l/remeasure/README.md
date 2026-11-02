# occluder-first-partition: Task 9l re-measure (prisms narrowed to wall free ends)

Recorded 2026-10-29 on `claude/occluder-first-partition` at HEAD `ad3d2315d`, with Task 9l's narrowing in: prisms are kept only for occluding walls, and their splitters are only the free vertical end planes. The method, flags, pose sets and harness are 9k's (`../../9k/README.md`), which are 9g's, re-rooted to `target/occluder-9l/`. Deviations are listed under Pinning.

No committed source changed. cap19 was a temporary edit that was built, frozen and edited back. `git status --short` was clean after the restore, and a release rebuild of the restored source hashes the same as the frozen cap26 binary (`2bce17fb1f80`).

Every table is generated from the raw records by `collect.py`. The full set is in `tables.md`, and the sections below quote it.

## Configurations

| Side | Compiler | Edit | Binary (SHA-1 prefix) | Records |
|---|---|---|---|---|
| before | the frozen before bakes, `prl-build-8a9a341` | – | – | 9g's HEAD-harness records |
| 9k cap26-noprism | 9k's HEAD with an empty prism set | – | `230962836a71` | 9k's records, unchanged (structure and walks only) |
| 9k cap26 | 9k's HEAD: every occluding face's prism | – | `71fa88a6a140` | 9k's records, unchanged |
| **9l cap26** | HEAD as is | – | `2bce17fb1f80` | full suite |
| **9l cap19** | HEAD | `CELL_SIZE_CAP_M` = 19.5072 in `partition/brush_bsp/cell_cap.rs` | `5c723e0e0275` | full suite |

## Pinning (testing_guide §Resource bounds)

- **Machine.** The baselines' Mac: Intel i9-9980HK, 16 threads. Every row is deterministic CPU work, and wall times are for scale only.
- **Binaries.** `prl-build-{cap26,cap19}` are frozen in `target/occluder-9l/bin/`. They were built with `cargo build --release -p postretro-level-compiler --bin prl-build`, with `scripts-build` beside them (`3f5fe03ae91c`, the same as 9k's). `prl-build-tests-cap26` (`fa59a6bf7810`) is the `prl-build` bin test harness at HEAD, built with `cargo test --release -p postretro-level-compiler --bin prl-build --no-run`, for the C18 rows.
- **Deviation: the harness binaries are used in place from `target/occluder-9k/bin/`, not copied.** These are `postretro-tests`, `uncap9g` and `mandatory-reader`, which 9k reused from 9g. `git diff d2efc11bb HEAD` touches no file outside `crates/level-compiler` and `context/`, so they compile from unchanged source.
- **Bakes.** These are 9k's commands and flags (`run9l-remeasure.sh bake`): `RUST_LOG=info`, `-v`, `--no-tui`, and a fresh private `--cache-dir` per bake, deleted after the bake. The hallway and the arena take `--no-cache`. The hallway took 936 s at cap26 and 851 s at cap19, and the two hallway bakes did not overlap.
- **Before.** No before run was repeated. Every pairing reads 9g's before records, as in 9k.
- **Rays, walks and pose sets.** These are 9k's: 160×90 rays, a 100° HFOV at 16:9, and a near plane of 0.1 m. The bake gap uses completed reads, L 16 m, pool cap 15, 2000 steps and 2 frames per pose on the tour. The pose sets are `../../baselines/<map>/<map>.poses` and the 608-pose rooftop set `../../9c/stress-street.rooftop.poses`.
- **Uncapped walk.** 9g's `uncap9g`. `xcheck.py` (output in `xcheck.out`) finds 0 differences against the sweep's `considered` at both caps on both sets, and every tripped pose costs over 5,000 uncapped.
- **C18.** The map sweep is `POSTRETRO_LEAK_SWEEP_PRL=<prl> prl-build-tests-cap26 behind_wall_leak_sweep --ignored --nocapture`, run from `crates/level-compiler` on the cap26 and the cap19 occlusion-test PRLs. It uses 70 poses × 11,538 hidden samples. The fixture row is `walk_from_in_front_of_a_laned_wall`.
- **Pilot.** occlusion-test at cap26 ran end to end first: bake, sweep, structure, readers, paired bake gap and the C18 map sweep. It had 1,217 poses, every pairing was valid and every verdict was PASS. The batch (`batch.sh`) then aborted on any INVALID pairing or missing verdict, and none occurred. Every run has the pose count of its set: 1,217 / 4,596 / 1,448 / 5,116 / 2,960, and 608 for the rooftop.
- **Concurrency.** As in 9k, measurements ran in the background while the next bake ran.
- **Disk.** Free space started at 23 GB. The lowest reading was 19 GB, and free space ended at 23 GB. `target/debug/incremental` and the protected records were not touched. The hallway, arena and campaign-test PRLs were deleted once measured.

## Verdicts

| Row | Map / set | 9k cap26 (all prisms) | **9l cap26** | **9l cap19** |
|---|---|---|---|---|
| M1 | occlusion-test, hallway, campaign-test, arena | PASS | PASS | PASS |
| M1 | street level (2,960 poses) | FAIL 312 trips (67 new) | **FAIL 29 trips (0 new)** | **FAIL 41 trips (1 new)** |
| M1 | rooftop (608 poses) | FAIL 168 trips (15 new) | **FAIL 26 trips (0 new)** | **FAIL 37 trips (0 new)** |
| M1 | new drawn misses, every map and set | 0 | 0 | 0 |
| M2 | all five maps, all three walks | PASS | PASS (0 / 0 / 0) | PASS (0 / 0 / 0) |
| M3 | all five | PASS | PASS | PASS |
| M4 | all five | PASS | PASS | PASS |
| C18 | occlusion-test map sweep | 0.00% | **0.00%** (0 of 11,538 at every pose; 510 cells, 270 portals) | **0.00%** (521 cells, 298 portals) |
| C18 | fixture row | 0.00% | **0.00%** over 75 poses × 12,032 (76 cells, 49 portals) | – |

- **M1.**
  - At cap26, every street and rooftop trip also tripped before, so the narrowed prisms bring back no new trip. 9k cap26-noprism had 19 and 27 trips.
  - cap19's one new trip is `g12.26.1@0`. It costs 5,020 tests uncapped, against the 5,000 cap; before, it cost 4,521.
  - The reported culled-opening misses on the street are 9 at cap26 and 11 at cap19. They are not failures.
- **M2.** Deferred texels are 0 on every walk. The street still trips while it walks, so its 0 is read on truncated sets. Its trip frames are 2,525 / 3,315 / 58 at cap26 and 3,424 / 4,248 / 82 at cap19, against 12,588 / 14,214 / 624 for 9k cap26 and 19,554 / 25,858 / 1,036 before. No other map has trip frames.
- **M3.** Every mover that logged a seal line before still logs at least one: hallway m0–m8, campaign-test m0 and arena m0–m5.
- **M4.** No leak or unsealed line anywhere.

## Narrowing cost against no prisms and all prisms (runtime portals; mean portal tests per pose)

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Prisms 9k → 9l (walls, end planes) | 17 → 8, 14 | 1,000 → 263, 285 | 11 → 4, 6 | 238 → 136, 166 | 88 → 17, 28 |
| Runtime portals: before / noprism / all / **9l cap26** / **9l cap19** | 693 / 241 / 430 / **270** / **298** | 4,722 / 2,368 / 5,357 / **2,926** / **3,499** | 524 / 288 / 402 / **306** / **460** | 1,333 / 608 / 1,128 / **844** / **1,188** | 6,288 / 3,756 / 7,667 / **3,885** / **4,381** |
| Portal tests, mean: before / noprism / all / **cap26** / **cap19** | 464 / 180 / 317 / **198** / **216** | 65.3 / 51.2 / 104.3 / **56.4** / **68.0** | 108 / 74 / 100 / **80** / **107** | 58.8 / 32.3 / 47.7 / **38.9** / **53.1** | 2,043 / 1,256 / 1,983 / **1,287** / **1,366** (truncated by trips) |
| Mean portal tests vs before, **cap26 / cap19** | −57 / −54% | **−14 / +4%** | −26 / −1% | −34 / −10% | −37 / −33% |
| Max portal tests per pose, cap26 / cap19 | 679 / 803 | 691 / 904 | 378 / 465 | 407 / 639 | (tripped) |
| Interior cells, cap26 / cap19 | 119 / 130 | 1,859 / 2,151 | 136 / 193 | 537 / 718 | 1,648 / 1,831 |
| Slivers (< 0.5 m), cap26 / cap19 | 55 / 58 | 11 / 11 | 66 / 88 | 0 / 0 | 1,196 / 1,277 |
| PRL faces, cap26 / cap19 | 1,055 / 1,066 | 11,736 / 12,160 | 1,035 / 1,148 | 3,068 / 3,456 | 10,534 / 10,711 |

The 9l cap26 partition counts match 9l's bfwv diagnosis exactly. The street PRL is byte-identical to the bfwv bake.

- **Walk cost.** At cap26, the hallway walks 10% more than no prisms and 14% less than before; with all prisms it walked 60% more than before. The arena walks 20% more than no prisms, and campaign-test 7% more.
- **Portals.** Every map is again under the before build at both caps.

Uncapped portal tests, street and rooftop:

| Set | Side | mean | p95 | max (worst pose) | over 5,000 | 2× max |
|---|---|---|---|---|---|---|
| street | before | 2,553.7 | 8,891 | 18,513 | 518 | 37,026 |
| street | 9k noprism | 1,259.5 | 3,511 | 6,045 | 19 | 12,090 |
| street | 9k all prisms | 2,150.6 | 6,298 | 11,587 | 312 | 23,174 |
| street | **9l cap26** | 1,292.5 | 3,610 | 6,467 (`g13.30.0@0`) | 29 | 12,934 |
| street | **9l cap19** | 1,375.7 | 3,936 | 6,775 (`g6.3.0@180`) | 41 | 13,550 |
| rooftop | before | 4,354.5 | 13,007 | 17,598 | 212 | 35,196 |
| rooftop | 9k noprism | 1,939.7 | 4,696 | 6,466 | 27 | 12,932 |
| rooftop | 9k all prisms | 3,418.5 | 8,013 | 11,955 | 168 | 23,910 |
| rooftop | **9l cap26** | 1,967.2 | 4,785 | 6,608 (`roof:B4.s2.c@180/-15`) | 26 | **13,216** |
| rooftop | **9l cap19** | 2,127.5 | 5,243 | 7,291 (`roof:B4.s2.c@180/-15`) | 37 | **14,582** |

**Implied step cap by §2's 2× rule** over the worst swept walk:
- **cap26 ≥ 13,216** (rooftop worst 6,608);
- **cap19 ≥ 14,582** (rooftop worst 7,291);
- 9k all prisms ≥ 23,910, and no prisms ≥ 12,932.

By H3's offline rate of 0.155 ms per 1,000 tests (release, in process), the worst 9l walk is about 1.0 ms at cap26 and 1.1 ms at cap19.

## Pool and lightmap rows

| Map | Side | Peak layers, random / far / tour (MiB at far peak) | Largest id-51 mandatory set (cells over 62.9 M) |
|---|---|---|---|
| hallway | before | 34 / 38 / 34 (546) | 97.3 M (177) |
| hallway | 9k all prisms | 23 / 32 / 20 (462) | 84.2 M (97) |
| hallway | **9l cap26** | 31 / **38** / 24 (**546**) | **104.2 M (371)** |
| hallway | **9l cap19** | 28 / **35** / 28 (**504**) | **99.3 M (198)** |
| campaign-test | before / 9l cap26 / 9l cap19 | 15–16 / 15 / 15–17 | 37.3 / 34.4 / 35.2 M (0) |
| occlusion-test | before / 9l cap26 / 9l cap19 | 10 / 9 / 10 | 30.6 / 25.6 / 25.3 M (0) |
| arena, street | all | 2 | ≤ 2.4 M |

- **Hallway pool peak.** It is 38 layers (546 MiB) at cap26, the same as before. At cap19 it is 35 layers (504 MiB). 9k's all-prism build peaked lower, at 32 and 29 layers, and 9g cap26 at 44.
- **Cost of the narrowing.** It gives back much of the prisms' id-51 relief on the hallway: the largest mandatory set rises from 84.2 M with all prisms to 104.2 M at cap26, against 97.3 M before and 116.3 M in 9g cap26. cap19 holds it to 99.3 M (198 cells over 62.9 M).
- **Headroom.** The headroom cost is 0 on every hallway walk at cap26. At cap19 it is 2 layers (28 MiB) on the tour.

## Guardrails (per-pose means against before)

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Tris in frustum, noprism / all / **cap26** / **cap19** | +3 / +7 / **+3** / **+3%** | +56 / +71 / **+54** / **+59%** | +73 / +78 / **+73** / **+76%** | +43 / +50 / **+44** / **+56%** | −8 / +31 / **−6** / **−4%** |
| Drawn lightmap texels, noprism / all / **cap26** / **cap19** | −18 / −25 / **−17** / **−20%** | +9 / −13 / **−1** / **−12%** | +10 / +3 / **+9** / **−10%** | −15 / −25 / **−20** / **−28%** | −43 / −39 / **−43** / **−48%** |

- **Tris.** Within 2 points of no prisms at cap26 on every map. The street's +31% with all prisms is gone.
- **Texels.** The narrowing keeps at most half of the prisms' texel saving at cap26: the hallway is −1%, against +9% without prisms and −13% with all. cap19 draws fewer texels than cap26 on every map.
- **Rooftop.** At cap26, the rooftop's tris in frustum are 2,721 mean (before 3,035, all prisms 3,858), and its drawn texels are 0.52 M (before 0.86 M).
- **M7.** No riser or tread plane bounds a cell: the stairs box holds 6 cells and 7 portals at both caps. The arena box holds 86 cells and 193 portals at cap26 (the same as 9k cap26) and 129 cells and 292 portals at cap19.

## cap26 against cap19

- **cap26 is better on walk cost.**
  - Street / rooftop trips: 29 / 26 at cap26, against 41 / 37 at cap19, including cap19's 1 new trip.
  - 2× figure: 13,216 against 14,582.
  - cap19 has 10–50% more runtime portals on every map, and its mean walks are 6–37% higher.
  - cap19 lifts the hallway walk above before (+4%).
- **cap19 is better on lightmaps.**
  - Hallway pool peak: 35 layers against 38.
  - Largest hallway id-51 set: 99.3 M against 104.2 M, with 198 cells over 62.9 M against 371.
  - Drawn texels are 3–19 points lower.
- **Both caps** pass M2, M3, M4 and C18.

## PRLs kept for the live frame-cost run (H3)

- `target/occluder-9l/cap26/occlusion-test/occlusion-test.prl`
- `target/occluder-9l/cap26/stress-street/stress-street.prl`
- `target/occluder-9l/cap19/occlusion-test/occlusion-test.prl`
- `target/occluder-9l/cap19/stress-street/stress-street.prl`

9k's all-prism H3 PRLs (`target/occluder-9k/{cap26,cap19}/{occlusion-test,stress-street}/*.prl`) were deleted. Their records stay, since `collect.py` reads them. The 9l diagnosis PRLs in `target/occluder-9l/{bfwv,free}/` were left in place. The bfwv street PRL is byte-identical to the 9l cap26 street PRL.

## Files

- `README.md` (this file) and `tables.md` (`python3 collect.py 9k-cap26-noprism 9k-cap26 9l-cap26 9l-cap19 > tables.md`).
- `<config>/<map>/` holds the small records, as in 9k: `sweep.out`, `structure.out`, `diff-sweep.out`, `bakegap.out`, `diff-bakegap.out`, `mandatory.out`, `shreach.out` (summary only), `bake.time` and `bake-extract.txt`. The arena adds `region-*.out`. The street adds `rooftop-sweep.out` and `uncapped-{street,rooftop}.top.txt`.
- `c18/` holds `sweep-cap26.out`, `sweep-cap19.out` and `fixture.out`.
- `xcheck.out` is the uncapped-walk cross-check.
- `run9l-remeasure.sh`, `batch.sh`, `copy-records.sh`, `collect.py` and `xcheck.py` are adapted from 9k:
  - they are re-rooted to `target/occluder-9l/`;
  - `collect.py` reads both prism log formats, and its sides name the task (`9k-<config>`, `9l-<config>`);
  - `batch.sh` deletes each non-H3 PRL once its records are written.
- The raw `.sweep` and `.bakegap` records, the full uncapped per-pose lists, the bake logs and the kept PRLs stay in `target/occluder-9l/`, which is gitignored.
