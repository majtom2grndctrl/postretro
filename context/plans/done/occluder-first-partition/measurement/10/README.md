# occluder-first-partition: Task 10 compiler-only re-check (after the review fixes)

Recorded 2026-10-29 on Windows 11 (6 logical cores, SATA SSD) at `claude/mac-agent-review-panel-ee30a1`, HEAD `e2d08bb29`, which holds the review fixes from agents A (doors and hulls), B (classifier and clusters) and C (wall free ends, cap, shipped cells). It re-measures what those fixes changed on the compiler against Task 9l's cap26 numbers (`../9l/remeasure/tables.md`, recorded on the Mac at `ad3d2315d`). No source changed: the one scratch test (step 3) was added to `leak_tests.rs`, run, and the file put back; `git status --short` then showed only `measurement/10/`.

Only the compiler's output is re-checked. The engine-side walks use HEAD's harness, whose step cap is 14,000 (`MAX_PORTAL_WALK_STEPS`), so a sweep record's `considered=` is the uncapped cost for every pose under 14,000. 9l counted its "trips" at the old 5,000 cap; the rows below count poses with `considered` over 5,000, which is the same set.

## Binaries and commands

| Role | Commit | `prl-build` (SHA-1 prefix) | `scripts-build` |
|---|---|---|---|
| HEAD | `e2d08bb29` | `f636aaf41260` | `3452968651f5` |
| Windows control | `ad3d2315d` (9l's tree) | `b163e94d0013` | HEAD's copy; `crates/script-compiler` is unchanged between the two commits |

- Builds: `cargo build --release -p postretro-level-compiler --bin prl-build` and `-p postretro-script-compiler --bin scripts-build`, copied to `target/occluder-10/bin/`. The control was built in a detached worktree with `--target-dir` inside this worktree's `target/` (a target dir under the scratchpad's long path fails to link with LNK1104). The control worktree and its target were removed afterwards.
- Bakes: `bake.sh` in this folder, which is 9l's `run9l-remeasure.sh bake` with `sha1sum` for `shasum`: `RUST_LOG=info`, `-v`, `--no-tui`, a private `--cache-dir` per bake (deleted after), and per-map flags (hallway `--sh-probe-spacing 4.0 --no-cache`; arena `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache`; street `--sh-probe-spacing 4.0 --lightmap-density 0.25`; occlusion-test and campaign-test none). The committed stress maps were baked as they are, not regenerated.
- C18 fixture: `cargo test --release -p postretro-level-compiler --bin prl-build walk_from_in_front_of_a_laned -- --include-ignored --nocapture` (runs the laned wall and the new two-layer variant).
- In-process counts: a scratch test over the five maps through `ShippedCells::build` (`partition-counts.out`), cross-checked against the PRLs by `world_poses_structure` (`*/structure.out`); the two agree on every map.
- C18 map sweep: `POSTRETRO_LEAK_SWEEP_PRL=<prl> cargo test --release -p postretro-level-compiler --bin prl-build behind_wall_leak_sweep -- --ignored --nocapture`.
- Walks: `world_poses_ray_sweep` with the committed pose files (`../baselines/<map>/<map>.poses`, 608-pose `../9c/stress-street.rooftop.poses`), 160x90 rays, as 9k/9l.
- Wall times (this machine, not comparable with the Mac's): occlusion-test 16 s, campaign-test 187 s, arena 18 s, street 35 s, hallway 1,348 s (9l on the Mac: occlusion-test 11 s, campaign-test 30 s, arena 11 s, street 19 s, hallway 936 s).

## Platform control (HEAD's harness on PRLs from 9l's compiler, `ad3d2315d`, baked on Windows)

The control reproduces 9l's recorded numbers exactly, so the HEAD-minus-9l deltas below are the fixes and not the platform.

| Quantity | 9l (Mac) | Windows control | Match |
|---|---|---|---|
| occlusion-test portals / interior / slivers | 270 / 119 / 55 | 270 / 119 / 55 | yes |
| street portals / interior / slivers | 3,885 / 1,648 / 1,196 | 3,885 / 1,648 / 1,196 | yes |
| campaign-test portals / interior / slivers | 306 / 136 / 66 | 306 / 136 / 66 | yes |
| arena portals / interior / slivers | 844 / 537 / 0 | 844 / 537 / 0 | yes |
| prisms campaign-test / arena (walls, end planes) | 4, 6 / 136, 166 | 4, 6 / 136, 166 | yes |
| C18 map sweep | 0.00%, 510 cells, 270 portals | 0.00%, 510 cells, 270 portals | yes |
| street over 5,000 / mean / max | 29 / 1,292.5 / 6,467 | 29 / 1,292.5 / 6,467 (`g13.30.0@0`) | yes |
| rooftop over 5,000 / mean / max | 26 / 1,967.2 / 6,608 | 26 / 1,967.2 / 6,608 (`roof:B4.s2.c@180/-15`) | yes |
| M3 seals per mover, campaign-test and arena | m0 5, m1 5; 3 3 5 5 3 5 | the same | yes |

Not controlled: the hallway (a 22 min bake). Its 9l numbers are used as recorded; the four other maps reproducing 9l exactly is the evidence they carry over. Control step 9(b) (the baselines' compiler) was not needed: no pose is over 5,000 that 9l did not also have.

## Verdicts

| Check | Rule | Result |
|---|---|---|
| C18 fixture | 0.00% | PASS: 0.00% over 75 poses x 12,032 samples (0 of 12,032 worst), 76 cells, 49 portals; the two-layer variant 0.00%, 81 cells, 54 portals |
| C18 occlusion-test map sweep | 0.00% | PASS: 0.00%, 0 of 11,538 at every one of 70 poses, 510 cells, 270 portals |
| M3 | every mover that sealed still seals | PASS: hallway m0-m8, campaign-test m0-m1, arena m0-m5 all log at least one seal line; door recognition 9 of 11 / 2 of 3 / 6 of 8, as 9l |
| M4 | no leak or unsealed line | PASS: none in any of the five bake logs |
| New drawn misses | 0 | PASS: `misses` is 0 at every pose on all seven sweeps (five maps, street, rooftop). Unpaired reading; the street's 9 culled-opening poses are the same count 9l reported, the rooftop's 0 |
| Step-cap trips at 14,000 | 0 | PASS: 0 on street (2,960 poses) and rooftop (608); every pose took the portal path |
| 2 x worst walk <= 14,000 | | PASS: worst 6,608 (rooftop), so 13,216 (9l: 13,216) |
| Poses over 5,000 (the 9l "trip" set) | no new poses | PASS: street 29 and rooftop 26, the same poses as 9l's lists |

## Per-map structure (in-process counts, confirmed from the PRLs)

| Map | Metric | 9l | Windows control | HEAD | Delta HEAD vs 9l |
|---|---|---|---|---|---|
| occlusion-test | runtime portals | 270 | 270 | 270 | 0 |
| | interior cells (all) | 119 (510) | 119 (510) | 119 (510) | 0 |
| | slivers < 0.5 m | 55 | 55 | 55 | 0 |
| | prisms: walls, end planes | 8, 14 | – | 9, 13 | +1 wall, -1 plane |
| hallway | runtime portals | 2,926 | – | 2,586 | -340 (-11.6%) |
| | interior cells (all) | 1,859 (7,891) | – | 1,656 (7,645) | -203 (-246) |
| | slivers < 0.5 m | 11 | – | 11 | 0 |
| | prisms: walls, end planes | 263, 285 | – | 135, 154 | -128, -131 |
| campaign-test | runtime portals | 306 | 306 | 306 | 0 |
| | interior cells (all) | 136 (840) | 136 (840) | 136 (840) | 0 |
| | slivers < 0.5 m | 66 | 66 | 66 | 0 |
| | prisms: walls, end planes | 4, 6 | 4, 6 | 4, 6 | 0 |
| arena | runtime portals | 844 | 844 | 798 | -46 (-5.5%) |
| | interior cells (all) | 537 (1,550) | 537 (1,550) | 506 (1,504) | -31 (-46) |
| | slivers < 0.5 m | 0 | 0 | 0 | 0 |
| | prisms: walls, end planes | 136, 166 | 136, 166 | 88, 88 | -48, -78 |
| street | runtime portals | 3,885 | 3,885 | 3,880 | -5 |
| | interior cells (all) | 1,648 (4,864) | 1,648 (4,864) | 1,645 (4,861) | -3 (-3) |
| | slivers < 0.5 m | 1,196 | 1,196 | 1,196 | 0 |
| | prisms: walls, end planes | 17, 28 | – | 7, 11 | -10, -17 |

- The classifier line (`Occluder classification:` brushes, occlude-alone, exempt reasons, clusters, sections, occluding members) is identical to 9l's on all five maps, so the B fixes changed no verdict on shipped content. The structural change is in fewer prisms (agent C's classifier-face gate on the prism floor) and the cells and portals that follow from them.
- Exterior flood-fill leaf counts (full tree, exterior / interior empty): occlusion-test 136 / 428 and campaign-test 241 / 354 are unchanged; arena 206 / 963 -> 206 / 931; street 307 / 2,966 -> 307 / 2,963; hallway 416 / 3,602 -> 416 / 3,393. Exterior counts are unchanged everywhere.
- Every changed count is lower or equal except occlusion-test's prism walls (+1 wall with one fewer end plane; portals unchanged).

## M3: seal lines per mover

| Map | 9l | Windows control | HEAD |
|---|---|---|---|
| hallway | m0 6, m1 6, m2 5, m3 4, m4 6, m5 4, m6 2, m7 1, m8 2 | – | m0 6, m1 6, m2 5, m3 4, m4 6, m5 4, m6 2, m7 1, m8 2 |
| campaign-test | m0 5, m1 5 | m0 5, m1 5 | m0 5, m1 5 |
| arena | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 |
| occlusion-test, street | none | none | none |

Arena m2 and m3 each lose one seal line (5 -> 4) as the prism change removes portals around the doors. The rule is "still seals", and both still do.

## Street and rooftop walks (uncapped, `considered` per pose)

| Set | Metric | 9l | Windows control | HEAD | Delta HEAD vs 9l |
|---|---|---|---|---|---|
| street (2,960) | poses over 5,000 | 29 | 29 | 29 (same poses) | 0 |
| | step-cap trips at 14,000 | – (5,000 cap: 29) | 0 | 0 | 0 |
| | mean | 1,292.5 | 1,292.5 | 1,292.6 | +0.1 |
| | max (worst pose) | 6,467 (`g13.30.0@0`) | 6,467 | 6,467 (`g13.30.0@0`) | 0 |
| | 2 x worst | 12,934 | 12,934 | 12,934 | 0 |
| | total tests | 3,825,818 | 3,825,818 | 3,826,024 | +206 |
| | drawn misses / culled poses | 0 / 9 | 0 / 9 | 0 / 9 | 0 |
| rooftop (608) | poses over 5,000 | 26 | 26 | 26 (same poses) | 0 |
| | step-cap trips at 14,000 | – (5,000 cap: 26) | 0 | 0 | 0 |
| | mean | 1,967.2 | 1,967.2 | 1,968.5 | +1.3 |
| | max (worst pose) | 6,608 (`roof:B4.s2.c@180/-15`) | 6,608 | 6,608 (`roof:B4.s2.c@180/-15`) | 0 |
| | 2 x worst | 13,216 | 13,216 | 13,216 | 0 |
| | total tests | 1,196,083 | 1,196,083 | 1,196,856 | +773 |
| | drawn misses / culled poses | 0 / 0 | 0 / 0 | 0 / 0 | 0 |

HEAD against the control, pose by pose: street 556 of 2,960 poses differ (largest change 60 tests, net +206); rooftop 132 of 608 differ (largest 28, net +773). The 29 street and 26 rooftop poses over 5,000 are the same poses in all three columns, and the worst walk did not move.

## Walks on the other four maps (extra; the recipe did not require them)

Baselines' pose files, 160x90 rays, HEAD's harness. `considered` is portal tests per pose, mean / max.

| Map (poses) | 9l mean / max | HEAD mean / max | Misses (drawn / faceless / culled) | Paths |
|---|---|---|---|---|
| occlusion-test (1,217) | 197.8 / 679 | 197.8 / 679 | 0 / 0 / 0 | portal 1,217 |
| hallway (4,596) | 56.4 / 691 | 52.8 / 759 | 0 / 0 / 0 | portal 4,596 |
| campaign-test (1,448) | 79.6 / 378 | 79.6 / 378 | 0 / 0 / 0 | portal 1,448 |
| arena (5,116) | 38.9 / 407 | 37.0 / 359 | 0 / 0 / 0 | portal 5,116 |

The hallway's mean walk is 6% cheaper but its worst pose is 68 tests (10%) dearer than 9l's; no pose is near the cap. Mean drawn cells: hallway 12.8 -> 11.5, arena 10.3 -> 9.6, the others unchanged. These are unpaired readings (no before-side sweep was re-run here), so they are absolute zero misses, not a paired check.

## What could not be compared

- The hallway has no Windows control (22 min bake); its before-column is 9l's Mac record.
- Wall times are not comparable across machines. Pool rows, bake-gap, mandatory-set and lightmap guardrails (9l's M2 and pool tables) were not re-run: the recipe covers compiler structure, seals, C18 and walks only. Hallway portals fell 11.6%, so the hallway and arena lightmap rows are the ones to re-run if the owner wants them.
- The Mac-side `before` records and the 8a9a341 "before" bakes were not re-created; no pose over 5,000 is new, so no paired reading was needed.

## Files

- `head/<map>/` and `control/<map>/`: `bake.time`, `bake-extract.txt` (classification, door, prism, flood-fill, runtime-cell and seal lines), `seals-per-mover.txt`, `structure.out`, `sweep.out` (HEAD's `.sweep` records stay in `target/occluder-10/`, gitignored). The street adds `rooftop-sweep.out` and the top 40 of each sorted `considered` list (`*.considered.top40.txt`; format `considered pose`, covering every pose over 5,000 when 29 or 26 are over).
- `c18/fixture.out`, `c18/sweep.out` (HEAD); `control/c18-sweep.out`.
- `partition-counts.out`: the in-process counts; `bake.sh`: the bake driver.

## Lightmap rows for the hallway and arena (follow-up)

Run after the report above, on the same HEAD PRLs (`target/occluder-10/head/`), with HEAD's harness. Commands are 9l's (`../9l/remeasure/run9l-remeasure.sh measure`) minus the pairing: `world_poses_bake_gap` with `POSTRETRO_WORLD_POSES_FILE` = the committed `../baselines/<map>/<map>.poses` (hallway 4,596 poses, arena 5,116, both committed; 9l used the same files) and no `POSTRETRO_WORLD_POSES_BEFORE`; the id-51 mandatory-set reader at L = 16 m; and `sh_streaming::reach_measurement::sh_reach_mandatory_bytes_from_prl` with `POSTRETRO_SH_REACH_LEADS=16`. The mandatory-reader is a scratch crate (`../9c/tools/mandatory-reader`, hard-coded Mac path to `level-format`); it was built from a scratchpad copy with that path pointed at this worktree, and it builds against HEAD unchanged. Records: `head/<map>/{bakegap,mandatory,shreach}.out`.

These readings are HEAD against 9l's and the before column recorded in `../9l/remeasure/tables.md`. Not reproducible here: the paired bake-gap verdict and `diff-bakegap` (they need a before `.bakegap` record under the current pool policy, and the harness now rejects records from the old policy; this machine has none and no before build). The bake-gap output therefore reads `UNPAIRED (record only)`. The M2 rule has a form that needs no pairing here: before deferred 0 on both maps (9l's table), and "a walk that deferred nothing before defers nothing after" is an absolute reading.

Harness drift to know about: `walk_epoch` is 4 in these PRLs (9l's were 3) and the step cap is 14,000, so the three walks are not the same frame sequences as 9l's (hallway random walk 96,641 frames against 93,837; arena 115,682 against 98,360). Left-behind texels, a "beside" figure, are therefore not like-for-like with 9l; deferred texels are the gate and are 0 either way. Wall time of the hallway bake-gap: 204 s here (9l, Mac: 924 s).

| Row | Map | Pass rule | before (as in 9l tables) | 9l cap26 | HEAD | Verdict |
|---|---|---|---|---|---|---|
| M2 deferred texels, random / far / tour | hallway | after <= before; 0 stays 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | PASS |
| | arena | same | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | PASS |
| M2 trip frames | hallway, arena | 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 (both) | PASS |
| Left-behind texels beside M2, random / far / tour | hallway | reported | 28.7 M / 38.4 M / 23,233.0 M | 4.8 M / 376.0 M / 23,522.9 M | 85.5 M / 280.5 M / 26,222.1 M | reported; not like-for-like (walk epoch) |
| | arena | reported | 0 / 0 / 9.3 M | 0 / 0 / 17.7 M | 0 / 0 / 16.0 M | reported |
| Pool peak layers (14.0 MiB each), random / far / tour | hallway | reported; 9l's reading: far-point peak equal to before | 34 / 38 / 34 | 31 / 38 / 24 | 38 / 38 / 26 | reported; far peak still 38 (= before), random +4 on 9l, tour 26 (24 occupied + 2 headroom) well under before |
| | arena | reported | 2 / 2 / 2 | 2 / 2 / 2 | 2 / 2 / 2 | reported, unchanged |
| Pool headroom cost, layers | hallway | reported | 0 / 0 / 7 | 0 / 0 / 0 | 0 / 0 / 2 (28.0 MiB on the tour) | reported |
| | arena | reported | 0 / 0 / 0 | 1 / 0 / 0 | 0 / 0 / 0 | reported |
| Pool growths, random / far / tour | hallway | reported | 12 / 6 / 3 | 5 / 9 / 2 | 3 / 6 / 2 | reported |
| Largest id-51 mandatory set (L 16 m), cells over 62.9 M | hallway | reported (the cap is chosen on frame time and walk cost, not on this) | 97.3 M (177) | 104.2 M (371) | **109.2 M (490)** | reported; moved the wrong way against 9l and is above before |
| | arena | reported | <= 2.4 M (0) | 0.9 M (0) | 1.0 M (0) | reported |
| SH mandatory bytes, max (gate floor 256 MiB) | hallway | worst cell under the floor | – | 4.811 MiB | 4.811 MiB | PASS (1.9% of the floor) |
| | arena | same | – | 0.796 MiB | 0.796 MiB | PASS |
| Lightmap mandatory bytes max / pool layers max (shreach) | hallway | reported | – | 347.971 MiB / 34 | 364.653 MiB / 35 | reported |
| | arena | reported | – | 3.161 MiB / 2 | 3.272 MiB / 2 | reported |
| Guardrail: drawn lightmap texels, mean per pose vs before | hallway | reported | 6.15 M | 6.09 M (-1%) | 6.46 M (+5.1%) | reported; gives back 6 points of the narrowing's relief |
| | arena | reported | 0.119 M | 0.096 M (-20%) | 0.097 M (-18.8%) | reported |
| Guardrail: tris in frustum, mean per pose vs before | hallway | reported | 80.3 | 124.0 (+54%) | 124.4 (+54.9%) | reported, unchanged |
| | arena | reported | 65.4 | 94.2 (+44%) | 94.5 (+44.5%) | reported, unchanged |
| Mean portal tests per pose vs before | hallway | reported | 65.3 | 56.4 (-14%) | 52.8 (-19.1%) | reported, better |
| | arena | reported | 58.8 | 38.9 (-34%) | 37.0 (-37.1%) | reported, better |

Before values for the pool, left-behind and mandatory rows are 9l's recorded 9g re-measure (not re-run); the guardrail and portal-test befores are the committed `../baselines/<map>/sweep.out` (9l states those PRLs are byte-identical to 9g's before). The growth counts for before are 9l's `first -> peak, growths` table.

Also reported by this build's unpaired bake-gap print (no 9l value to compare): drawn blocks outside the baked id-51 set, summed over each walk. Hallway 79 / 148 / 130 (random / far / tour); arena 58 / 71 / 0.

Reading: the gate row (M2) holds on both maps. The hallway's id-51 mandatory maximum rose again after the review fixes (104.2 M -> 109.2 M, 371 -> 490 cells over the pool cap), so the hallway is now further above before's 97.3 M than at 9l; deferral is still 0 because the pool grows to demand. The drawn-texel guardrail also moved up on the hallway. The prism count dropping 263 -> 135 walls (fewer end planes) is the likely cause of both, as in 9l's own account of the narrowing giving back part of the prisms' relief; I did not test that attribution.

## After the second review panel

Recorded 2026-10-29 on the final compiler, HEAD `35a587db8` (the second review panel's fixes at `fbf0134fd`, then the owner's sealing-tolerance change: on-face tolerance 1e-5 m, and oblique door edges flush with occluding jamb ends seal). Frozen binaries `prl-build` `441144d88369` and `scripts-build` `3452968651f5` in `target/occluder-10r2/bin/`. Same recipe, maps, flags, pose files and harness as the first section, with no platform control (the first one proved Windows reproduces 9l). Records: `round2/` (`round2/head/<map>/` plus `round2/c18/` and `round2/partition-counts.out`). The held re-bake is in `rebake.md`.

Columns: 9l (Mac, `ad3d2315d`), round 1 (`e2d08bb29`, the first section), round 2 (`35a587db8`). Cross-check against the superseded `fbf0134fd` bakes (not kept): its partition counts, cells, portals, prisms, door recognition and per-mover seal lines were identical to `35a587db8`'s on all five maps, so the sealing change moved no structure and no seal line on these maps. Its walks and lightmap rows were not run.

### Verdicts

| Check | Rule | Result |
|---|---|---|
| C18 fixture (laned, two-layer, new lintel variant) | 0.00% | PASS: 0.00% over 75 poses x 12,032 samples on all three (76 cells / 49 portals, 81 / 54, 73 / 47) |
| C18 occlusion-test map sweep | 0.00% | PASS: 0.00%, 0 of 11,538 at every pose, 510 cells, 270 portals |
| M3 | every mover that sealed still seals | PASS: hallway m0-m8, campaign-test m0-m1, arena m0-m5; door recognition 9 of 11 / 2 of 3 / 6 of 8 |
| M4 | no leak lines | PASS: none in any of the five bake logs |
| New drawn misses | 0 | PASS: `misses` 0 at every pose on all seven sweeps; street culled poses 9, rooftop 0 (as 9l) |
| Step-cap trips at 14,000 | 0 | PASS: 0 on street and rooftop, every pose on the portal path |
| 2 x worst walk <= 14,000 | | PASS: worst 6,608, so 13,216 |
| Poses over 5,000 | no new poses vs 9l's lists | PASS: street 29, rooftop 26, the same poses |
| M2 deferred texels, hallway and arena | after <= before; 0 stays 0 | PASS: 0 on all three walks of both |
| M2 trip frames | 0 | PASS: 0 on both |

### Structure (in-process counts, confirmed from the PRLs by `world_poses_structure`)

| Map | Metric | 9l | Round 1 | Round 2 | Delta vs 9l | Delta vs round 1 |
|---|---|---|---|---|---|---|
| occlusion-test | runtime portals | 270 | 270 | 270 | 0 | 0 |
| | interior (all) cells | 119 (510) | 119 (510) | 119 (510) | 0 | 0 |
| | slivers < 0.5 m | 55 | 55 | 55 | 0 | 0 |
| | prisms walls, end planes | 8, 14 | 9, 13 | 8, 12 | 0, -2 | -1, -1 |
| hallway | runtime portals | 2,926 | 2,586 | 2,924 | -2 | +338 |
| | interior (all) cells | 1,859 (7,891) | 1,656 (7,645) | 1,857 (7,881) | -2 (-10) | +201 (+236) |
| | slivers < 0.5 m | 11 | 11 | 11 | 0 | 0 |
| | prisms walls, end planes | 263, 285 | 135, 154 | 260, 279 | -3, -6 | +125, +125 |
| campaign-test | runtime portals | 306 | 306 | 306 | 0 | 0 |
| | interior (all) cells | 136 (840) | 136 (840) | 136 (840) | 0 | 0 |
| | slivers < 0.5 m | 66 | 66 | 66 | 0 | 0 |
| | prisms walls, end planes | 4, 6 | 4, 6 | 4, 6 | 0 | 0 |
| arena | runtime portals | 844 | 798 | 823 | -21 (-2.5%) | +25 |
| | interior (all) cells | 537 (1,550) | 506 (1,504) | 522 (1,528) | -15 (-22) | +16 (+24) |
| | slivers < 0.5 m | 0 | 0 | 0 | 0 | 0 |
| | prisms walls, end planes | 136, 166 | 88, 88 | 101, 104 | -35, -62 | +13, +16 |
| street | runtime portals | 3,885 | 3,880 | 3,885 | 0 | +5 |
| | interior (all) cells | 1,648 (4,864) | 1,645 (4,861) | 1,648 (4,864) | 0 | +3 |
| | slivers < 0.5 m | 1,196 | 1,196 | 1,196 | 0 | 0 |
| | prisms walls, end planes | 17, 28 | 7, 11 | 8, 12 | -9, -16 | +1, +1 |

- The `Occluder classification` line is identical to 9l's and round 1's on all five maps.
- Exterior flood-fill leaves (exterior / interior empty): occlusion-test and campaign-test unchanged from 9l (136 / 428, 241 / 354). Hallway 416 / 3,602 -> 3,393 -> 3,600; arena 206 / 963 -> 931 -> 947; street 307 / 2,966 -> 2,963 -> 2,966. Exterior counts never moved.
- **The hallway is back at 9l's structure** (portals 2,924 against 2,926, prisms 260 against 263). Round 1 had dropped it to 2,586 portals and 135 prism walls. The second panel's joint and layer gates (F2, F3) restored nearly all of those prisms; I did not isolate which fix did it. The lintel variant that exposed the F2 leak (8.91% mean before the fix) is 0.00% now.
- The arena settled between: 823 portals, 21 below 9l and 25 above round 1, with 101 prism walls against 136 (9l) and 88 (round 1). F3 (panels on longer backing walls get no prism) is the likely reason the arena does not return to 9l's 136; I did not test that. Every arena delta against 9l is a decrease, and no arena walk figure got worse (below).
- Street prisms stay at 8 walls against 17, yet its portals and cells are exactly 9l's.

### M3: seal lines per mover

| Map | 9l | Round 1 | Round 2 |
|---|---|---|---|
| hallway | m0 6, m1 6, m2 5, m3 4, m4 6, m5 4, m6 2, m7 1, m8 2 | same | same |
| campaign-test | m0 5, m1 5 | same | same |
| arena | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m2 4, m3 4, others same | m2 4, m3 4, others same |
| occlusion-test, street | none | none | none |

No door that recognised before stopped sealing, and none newly seals: door recognition is hallway 9 of 11 (movers 0-8), campaign-test 2 of 3 (0, 1), arena 6 of 8 (0-5), occlusion-test 0 of 1, street 0 of 0 in all three columns. Arena m2 and m3 stay at 4 seal lines against 9l's 5, as in round 1; the owner's tolerance change moved no seal line (`fbf0134fd` and `35a587db8` agree).

### Street and rooftop walks (uncapped, `considered` per pose)

| Set | Metric | 9l | Round 1 | Round 2 | Delta vs 9l |
|---|---|---|---|---|---|
| street (2,960) | poses over 5,000 | 29 | 29 | 29 (same poses) | 0 |
| | step-cap trips at 14,000 | 0 (5,000 cap: 29) | 0 | 0 | 0 |
| | mean | 1,292.5 | 1,292.6 | 1,292.5 | 0 |
| | max (worst pose) | 6,467 (`g13.30.0@0`) | 6,467 | 6,467 (`g13.30.0@0`) | 0 |
| | total tests | 3,825,818 | 3,826,024 | 3,825,818 | 0 |
| | drawn misses / culled poses | 0 / 9 | 0 / 9 | 0 / 9 | 0 |
| rooftop (608) | poses over 5,000 | 26 | 26 | 26 (same poses) | 0 |
| | step-cap trips at 14,000 | 0 (5,000 cap: 26) | 0 | 0 | 0 |
| | mean | 1,967.2 | 1,968.5 | 1,967.2 | 0 |
| | max (worst pose) | 6,608 (`roof:B4.s2.c@180/-15`) | 6,608 | 6,608 | 0 |
| | total tests | 1,196,083 | 1,196,856 | 1,196,083 | 0 |
| | drawn misses / culled poses | 0 / 0 | 0 / 0 | 0 / 0 | 0 |
| both | 2 x worst walk | 13,216 | 13,216 | 13,216 | 0 |

Round 2's street and rooftop walks equal 9l's to the test: totals identical, and none of the top-40 `considered` values differ. Round 1's differences (556 street poses, +206 tests) are gone with the street's portals back at 3,885.

### Walks on the other four maps (`considered` mean / max)

| Map | 9l | Round 1 | Round 2 | Misses (drawn / faceless / culled) |
|---|---|---|---|---|
| occlusion-test | 197.8 / 679 | 197.8 / 679 | 197.8 / 679 | 0 / 0 / 0 |
| hallway | 56.4 / 691 | 52.8 / 759 | 56.4 / 691 | 0 / 0 / 0 |
| campaign-test | 79.6 / 378 | 79.6 / 378 | 79.6 / 378 | 0 / 0 / 0 |
| arena | 38.9 / 407 | 37.0 / 359 | 38.6 / 416 | 0 / 0 / 0 |

The hallway's worst pose is 691 again (round 1: 759). The arena's mean is 38.6 against 38.9 and its worst pose 416 against 407 (+2.2%); no pose is near any cap.

### Hallway and arena lightmap rows

Same method as the follow-up section: `world_poses_bake_gap` (unpaired, no before record under the current pool policy), the id-51 mandatory reader at L = 16 m, the SH reach test. The before column is 9l's recorded 9g re-measure. The walks run under walk epoch 4 and the 14,000 cap, so their frame sequences differ from 9l's and left-behind texels are not like-for-like with 9l.

| Row | Map | Rule | Before | 9l | Round 1 | Round 2 | Verdict |
|---|---|---|---|---|---|---|---|
| M2 deferred texels (random / far / tour) | hallway | after <= before; 0 stays 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | PASS |
| | arena | same | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | PASS |
| M2 trip frames | both | 0 | 0 | 0 | 0 | 0 | PASS |
| Left-behind texels | hallway | reported | 28.7 M / 38.4 M / 23,233 M | 4.8 M / 376.0 M / 23,523 M | 85.5 M / 280.5 M / 26,222 M | 8.7 M / 266.5 M / 23,335 M | reported; back near 9l |
| | arena | reported | 0 / 0 / 9.3 M | 0 / 0 / 17.7 M | 0 / 0 / 16.0 M | 0 / 0 / 17.7 M | reported |
| Pool peak layers (random / far / tour) | hallway | reported | 34 / 38 / 34 | 31 / 38 / 24 | 38 / 38 / 26 | 32 / 37 / 24 | reported; far peak 37, under before's 38 |
| | arena | reported | 2 / 2 / 2 | 2 / 2 / 2 | 2 / 2 / 2 | 2 / 2 / 2 | reported |
| Pool headroom cost | hallway | reported | 0 / 0 / 7 | 0 / 0 / 0 | 0 / 0 / 2 | 0 / 0 / 0 | reported |
| | arena | reported | 0 / 0 / 0 | 1 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | reported |
| Pool growths | hallway | reported | 12 / 6 / 3 | 5 / 9 / 2 | 3 / 6 / 2 | 4 / 7 / 2 | reported |
| Largest id-51 mandatory set (cells over 62.9 M) | hallway | reported | 97.3 M (177) | 104.2 M (371) | 109.2 M (490) | **104.2 M (371)** | reported; exactly 9l's |
| | arena | reported | <= 2.4 M (0) | 0.9 M (0) | 1.0 M (0) | 1.0 M (0) | reported |
| SH mandatory max (floor 256 MiB) | hallway | under floor | – | 4.811 MiB | 4.811 MiB | 4.811 MiB | PASS |
| | arena | under floor | – | 0.796 MiB | 0.796 MiB | 0.796 MiB | PASS |
| Lightmap mandatory max / pool layers max | hallway | reported | – | 348.0 MiB / 34 | 364.7 MiB / 35 | 348.0 MiB / 33 | reported |
| | arena | reported | – | 3.161 MiB / 2 | 3.272 MiB / 2 | 3.246 MiB / 2 | reported |
| Drawn lightmap texels, mean per pose vs before | hallway | reported | 6.148 M | 6.085 M (-1.0%) | 6.463 M (+5.1%) | 6.087 M (-1.0%) | reported; round 1's +5.1% is gone |
| | arena | reported | 0.1195 M | 0.0960 M (-19.6%) | 0.0970 M (-18.8%) | 0.0964 M (-19.3%) | reported |
| Tris in frustum vs before | hallway | reported | 80.3 | 124.0 (+54.4%) | 124.4 (+54.9%) | 124.0 (+54.4%) | reported |
| | arena | reported | 65.4 | 94.2 (+44.0%) | 94.5 (+44.5%) | 94.9 (+45.1%) | reported |
| Mean portal tests vs before | hallway | reported | 65.3 | 56.4 (-13.6%) | 52.8 (-19.1%) | 56.4 (-13.6%) | reported |
| | arena | reported | 58.8 | 38.9 (-33.8%) | 37.0 (-37.1%) | 38.6 (-34.4%) | reported |

The 9l percentages here are recomputed from the recorded means to one decimal, so a few differ by a point from the rounded figures in 9l's own tables.

Also reported by the unpaired print: drawn blocks outside the baked id-51 set, hallway 45 / 133 / 92 and arena 18 / 28 / 0 (round 1: 79 / 148 / 130 and 58 / 71 / 0).

### Rows that moved by more than a few percent against round 1

- **Hallway structure, mandatory set and drawn texels all returned to 9l.** The rows that got worse in round 1 (prisms 263 -> 135, mandatory max 104.2 M -> 109.2 M with 371 -> 490 cells over the pool cap, drawn texels -1.0% -> +5.1%, worst walk 691 -> 759) are back to 9l: the mandatory set, its cell count and the worst walk to the digit, the texels within 0.1 point. That fits round 1's drop being prisms the first-round gates refused and the panel restoring them. The mandatory set matching 9l exactly while portals differ by 2 and prisms by 3 walls says those few differing prism planes are not what moves it.
- **Hallway pool far-point peak 38 -> 37, tour headroom 2 -> 0, growths 3 / 6 / 2 -> 4 / 7 / 2:** the same restoration; all reported, none gated.
- **Arena:** portals +25 (+3.1%) and prism walls +13 against round 1, still below 9l; walk mean +4.3% (37.0 -> 38.6) and worst pose 359 -> 416, both toward 9l. All reported.
- No door changed recognition or seals against round 1 or 9l.
