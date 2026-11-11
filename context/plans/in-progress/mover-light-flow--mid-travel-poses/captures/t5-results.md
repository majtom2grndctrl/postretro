# Task 5: first-slice capture, results

Measured 2026-11-10 on AMD Radeon Pro 5300M under Metal. Owner gate M1 (depth rule) reads from here.

## Builds

| Column | Compiler and engine | Notes |
|---|---|---|
| today | 83dfb5317 (epoch 4), cargo `--release` binaries | `git archive` export, built once, tree deleted |
| through-base, lerp | 1ec747309 + this task's test-only diff, cargo `--release` | depth rule by `POSTRETRO_MOVER_DEPTH_BLEND`, one process per capture |
| stripped | either build's PRL with id 57 removed | the two stripped PRLs are byte-identical, and their frames are pixel-identical in both engines |
| depth stripped (diagnostic) | new PRL with only id 57's depth block removed | floor and interior only |

- PRLs: `prl-build --release --no-tui -v -j 6`, cold.
- Today's id 57 sizes equal the probe-channel 12d record exactly (6,487,380 / 2,275,408 / 42,952 B).
- Captures: 960×540, `sync-proof`, `force_full_resident_sh_compose`, reflections off, every light term. `setpiece_room` or `em_sweeper` forced to t′, other movers unforced.
- Harness and raw metrics: `t5-harness/` (`cap.py`, `strip57.py`, `sheets.py`, `tables.py`, `luma_chart.py`, `results.json`).

## Views and metrics

| View | Camera (map units) | Crop |
|---|---|---|
| Roof top | (680, 320, 300), yaw 0, pitch −30, fov 90 | roof top at z 176t′, x 840–1144, y 168–472 |
| Sweeper west face | (1100, 256, 140), yaw 0, pitch −20, fov 90 | x 1280 + 256t′, y 72–440, z 8–88 |
| Sweeper east face | (1700, 256, 140), yaw 180, pitch −20, fov 90 | x 1312 + 256t′, same y/z |
| Room floor | (860, 320, floor + 64), yaw 0, pitch −28, fov 90 | floor top, x 856–1128, y 184–456 (5,304 blocks) |
| Room interior | (860, 320, floor + 72), yaw 0, pitch −8, fov 100 | whole frame (mean luma) |

Floor top is z −176 + 176t′. Rec. 709 luma of the PNG, 0–255.

- **Blocks.** 8×8-pixel blocks whose four corners land inside the crop.
- **Excess.** A block's value minus the median of the crop blocks within 6 blocks of it.
- **Peaks / pits.** Blocks past +3 / −3 excess that are a local max / min of the block values over their 8 neighbours.
- **12c** (floor) reads each frame's own block means: max excess / peaks.
- **12d** (roof, faces) reads as built minus stripped per block. Cells give the crop mean, then +bright max/peaks, then −dark max/pits.
- **Reproduces the predecessor.** Today's floor max excess matches probe-channel §Task 12c/12d within 0.1 levels (6.5, 8.9, 8.7, 6.8 at 0.80, 0.90, 0.95, 1.00). Roof 0.70 reads −5.6, +3.9/0 −3.2/2 against its −5.8, +4.1/0 −3.1/2.
- **Peak counts are not comparable across tables.** Floor peak counts run 2–5 above the predecessor's, whose exact peak rule was not recorded. Compare peak counts only within this file.

## Roof top (12d)

Sheet `t5-roof-sheet.png`, block map `t5-roof-map.png`, stills `t5-roof-t0.65-{today,through-base,stripped}.png`.

