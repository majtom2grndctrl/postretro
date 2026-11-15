# Task 7: redundant mid-pose skipping, results

Measured 2026-11-10 on the Intel i9 MacBook Pro (`-j 6` under `nice`), on this branch at 8b9fadb3a plus the task 7 working tree. Cold `--release` bakes with this tree's cargo `--release` `prl-build` (`prl-build <map> --release --no-tui -v -j 6`), against task 6b's binary rerun back to back on the same machine. No engine captures: AC 19's numbers are unchanged (below).

## What runs where

- Skipping runs once per build, after the SH base bake hands back every mover's states (memo hit or bake) and before id 57's bounce and depth blocks lay out and the mover tiles reserve id 59's range (`pipeline/mover_bounce_stage.rs` `skip_mid_poses`, `mover_probe_channel/skip.rs`).
- The per-mover bounce memo still holds each mover's reduced, unskipped states. Its key folds the pose knots as path fractions and never the ramp, so a ramp edit hits every bounce memo and re-decides skips; the memo's payload and stage version are unchanged.
- Each mover's knots run through its ramp by the format's shared rule (`postretro_level_format::mover_probe_channel::{effective_knots, pose_interval, depth_leg}`), which the renderer's install expansion, depth blend and CPU compose now call too.
- Per probe, from knot 0, each interval reaches the farthest later knot (up to the next terminus or base-reading mid pose) such that compose at every stored pose blended across lands within that pose's bars. Bounce: the probe's round-trip bar and the 2-level perceptual floor (`clears_bounce_bars`, the test a mid pose's stored-or-base choice uses). Depth: id 59's significance score against the per-interval through-base blend, below 0.05. Poses the ramp squeezes out are never read and always skip.
- Depth's base in the test is the mover-absent depth the significance floor reads (id 59 before the mover tiles reserve their range, else averaged id-34 moments). The reservation's trim could drop a tile-carrying probe's id-59 tile afterwards; neither map trims (`0 more dropped`).

## Bytes and cost

| | setpiece, 6b | setpiece, 7 | enemy-movers, 6b | enemy-movers, 7 |
|---|---|---|---|---|
| id 57 | 11,292,647 B | 9,144,759 B (−2,147,888, −19.0%) | 4,596,382 B | 3,194,622 B (−1,401,760, −30.5%) |
| bounce payload | 4,791,096 B | 3,988,008 B | 2,143,800 B | 1,763,640 B |
| depth block | 4,020,288 B | 2,675,488 B | 1,865,368 B | 843,768 B |
| CPU-held closed-pose tiles | 10,013 | 6,651 | 4,638 | 2,084 |
| direct block, across marks | 2,392,499 B, 2,040 B | same | 561,402 B, 792 B | same |
| affected probes, depth records, live tiles | 13,403, 1,257, 1,248 | same | 4,570, 847, 749 | same |
| cold `--release` wall (6b rerun / 7, two runs) | 24.0 s | 24.1, 24.5 s | 8.0 s | 7.9, 8.2 s |
| SH Bake | 10.98 s | 11.12, 11.52 s | 6.71 s | 6.73, 6.83 s |
| skip pass | — | 0.04 s | — | 0.02 s |

- Every other section is byte-for-byte the same size: the whole PRL shrinks by exactly id 57's difference on both maps. Two cold builds are byte-identical.
- The wall and SH Bake spread is run-to-run noise; the skip pass itself costs 0.01–0.03 s per mover. Task 6b's recorded 23.6 s / 7.7 s were from an earlier, cooler run.
- Direct q is never skipped, and the across marks are unchanged. No probe left a block on these maps (no ramp, so nothing is squeezed out).

## Skipped against stored (probe, mid pose) states

| Mover | Poses | Bounce kept / skipped | Depth kept / skipped |
|---|---|---|---|
| `setpiece_wall` | 15 | 1,307 / 1,607 (55%) | 2,877 / 1,894 (40%) |
| `setpiece_room` | 6 | 982 / 2,111 (68%) | 1,868 / 1,468 (44%) |
| `em_lift` | 4 | 78 / 146 (65%) | 171 / 99 (37%) |
| `em_sweeper` | 8 | 941 / 807 (46%) | 409 / 1,289 (76%) |
| `em_crusher_open` | 6 | 214 / 239 (53%) | 146 / 330 (69%) |
| `em_crusher_wall` | 9 | 318 / 568 (64%) | 256 / 836 (77%) |

## AC 19 (compiler CPU compose, skipping applied)

The compose test now runs the pipeline's skip pass before laying out the blocks.

| Group | Probes | Task 6b: bounce / direct | Task 7: bounce / direct |
|---|---|---|---|
| em_sweeper, t′ 0.30 | 20 | 1.07 / 0.00 | 1.07 / 0.00 |
| roof, z 83.6, t′ 0.4 | 64 | 1.78 / 0.00 | 1.78 / 0.00 |
| roof, z 123.0, t′ 0.6 | 64 | 1.80 / 0.00 | 1.80 / 0.00 |

Termini only: unchanged (19.54 / 58.57, 11.09 / 23.34, 14.51 / 39.70).
