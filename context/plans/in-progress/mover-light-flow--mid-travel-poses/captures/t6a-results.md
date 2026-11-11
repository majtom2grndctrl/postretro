# Task 6a: enclosed mid poses blend across, results

Measured 2026-11-10 on the AMD Radeon Pro 5300M under Metal, on this branch at 9d9152181 plus the task 6/6a working tree. Depth runs through the base over every interval (task 6); a mid pose that encloses a probe stores no value state there and blends across, in bounce and in direct through the section's across marks, and keeps its dark depth tile (task 6a).

## Builds

| Column | Compiler and engine | Notes |
|---|---|---|
| today | 83dfb5317 (epoch 4) | task 5's captures |
| task 5 | 1ec747309 + task 4, through-base | task 5's captures (`tb`) |
| 5b blend | task 5b's `POSTRETRO_DIAG_MID_ENCLOSED=blend` experiment | task 5b's captures |
| task 6a | this tree, cargo `--release` `prl-build` and `postretro --features capture` | `prl-build --release --no-tui -v -j 6`, cold |
| reference | task 5b's static-brush reference through its CPU model (`t5b/dump/base/room-cpu.json`) | no Chebyshev depth, no dynamic lights |

- Harness: `t6a-harness/cap6a.py` (task 5's `cap.py` views and metrics, task 5b's t′ grids, column `t6a`), `t6a-harness/charts6a.py`. Raw metrics: `t6a-harness/t6a-results.json`.
- Only id 57 changed. With id 57 stripped, both maps' PRLs are byte-identical to task 5's stripped PRLs, so `strip-new` stays the 12d reference.
- id 57 grows by the across block alone: setpiece 11,345,255 → 11,347,295 B (+2,040 B, 25 (cell, mover) pairs), enemy-movers 4,551,526 → 4,552,318 B (+792 B, 15 pairs). Every other block's size is unchanged. Cold bake 20.4 s and 6.8 s wall.

## Rising room interior mean luma

Chart `t6a-luma.png`. t′ every 0.025; selected rows:

| t′ | today | task 5 | 5b blend | task 6a | reference (CPU) |
|---|---|---|---|---|---|
| 0.10 | 1.66 | 7.29 | 1.28 | 1.28 | 0.21 |
| 0.20 | 4.12 | 1.98 | 1.92 | 1.92 | 0.34 |
| 0.30 | 4.77 | 11.14 | 2.39 | 2.39 | 0.27 |
| 0.40 | 5.30 | 3.10 | 2.96 | 2.96 | 1.51 |
| 0.50 | 5.65 | 13.70 | 3.21 | 3.21 | 3.01 |
| 0.55 | 5.77 | 16.79 | 3.31 | 3.31 | 4.13 |
| 0.60 | 6.55 | 5.34 | 5.05 | 5.05 | 3.31 |
| 0.70 | 6.52 | 7.73 | 6.69 | 6.69 | 5.13 |
| 0.80 | 12.82 | 7.15 | 7.15 | 7.15 | 5.70 |
| 0.90 | 12.19 | 8.02 | 8.02 | 8.02 | 7.29 |
| 1.00 | 9.08 | 9.08 | 9.08 | 9.08 | 8.59 |

- **The pulse is gone.** The peak drops from 16.79 (task 5, t′ 0.55) to 3.31. Over 0.0–0.6 the interior is now darker than today everywhere; over 0.80–0.95 it equals task 5 (no enclosed pose there).
- **Task 6a equals task 5b's blend experiment at all 41 t′, to the second decimal.** The experiment interpolated q at bake time by pose index; compose now blends across at runtime, and the knots are evenly spaced with no ramp, so the two agree.
- **Not strictly monotone.** Small dips at the knots t′ 0.2 (2.44 at 0.15 → 1.92) and 0.4 (3.14 at 0.375 → 2.96), and the reference's own dip at 0.6 is mirrored there. Task 5 attributed knot dips to exact closed-pose depth at a knot against permissive depth between knots; not re-diagnosed here.
- The build still reads 1–2.5 luma over the reference mid-travel: the blended later shares (task 5b Question 4), which task 6b replaces.

## Sweeper faces (12d: as built minus stripped, block means)