| t′ | today | through-base | lerp |
|---|---|---|---|
| 0.30 | −6.9, +3.9/0 −3.6/0 | −1.4, +1.1/0 −1.4/0 | −1.4, +1.1/0 −1.4/0 |
| 0.40 | −9.9, +7.0/10 −5.7/1 | −0.1, +0.8/0 −0.9/0 | −0.1, +0.8/0 −0.9/0 |
| 0.50 | −13.9, +9.8/8 −8.6/1 | −2.1, +1.5/0 −1.6/0 | −2.1, +1.5/0 −1.6/0 |
| 0.60 | −17.8, +14.9/11 −15.6/3 | −0.3, +0.7/0 −0.7/0 | −0.3, +0.7/0 −0.7/0 |
| 0.65 | −20.0, +14.2/21 −15.0/4 | **−7.6, +4.9/13 −4.2/0** | **−7.1, +4.7/13 −4.2/0** |
| 0.70 | −5.6, +3.9/0 −3.2/2 | −3.4, +2.2/0 −2.3/0 | −3.4, +2.2/0 −2.3/0 |

- 0.30–0.60: the band is gone, within 2.1 levels of stripped with no pits.
- 0.65: broad −7.6 with a faint lattice of 13 peaks.
  - The z 123 layer is open air at 0.65, but it blends 25% toward pose 4's state (t 0.8), which is dark inside the raised room.
  - This is one lattice step of the old residue. It is expected by design and misses M3's 3-level bar.
- 0.70: −3.4, no pits (today −5.6, 2 pits).

## Sweeper west face (12d)

Sheet `t5-sweeper-west-sheet.png`, map `t5-sweeper-west-map.png`, stills `t5-sweeper-west-t0.25-{today,through-base}.png`.

| t′ | today | through-base | lerp |
|---|---|---|---|
| 0.20 | −1.7, +2.8/0 −2.5/0 | +0.1, +0.5/0 −0.2/0 | +0.1, +0.5/0 −0.2/0 |
| 0.25 | −17.4, +2.3/0 −17.5/23 | +0.7, +0.8/0 −0.6/0 | +0.7, +0.8/0 −0.7/0 |
| 0.28 | −15.1, +2.7/0 −18.2/15 | +0.8, +0.4/0 −0.3/0 | +0.6, +0.6/0 −0.6/0 |
| 0.30 | −15.5, +2.7/0 −15.8/20 | +0.8, +0.3/0 −0.2/0 | +0.8, +0.3/0 −0.3/0 |
| 0.32 | −14.5, +2.5/0 −14.5/18 | +0.7, +0.2/0 −0.3/0 | +0.7, +0.2/0 −0.3/0 |

The residue is gone under both rules: within 1 level of stripped, no pits. Today reads 14.5–17.4 dark with 15–23 pits.

## Sweeper east face (12d)

Sheet `t5-sweeper-east-sheet.png`, map `t5-sweeper-east-map.png`, stills `t5-sweeper-east-t0.80-{today,through-base}.png`.

| t′ | today | through-base | lerp |
|---|---|---|---|
| 0.00 | −7.1, +2.3/0 −7.6/12 | −7.1, +2.3/0 −7.6/12 | −7.1, +2.3/0 −7.6/12 |
| 0.05 | −5.6, +1.7/0 −7.8/13 | −3.6, +3.2/2 −5.2/5 | −4.1, +3.5/4 −4.8/5 |
| 0.10 | −2.6, +1.6/0 −2.4/0 | −2.9, +2.3/0 −3.4/1 | −2.9, +2.3/0 −3.4/2 |
| 0.15 | −0.6, +0.4/0 −0.9/0 | **−4.8, +1.6/0 −5.1/12** | **−5.5, +1.9/0 −5.7/9** |
| 0.70 | +6.0, +1.8/0 −2.7/0 | −3.6, +1.5/0 −1.5/0 | −4.1, +1.8/0 −1.1/0 |
| 0.75 | +1.4, +3.4/10 −1.0/0 | −5.1, +3.0/0 −2.1/0 | −5.1, +3.0/0 −2.1/0 |
| 0.80 | +1.1, +4.1/19 −1.1/0 | −5.8, +5.0/12 −2.9/0 | −5.8, +5.0/12 −2.9/0 |
| 0.85 | +1.2, +2.8/0 −1.0/0 | −6.6, +4.2/9 −3.4/3 | −6.8, +4.2/10 −3.4/3 |

