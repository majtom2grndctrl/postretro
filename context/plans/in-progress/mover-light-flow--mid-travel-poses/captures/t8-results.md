# Task 8: memos per pose, results

Measured 2026-11-11 on the Intel i9 MacBook Pro (`-j 6` under `nice`), on this branch at fb402ea4f plus the task 8 working tree. Every build is this tree's cargo `--release` `prl-build` (`prl-build <map> --no-tui -v -j 6`), against task 7's binary (937ba607d tree) run back to back through the same scenarios. Edited maps are copies in the scratchpad with the dev content symlinked beside them.

## What changed

- The bounce half's memo (`"mover_probe_bounce"`, version 10) holds one entry per (mover, pose): that pose's raw states before its mover's poses reduce. A terminus keeps its changed probes (compact tile and affected bit) and every probe its rays met; a mid pose keeps its stored or base-reading probes over its domain. `reduce_poses`, skipping, the depth floor and the tile range all run after every pose resolves, from memo or bake.
- Key: id 34's volume key, the round-trip bar (irradiance format, BC6H codec version), the perceptual floor and its display; then the pose's kind, its effective bounce count, its posed solid, side textures and their albedo, and at a mid pose only the domain's probe list and (re-baking bounce) its static-patch list. No knot, pose index, pose count, ramp, depth floor or mover id.
- The terminus bounce-one-over-domain memo the plan's Corrections named is moot: since task 6b a terminus gathers bounce one and its later bounces over every static patch and every valid probe, never over the domain, so its full-state memo needs no domain and a middle-waypoint move keeps it. No such memo was added.
- The direct memo already folded every pose (task 3, version 7). Unchanged.

## Timing (wall, seconds)

| Map | Scenario | Task 7 | Task 8 | Bounce memo, task 8 |
|---|---|---|---|---|
| setpiece | cold `--release` | 25.51 | 25.44 | — |
| setpiece | fill (cold, cache on) | 25.54 | 24.64 | 21 poses baked |
| setpiece | warm, no edit | 0.99 | 1.10 | 21 of 21 hit |
| setpiece | warm, `setpiece_room` `light_poses` 6 → 11 | 7.46 | 2.98 | termini + 4 kept mid poses hit; 5 baked |
| setpiece | warm, `setpiece_room` middle waypoint moved 16 u off its line | 5.53 | 2.60 | termini hit; 4 mid poses baked |
| enemy-movers | cold `--release` | 8.53 | 8.65 | — |
| enemy-movers | fill | 8.63 | 8.42 | 27 poses baked |
| enemy-movers | warm, no edit | 0.54 | 0.61 | 27 of 27 hit |
| enemy-movers | warm, `em_sweeper` `light_poses` 8 → 15 | 3.18 | 1.46 | termini + 6 kept mid poses hit; 7 baked |
| enemy-movers | warm, `em_sweeper` middle waypoint moved 16 u up | 2.43 | 1.35 | termini hit; 6 mid poses baked |

- The middle-waypoint runs fill a cache from a copy with a middle waypoint on the line (`setpiece_room` at 992 320 −8, `em_sweeper` at 1424 256 48) and then move it (to 1008 320 −8, 1424 256 64). Pose counts stay 6 and 8.
- SH Bake stage on the edit runs: setpiece 6.13 → 1.88 s (`light_poses`), 4.25 → 1.50 s (middle waypoint); enemy-movers 2.57 → 0.88 s, 1.85 → 0.79 s.
- Warm no-edit costs about 0.1 s more (SH Bake 0.07 → 0.18 s setpiece, 0.03 → 0.12 s enemy-movers): the lookup now reads and decodes each pose's raw states and reduces every fully-hit mover, where task 7 read reduced states.
- Cold is unchanged within noise (task 7 recorded 24.1–24.5 s and 7.9–8.2 s on an earlier run).
- Direct entries re-bake per light as before on a `light_poses` or waypoint edit (9 of 10 lights setpiece, 10 of 13 enemy-movers): the per-light key folds every pose.

## Byte identity

- Every warm build above equals the same map's `--no-cache` `--release` build byte for byte (fill, no-edit, `light_poses` edit and middle-waypoint move, both maps).
- Every task 8 PRL equals task 7's PRL for the same scenario byte for byte: the restructure moves no output byte.

## On-disk cache, mover probe channel stages

Entries classified by payload shape (`scratchpad/t8/cache_sizes.py`). Everything else is identical between the two binaries.

| Cache | `mover_probe_bounce`, task 7 (per mover) | `mover_probe_bounce`, task 8 (per pose) | `mover_probe_direct` | `mover_absent_bounces` | other stages |
|---|---|---|---|---|---|
| setpiece, after fill | 2 entries, 8.62 MiB | 21 entries, 19.11 MiB | 10, 5.50 MiB | 1, 0.95 MiB | 326.55 MiB |
| setpiece, + `light_poses` edit | 3, 14.52 MiB | 26, 21.58 MiB | 19, 10.95 MiB | same | same |
| setpiece, middle-waypoint pair | 3, 12.18 MiB | 25, 21.22 MiB | 19, 10.91 MiB | same | 326.56 MiB |
| enemy-movers, after fill | 4, 3.87 MiB | 27, 14.00 MiB | 11, 1.98 MiB | 1, 0.50 MiB | 24.74 MiB |
| enemy-movers, + `light_poses` edit | 5, 7.17 MiB | 34, 15.29 MiB | 21, 3.91 MiB | same | same |
| enemy-movers, middle-waypoint pair | 5, 6.04 MiB | 33, 15.50 MiB | 21, 3.88 MiB | same | 24.74 MiB |

- A fresh cache's bounce memo grows 2.2× on the setpiece and 3.6× on enemy-movers: raw termini keep every probe past the round-trip bar (not only affected ones) and every probe whose rays met the pose, which the reduction would drop. Whole-cache cost: +10.5 MiB (3%) on the setpiece, +10.1 MiB (32%) on enemy-movers.
- An edit adds only its new poses' entries (setpiece `light_poses` +2.47 MiB against task 7's +5.90 MiB for a whole new mover entry).

## Scripts and logs

`scratchpad/t8/` (`run.sh`, `scenarios.sh`, `cache_sizes.py`, `logs/`, `out/`, `caches/`).