Cells: crop mean, then +bright max / peaks, −dark max / pits.

West face:

| t′ | today | task 5 | task 6a |
|---|---|---|---|
| 0.20 | −1.7, +2.8/0 −2.5/0 | +0.1, +0.5/0 −0.2/0 | −0.6, +0.5/0 −1.0/0 |
| 0.25 | −17.4, +2.3/0 −17.5/23 | +0.7, +0.8/0 −0.6/0 | −1.8, +1.0/0 −1.9/0 |
| 0.28 | −15.1, +2.7/0 −18.2/15 | +0.8, +0.4/0 −0.3/0 | −0.2, +0.8/0 −1.5/0 |
| 0.30 | −15.5, +2.7/0 −15.8/20 | +0.8, +0.3/0 −0.2/0 | +0.2, +1.0/0 −1.3/0 |
| 0.32 | −14.5, +2.5/0 −14.5/18 | +0.7, +0.2/0 −0.3/0 | +0.2, +0.6/0 −1.0/0 |

East face:

| t′ | today | task 5 | task 6a |
|---|---|---|---|
| 0.00 | −7.1, +2.3/0 −7.6/12 | same | same |
| 0.05 | −5.6, +1.7/0 −7.8/13 | −3.6, +3.2/2 −5.2/5 | −4.3, +2.2/0 −6.7/10 |
| 0.10 | −2.6, +1.6/0 −2.4/0 | −2.9, +2.3/0 −3.4/1 | −3.5, +2.0/0 −4.0/9 |
| 0.143 | −0.7, +0.5/0 −0.9/0 | −5.8, +1.5/0 −5.2/7 | −5.9, +1.6/0 −5.4/8 |
| 0.15 | −0.6, +0.4/0 −0.9/0 | −4.8, +1.6/0 −5.1/12 | −5.0, +1.5/0 −5.3/10 |
| 0.20 | −0.2, +0.4/0 −0.9/0 | −2.7, +3.0/1 −3.1/1 | −2.8, +1.6/0 −4.6/2 |
| 0.25 | +0.6, +0.5/0 −0.7/0 | −2.8, +2.9/0 −1.8/0 | −2.8, +2.2/0 −2.7/0 |
| 0.70–0.85 | +1.1 to +6.0 | −3.6 to −6.6 | equal to task 5 |

- Every face row equals task 5b's blend column exactly.
- West face: within 1.9 levels of stripped with no pits at 0.20–0.32, slightly darker than task 5 at 0.25, where task 5's leak (25% of the east lights through the enclosed pose 1) no longer brightens it.
- East face: darker than task 5 at 0.05–0.20 with more pits, the frame moving toward the static-brush reference. Task 5b's CPU model put that reference at −11.1 and −8.6 against stripped at 0.05 and 0.10, and task 5 +5.5 to +7 levels above it there; stripped is not the truth on this face (task 5b Question 2). No new reference dump was taken for this build.

## AC 19 (compiler, CPU compose against the static-brush reference)

`cargo test -p postretro-level-compiler --bin prl-build mid_pose_compose -- --ignored --nocapture`. The test's CPU compose now honours across marks, as the renderer does.

| Group | Probes | Task 5: bounce / direct | Task 6a: bounce / direct | Termini only |
|---|---|---|---|---|
| em_sweeper, x 1339, t′ 0.25 (dropped from AC 19) | 20 | 7.39 / 15.84 | 6.13 / 9.01 | 20.02 / 65.06 |
| em_sweeper, t′ 0.30 | 20 | 4.19 / 0.00 | **4.19** / 0.00 | 19.54 / 58.57 |
| roof, z 83.6, t′ 0.4 | 64 | 3.57 / 0.00 | **3.57** / 0.00 | 11.09 / 23.34 |
| roof, z 123.0, t′ 0.6 | 64 | 6.04 / 0.00 | **6.04** / 0.00 | 14.51 / 39.70 |

- Both tests still fail the 3-level bar, on bounce alone: the termini-blended later shares, task 6b's subject. Direct passes at every held group.
- The sweeper test now composes t′ 0.30 only, per the restated AC 19; the 0.25 row above is from a run before that edit.