- **Stripped is not the truth on this face.** At t′ 0, a terminus, every build reads −7.6/12 pits: exact rest data, unchanged.
  - The slab legitimately shades the probes in front of its east face, and stripped is the mover-absent frame.
  - So M4's "within 3 levels of stripped" cannot hold near t′ 0 for any build.
- **0.70–0.85.** Today reads brighter than stripped (+1.1 to +6.0), because its probes lerp toward the enclosed t = 1 terminus, which reads the base. The new build reads 3.6–6.8 darker, with 0–3 pits.
  - Which is nearer the truth is not settled here. No static-brush frame exists for a mover face, because a static face is lightmap-lit.
- **0.15 is a regression:** 12 pits (through-base) or 9 (lerp), against today's 0. Not diagnosed.

## Rising room floor (12c)

Sheet `t5-floor-sheet.png` (as rendered) and `t5-floor-boost.png` (levels stretched 0–35%, with the depth-stripped column). Excess map `t5-floor-map.png` (black 0, white +8). Stills `t5-floor-t0.70-{today,through-base,lerp}.png`.

| t′ | today | through-base | lerp | depth stripped | stripped |
|---|---|---|---|---|---|
| 0.50 | 7.1 / 8 [4.2] | 3.9 / 2 [38.9] | 3.8 / 1 [39.2] | 3.9 / 2 [38.9] | 3.8 / 1 [54.6] |
| 0.60 | 6.2 / 8 [6.9] | 8.7 / 10 [20.2] | 8.7 / 10 [20.2] | 6.6 / 8 [31.1] | 2.7 / 0 [63.4] |
| 0.70 | 7.9 / 11 [7.3] | 9.4 / 9 [23.8] | 9.7 / 6 [23.1] | 9.4 / 9 [23.8] | 3.5 / 0 [64.4] |
| 0.80 | 6.5 / 7 [30.5] | 7.3 / 8 [23.5] | 7.3 / 8 [23.5] | 7.3 / 8 [23.5] | 2.5 / 0 [65.5] |
| 0.86 | 7.6 / 8 [32.9] | 8.8 / 8 [23.4] | 8.8 / 8 [23.4] | 8.8 / 8 [23.4] | 3.1 / 0 [65.5] |
| 0.90 | 8.9 / 6 [31.1] | 10.1 / 11 [23.8] | 10.1 / 11 [23.8] | 10.1 / 11 [23.8] | 3.5 / 0 [65.5] |
| 0.95 | 8.7 / 8 [28.2] | 9.3 / 9 [24.3] | 9.3 / 9 [24.3] | 9.3 / 9 [24.3] | 3.2 / 0 [65.5] |
| 1.00 | 6.8 / 5 [26.1] | 6.8 / 5 [26.1] | 6.8 / 5 [26.1] | — | 2.2 / 0 [65.6] |

Cells: max excess / peaks [crop mean luma].

- No 12c diamonds under either rule; 0.86–0.95, the old diamond band, reads alike in both.
- The new build reads 0.6–1.2 levels higher max excess than today over 0.80–0.95. Visually it shows broad shapes, no lattice, and is darker (23–24 against 28–33).
- **Lattice dots from the values, not depth.** With depth stripped, t′ 0.60 shows a clear square lattice (`t5-floor-boost.png`); at the knot, exact closed-pose depth hides it. At 0.70, between knots, a faint lattice shows in the far floor under through-base, lerp and depth-stripped alike.
- 0.50–0.70 is far brighter than today (20–39 against 4–7). The doorway clears the arena floor from t′ ≈ 0.27, so some light is right. Mid-interval depth is permissive, though (below), so part is leak.

## Rising room interior mean luma

Chart `t5-luma.png`.

| t′ | today | through-base | lerp | depth stripped | stripped |
|---|---|---|---|---|---|
| 0.00 | 0.57 | 0.57 | 0.57 | 0.87 | 14.41 |
| 0.05 | 1.15 | 4.38 | 4.37 | 4.38 | 14.64 |
| 0.10 | 1.66 | 7.29 | 7.28 | 7.29 | 14.92 |
| 0.15 | 3.74 | 4.66 | 4.64 | 4.66 | 19.03 |
| 0.20 | 4.12 | 1.98 | 1.98 | 3.56 | 19.70 |
| 0.25 | 4.45 | 7.75 | 7.63 | 7.75 | 20.05 |
| 0.30 | 4.77 | 11.14 | 11.21 | 11.14 | 20.30 |
| 0.35 | 5.09 | 9.12 | 9.05 | 9.12 | 22.64 |
| 0.40 | 5.30 | 3.10 | 3.10 | 6.24 | 23.73 |
| 0.45 | 5.50 | 9.99 | 9.78 | 9.99 | 24.13 |
| 0.50 | 5.65 | 13.70 | 13.74 | 13.70 | 24.44 |
| 0.55 | 5.77 | 16.79 | 16.81 | 16.79 | 24.93 |
| 0.60 | 6.55 | 5.34 | 5.34 | 9.93 | 27.37 |
| 0.65 | 6.60 | 7.96 | 7.45 | 7.96 | 27.84 |
| 0.70 | 6.52 | 7.73 | 7.43 | 7.73 | 28.14 |
| 0.75 | 6.36 | 7.84 | 7.72 | 7.85 | 28.42 |
| 0.80 | 12.82 | 7.15 | 7.15 | 7.19 | 29.12 |
| 0.85 | 13.24 | 7.37 | 7.25 | 7.37 | 29.55 |
| 0.90 | 12.19 | 8.02 | 7.90 | 8.02 | 29.77 |
| 0.95 | 10.91 | 8.78 | 8.61 | 8.78 | 29.96 |
| 1.00 | 9.08 | 9.08 | 9.08 | 9.85 | 30.20 |

- Today rises smoothly (0.6 → 6.6), then jumps to 12.8 at 0.80.
- **New: the interior pulses at the pose spacing.** It dips at the knots (0.20, 0.40, 0.60) and peaks between them (7.3, 11.1, 16.8), under both rules. Over 0.0–0.6 it is brighter than today except at the knots; over 0.80–0.95 it is darker.
- **The depth-stripped column splits the pulse.**
  - Between knots, through-base equals depth-stripped exactly: any blended depth tile carries enough variance that Chebyshev passes everything.
  - At the knots, exact closed-pose depth darkens by 1.6–4.6.
  - Depth-stripped still pulses (3.6 at 0.20, 11.1 at 0.30, 6.2 at 0.40, 16.8 at 0.55, 9.9 at 0.60), so the values pulse too.
  - Suspect, not proven: probe layers enclosed by the floor slab exactly at poses 1–3 (z −152.7, −113.3, −73.9) read the base there.
- Lerp keeps slightly more rejection between knots than through-base (0.65: 7.45 against 7.96; 0.45: 9.78 against 9.99).

## Depth rule (M1): what differs

- **Faces and roof:** a wash. Roof 0.65: lerp −7.1 against through-base −7.6. East 0.15: lerp −5.5/9 pits against −4.8/12. West: equal within 0.2.
- **Floor:** equal except 0.70 (lerp 9.7/6 peaks, through-base 9.4/9). Neither shows 12c's diamonds.
- **Interior:** lerp is up to 0.5 luma darker between knots, so it keeps some rejection. Through-base equals no depth between knots.
- **Neither rule removes the knot pulse.** Depth is exact at a knot and permissive between knots under both.

## AC 19 (compiler, CPU compose against static-brush reference)

Tests in `crates/level-compiler/src/pipeline/mover_mid_pose_compose_tests.rs`, both `#[ignore]`d:
- `the_sweepers_west_face_probes_compose_within_three_levels_of_a_static_slab`
- `the_roof_tops_probes_compose_within_three_levels_of_a_static_room`

Run: `cargo test -p postretro-level-compiler --bin prl-build mid_pose_compose -- --ignored --nocapture` (~30 s).

Bounce is measured on `domain.rs` display levels. Direct uses the same measure at the renderer's direct-SH weight (1.0).

| Group | Probes | Mid poses: bounce / direct | Termini only (`light_poses 2`): bounce / direct |
|---|---|---|---|
| em_sweeper, x 1339 layer, t′ 0.25 | 20 | **7.39** / **15.84** | 20.02 / 65.06 |
| em_sweeper, t′ 0.30 | 20 | **4.19** / 0.00 | 19.54 / 58.57 |
| roof, z 83.6 layer, t′ 0.4 | 64 | **3.57** / 0.00 | 11.09 / 23.34 |
| roof, z 123.0 layer, t′ 0.6 | 64 | **6.04** / 0.00 | 14.51 / 39.70 |

Both tests fail. Termini-only misses everywhere, as required; the mid-pose build misses the 3-level bar in every group.

- **Bounce one is exact.** At the stored knots, bounce one matches the reference's first bounce within 0.04 levels (roof) and 0.34 (sweeper).
- **Roof and sweeper t′ 0.30 bounce: the blended later shares.** The reference's later bounces add 10–15 levels at these probes, and the termini blend falls short by 3–7.
  - On the roof, the raised terminus is dark inside the room and lends a small share.
  - This is the "mid-pose later bounces blend from the termini" Decision, not pose spacing.
- **Sweeper t′ 0.25: the enclosing rule at pose 1** (t 0.143).
  - Pose 1 encloses the x 1339 probes, so it stores bounce mark Base and q 255 for the east lights.
  - Compose blends from that unshadowed state toward pose 2's shadow (q 0), so 25% of the east lights leak: direct 15.8, bounce 7.4.
  - This is the plan's "likeliest artifact" on the slab's trailing face. On the west face frame it does not show (+0.7 vs stripped), because those lights sit behind the face's normal.
- **Not the floor-patch phase** of plan.md's open owner question: bounce one is within 0.34 levels here.

## Bake time and id-57 bytes (cold `--release`, cargo-release `prl-build`, `-j 6`, wall)

| Map | Poses (new) | Bake today → new | id 57 today → new | PRL today → new |
|---|---|---|---|---|
| `mover-closet` | closet_door 4 | 2.64 → 2.69 s | 42,952 → 67,676 B | 6,806,371 → 6,831,095 B |
| `mover-setpiece` | wall 15, room 6 | 19.00 → 21.07 s | 6,487,380 → 11,345,255 B | 49,374,457 → 54,232,332 B |
| `enemy-movers` | lift 4, sweeper 8, crusher_open 6, crusher_wall 9 | 6.20 → 6.64 s | 2,275,408 → 4,551,526 B | 9,357,915 → 11,634,033 B |

id 57 by block, today → new (B):

| Map | Direct block (visibility / own tiles) | Bounce block (index / payload) | Depth block | Mover records |
|---|---|---|---|---|
| closet | 108 → 108 (0 / 0) | 31,276 → 36,776 (172 → 488 / 31,104 → 36,288) | 11,540 → 30,740 | — → 36 |
| setpiece | 2,208,248 → 2,390,459 (26,916 → 169,815 / 2,164,536 → 2,203,848) | 3,504,688 → 4,934,372 (11,104 → 88,628 / 3,493,584 → 4,845,744) | 774,404 → 4,020,288 | — → 120 |
| enemy-movers | 393,664 → 560,610 (4,572 → 23,510 / 382,320 → 530,064) | 1,434,024 → 2,125,356 (5,400 → 25,620 / 1,428,624 → 2,099,736) | 447,656 → 1,865,368 | — → 176 |

- Mover bounce stage per mover: setpiece wall 2.4 → 3.3 s, room 2.8 → 3.6 s; sweeper 1.2 → 1.3 s; the other movers +0.1 s.
- Depth CPU-held closed-pose tiles: setpiece 1,906 → 10,013; enemy-movers 1,102 → 4,638. Live tiles: setpiece 991 → 1,248.
- Both payloads stay under the 8 MiB resident bars.
- Not measured: warm times and `low` fidelity (neither exists yet), and the hallway.
