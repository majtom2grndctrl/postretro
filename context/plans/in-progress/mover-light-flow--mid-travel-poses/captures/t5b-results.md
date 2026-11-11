# Task 5b: diagnosis of the first-slice artifacts

Measured 2026-11-10 on the AMD Radeon Pro 5300M under Metal, at HEAD 8ee526736 plus uncommitted diagnostic switches (listed at the end). This is a diagnosis. Every candidate fix below changes a brief Decision or Acceptance row, so the owner decides.

## Method

- **CPU model of the frame.** The `#[ignore]`d dump tests in `pipeline/mover_mid_pose_diag_tests.rs` record, for every probe in a box around the mover:
  - per pose: enclosure, domain membership, bounce mark, q per gated light, and the pose's bounce and direct along the six axis normals;
  - per t: the bounce and direct composed from the assembled id-57 section by the renderer's piecewise rule;
  - a static-brush reference at t (the mover made static there, all bounces, every baked light's term), today's compose (`light_poses 2`, brief AC 2), and the mover-absent values.
- `t5b-harness/cpu_interior.py` and `cpu_face.py` cast the capture's rays (or sample the face plane) and weigh the 8 probes as `sh_sample.wgsl` does: 0.1-cell normal offset, trilinear × validity × smooth mover backface. They leave out Chebyshev depth and dynamic lights.
- **The CPU model reproduces the GPU frame.** The interior mean reads within about 1 luma of the GPU at all 21 t′, for task 5's build and for today's (`t5b-cpu-model.png`). Here "truth" means the static-brush reference read through this same model.
- **GPU captures.** Task 5's harness (`t5b-harness/cap5b.py` wraps `t5-harness/cap.py`), with through-base depth.
  - Experiment PRLs: cold `--release` bakes by the working tree's `prl-build` with one switch set (`bake.sh`).
  - With no switch set, the working tree bakes PRLs byte-identical to task 5's `new-*.prl` on both maps.
- AC 19 was rerun under each switch.

## 1. Interior luma pulse

**Cause (high confidence): the enclosing rule at mid poses.** Each mid pose t 0.2, 0.4 and 0.6 puts the room's 16-unit floor slab around one probe layer:

| Pose | Floor slab z | Layer it encloses |
|---|---|---|
| t 0.2 | −156.8 to −140.8 | z −152.7 |
| t 0.4 | −121.6 to −105.6 | z −113.3 |
| t 0.6 | −86.4 to −70.4 | z −73.9 |

At pose t 0.8, the z −34.5 layer sits 0.7 u above the floor top, so it is not enclosed. That is why 0.6–1.0 does not pulse.

At the enclosing pose, that layer stores q = 255 and bounce mark Base: id 34, the open shaft, which is bright. At every other pose it stores its true state, which is dark.

Between knots, compose ramps that layer toward the bright state while it is still above the floor, inside the room and in front of every floor fragment, at full backface weight. Probe 4527 (1004, 301, −113.3), along +z:

| | Pose 0 | Pose 1 | Pose 2 (encloses it) | Pose 3 | Poses 4–5 |
|---|---|---|---|---|---|
| Direct (levels) | 0 | 0 | 37.96 | 0 | 0 |
| Bounce (levels) | 0 | 0 | 12.33 | 0 | 0 |

Composed at t′ 0.30 it reads direct 24.9 and bounce 6.2. The reference reads 0.00 and 0.04 there.

In the frame, the crossing layer carries 42% of the probe weight at mid-interval. It accounts for about 10× the excess of everything else (`cpu_interior.py --vs-ref`): z −113.3 at t′ 0.30, z −73.9 at t′ 0.55.

The pulse is mostly the **direct** half. At 0.55 the CPU model reads direct 13.5 against a reference of 3.0, and bounce 6.6 against 1.2.

At a knot, the enclosed layer sits behind the floor. Its 0.03 backface floor still gives it about 7% of the weight. The exact closed-pose depth tile rejects it there, which is task 5's knot dip.

### Stage verdicts

| Stage | Verdict | Evidence |
|---|---|---|
| Placement | clean | 6 poses, knots 0, 0.2 … 1 (L 176, s 39.37). |
| Pose solids | clean | Slab positions as tabled above; pose 4 encloses nothing. |
| Enclosure test | clean, as specified | 60 probes per crossing layer enclosed at exactly one mid pose. |
| Direct q | **cause (rule)** | q 255 at the enclosing pose for lights 2, 3 and 11, which are 0 at every other pose. |
| Bounce states and marks | **cause (rule)** | Mark B at the enclosing pose (12.3 levels); dark S at every other pose. |
| Later shares | not the pulse | They add the residual excess after the fix (Question 4). |
| Unmarked mid poses (b) | clean | Every room probe is in the domain (856 of 856); none is unmarked mid-path. |
| Within-bar Base marks (c) | clean | `POSTRETRO_DIAG_MID_BASE=unmarked`: the GPU interior equals task 5's within 0.01 at all 41 t′, and AC 19 is unchanged. |
| Section bytes | clean | Round-trip decode in the dump; switch-off PRLs byte-identical to task 5's. |
| Renderer install: knots, interval tables, tile rank (d) | clean | The CPU recompute from the section matches the GPU curve within about 1 luma (`t5b-cpu-model.png`). A bake-only change moves the GPU frame exactly as the CPU predicts. `mb_entry_blend` and `mc_interval` read correctly. |
| Compose: depth | not the cause | Permissive between knots, exact at knots; it explains the dips, not the peaks. |
| Sampler weights | clean, by design | The behind-floor layer gets about 7% at a knot; between knots the leaking layer is in front of the floor. |

### Discriminating experiments: interior GPU mean luma

| t′ | today | task 5 | blend (`MID_ENCLOSED=blend`) | dark (`MID_ENCLOSED=dark`) | unm (`MID_BASE=unmarked`) | reference, CPU model |
|---|---|---|---|---|---|---|
| 0.10 | 1.66 | 7.29 | 1.28 | 1.28 | 7.29 | 0.21 |
| 0.20 | 4.12 | 1.98 | 1.92 | 1.92 | 1.98 | 0.34 |
| 0.30 | 4.77 | 11.14 | 2.39 | 2.39 | 11.14 | 0.27 |
| 0.325 | 4.93 | 12.58 | 2.46 | 2.46 | 12.58 | — |
| 0.40 | 5.30 | 3.10 | 2.96 | 2.96 | 3.10 | 1.51 |
| 0.50 | 5.65 | 13.70 | 3.21 | 3.03 | 13.70 | 3.01 |
| 0.55 | 5.77 | 16.79 | 3.31 | 2.97 | 16.79 | 4.13 |
| 0.60 | 6.55 | 5.34 | 5.05 | 5.05 | 5.34 | 3.31 |
| 0.80 | 12.82 | 7.15 | 7.15 | 7.15 | 7.15 | 5.70 |
| 1.00 | 9.08 | 9.08 | 9.08 | 9.08 | 9.08 | 8.59 |

Full series every 0.025 in `t5b-harness/t5b-results.json`. Chart `t5b-luma.png`; stills `t5b-interior-sheet.png` (t′ 0.30, 0.40, 0.55 × today, task 5, blend, dark).

What each hypothesis predicted, and what happened:
- **(a) predicted** that removing Base and q 255 at enclosed mid poses kills the peaks, whichever value replaces them. Observed: blend and dark both turn the pulse into a monotone 0.57 → 9.08.
- **(c) predicted** that making within-bar mid marks unmarked changes the frame. Observed: no change.
- **(d) predicted** a CPU/GPU mismatch. Observed: none.

After the fix, the interior still reads 1–2.5 luma above the reference mid-travel. In the CPU model that excess is bounce: at 0.4, 2.19 against 0.65. It comes from the termini's later shares (Question 4).

## 2. Sweeper east face pits at t′ 0.15

**Cause (medium-high confidence): the rest-state lattice pattern, one lattice step on.** This is not a new error.

At t 0.143–0.15 the east face sits 28 u behind the x 1378.7 probe layer. At t′ 0 it sits 27 u behind the x 1339.4 layer, the same phase. Every build shows 12 pits at t′ 0, which is exact rest data. The task 5 build at 0.15 reads that layer's pose-1 state, which is nearly exact at the face's position.

CPU model of the east face against the static-brush reference (`cpu_face.py`):

| t′ | task 5 − stripped | today − stripped | **reference − stripped** | task 5 − reference, mean (per-point range) |
|---|---|---|---|---|
| 0.00 | −5.9 | −5.9 | −7.0 | +1.0 (−0.7 … +1.2) |
| 0.143 | −5.0 | −1.6 | −6.3 | +1.2 (−0.7 … +1.0) |
| 0.15 | −4.9 | −1.6 | −7.2 | +2.3 (−1.4 … +0.8) |

- The truth is darker than stripped at 0.15. Today's 0 pits come from blending toward a brighter later state, 5.6 levels too bright.
- Probe 2587 (1378.7, 236, 62.7) along +x at 0.15: composed bounce 34.6 and direct 40.4, against the reference's 33.7 and 38.3.

| Stage | Verdict |
|---|---|
| Placement, pose solids, enclosure | clean (the x 1378.7 layer is enclosed only at pose 2, t 0.286) |
| q, bounce at pose 1 | clean, within about 2 levels of the reference |
| Renderer, sampler | clean (the CPU model matches) |
| Metric | **the cause of the "regression" reading**: 12d measures against stripped, which is not the truth on this face (task 5 already noted this at t′ 0) |

**A real artifact on this face is the opposite sign.** Mid-interval, the leading face reads too bright against the reference, by the same mechanism as Question 1:
- At t′ 0.05, 0.10, 0.20 and 0.25 the build reads +7.0, +5.5, +6.1 and +5.4 levels above the reference on mean.
- The cause is x 1339.4 (pose 1) and x 1378.7 (pose 2) reading Base and q 255 at the pose that encloses them.
- Blend halves it: +4.6, +3.2, +3.4, +2.6.
- In the GPU 12d metric against stripped, blend adds pits at 0.05–0.10 (5 → 10 and 1 → 9). That is the frame moving toward the reference, which reads −11.1 and −8.6 against stripped there (`t5b-sweeper-east-sheet.png`).

## 3. Sweeper trailing-face direct leak at t′ 0.25

**Confirmed: the same mechanism as Question 1.** Probe 2735 at x 1339:
- Pose 1 (t 0.143) encloses it, so lights 3 and 4 (east) keep q 255 there; pose 2 has q 0.
- Compose reads poses 1..2 at weight 0.75, so 25% of the east lights leak: direct 15.84 and bounce 7.39 levels.
- Lights 0 and 1 (west) read q 0 at pose 0 and 255 at pose 1.

The west face frame does not show it: within 1 level of stripped, because the east lights sit behind the face's normal.

| Switch | AC 19 sweeper 0.25 (bounce / direct) | 0.30 | Roof 0.4 | Roof 0.6 | West frame 0.25 (12d) |
|---|---|---|---|---|---|
| none (task 5) | 7.39 / 15.84 | 4.19 / 0.00 | 3.57 / 0.00 | 6.04 / 0.00 | +0.68, no pits |
| blend | **6.13 / 9.03** | 4.19 / 0.00 | 3.57 / 0.00 | 6.04 / 0.00 | −1.81, no pits |
| dark | 11.46 / 18.06 | 4.19 / 0.00 | 3.57 / 0.00 | 6.04 / 0.00 | −11.56, 27 peaks |
| unm | 7.39 / 15.84 | unchanged | unchanged | unchanged | +0.68 |

- **Residual under blend is structural.** The probe is inside the slab for t 0.107–0.232, and blending across pose 1 spans t 0–0.286. Any linear blend of pose states spanning the crossing misses at 0.25, here 12.5% of the east lights.
- **Dark is wrong on the lit side.** It darkens west lights before the crossing:
  - East face at 0.05: −15.6 against stripped, 13 pits and 14 peaks.
  - West face at 0.25: −11.6.

## 4. Later-bounce shortfall (measurement)

Each column is the error in display levels, on the reference's bounce one plus that share, against the reference's full state, at AC 19's groups (`diag_later_shares`). The "inside room" rows are a new group: probes inside the rising room's inner box. Termini bounce one is the bake's own domain gather (`keep_poses`).

| Group | Probes | (i) blended termini shares, worst/mean | (ii) id 34's share | (iii) true share's size | Composed (AC 19) |
|---|---|---|---|---|---|
| Sweeper x 1339, t′ 0.25 | 20 | 4.54 / 2.90 | 4.26 / 2.52 | 12.09 / 11.65 | 7.39 / 3.95 |
| Sweeper, t′ 0.30 | 20 | 3.96 / 2.73 | 3.76 / 2.37 | 12.86 / 12.01 | 4.19 / 2.97 |
| Roof z 83.6, t′ 0.4 | 64 | 3.57 / 2.60 | **1.94 / 1.04** | 12.98 / 10.99 | 3.57 / 2.60 |
| Roof z 123, t′ 0.6 | 64 | 6.03 / 4.25 | **1.55 / 0.87** | 13.68 / 11.03 | 6.04 / 4.25 |
| Inside room, t′ 0.4 | 224 | **11.55 / 4.93** | 15.93 / 11.02 | 2.86 / 0.54 | 11.54 / 4.93 |
| Inside room, t′ 0.6 | 224 | **8.88 / 5.06** | 15.99 / 11.12 | 6.23 / 1.43 | 8.88 / 5.06 |

Also measured, and rejected:

| Variant | Sweeper 0.25 / 0.30 | Roof 0.4 / 0.6 | Inside room 0.4 / 0.6 |
|---|---|---|---|
| (iv) Pose bounce one × id 34's later/first ratio | 22.5 / 18.2 | 10.7 / 9.6 | 4.1 / 10.3 |
| (v) Termini shares × the pose's bounce one over the termini's | 19.5 / 16.7 | 6.2 / 9.8 | 2.8 / 4.4 |

- On the roof and the sweeper, (i)'s error equals AC 19's composed miss exactly, because bounce one is exact at the knots. This confirms task 5's attribution.
- **No cheap share source fits everywhere.**
  - id 34's share fixes the roof but lights a moving enclosure's interior 16 levels too bright.
  - Termini shares fail where the mid pose's local visibility matches neither terminus. Inside the room, both termini see the probe in open air, so they lend 5–12 levels of share the sealed room never receives.
  - Near the sweeper's face, the slab's own later radiance is local, and no source has it: (i) and (ii) both miss by about 4.

## Candidate fixes

| Artifact | Candidate | Measured effect | Cost | Changes |
|---|---|---|---|---|
| Interior pulse (Q1), trailing-face leak (Q3), leading-face mid-interval brightening (Q2 side note) | An enclosed **mid** pose blends across for values. Bounce is unmarked; direct uses its neighbours' q interpolated by knot. Depth keeps the dark tile, and termini are unchanged. | Interior peaks gone (0.30: 11.14 → 2.39; 0.55: 16.79 → 3.31; monotone 0.57 → 9.08). Sweeper 0.25 direct 15.84 → 9.03, bounce 7.39 → 6.13. East face mean against the reference about halved. Roof and floor 0.70–1.00 unchanged; floor 0.50 loses the leak's flat wash. | Bounce: an existing wire meaning (unmarked mid). Direct has no "across" value, so either a wire change, or the bake writes interpolated q, which must use post-ramp knots and skipped neighbours (MP9-like) and re-key on ramp edits. | Decision "The enclosing rule holds at every pose" ("A pose whose brushes enclose the probe stores no value state there and reads the base, and keeps the dark depth tile"). Acceptance "A probe enclosed at a mid pose stores no value state there, reads the base, and stores the dark depth tile." The counsel line "an enclosed pose reads the base". `build_pipeline.md` Skipping ("A probe a pose encloses still reads the base there"). |
| East face 0.15 pits (Q2) | None in the bake. Judge this face against a static-brush reference, not stripped. | The build is within −1.4 … +0.8 levels per point of the reference at 0.15. Today is 5.6 brighter on mean. | — | Manual row "em_sweeper's west face at t′ 0.20–0.32, and its east face at 0.70–0.85 and 0.00–0.15: within 3 levels of stripped, with no pits, at `low` and `high`." |
| Sweeper 0.25 direct residual (9.03 under blend) | Not fixable by pose states at this spacing: the crossing sits inside one knot interval. Alternatives are a per-probe crossing window (the rejected rival b) or denser poses. | — | — | AC 19 ("compose bounce and direct within 3 display levels"), owner's bar. |
| Later shares (Q4) | No cheap substitute passes all groups. A correct mid-pose later bounce needs posed later bounces (about 130 s a pose on the hallway, research §Cost) or a near-field later-bounce trace over the domain's patches (not built, not measured). | (ii) would pass the roof only; (v) the room interior only. | — | Decision "Mid-pose later bounces blend from the termini (owner)", including "later bounces are low-frequency". AC 19's bounce half. The brief's Shape-and-rival point that poses describe "a room that passes fully through a probe": true for bounce one and direct, not for its later bounces. |

**Not established.**
- The east face at 0.70–0.85 (task 5's "new reads 3.6–6.8 darker"). By the CPU model, the build reads within 1.1 levels of the reference on mean there, where today reads 1.5–4.6 brighter than the reference (`today − stripped` −0.1 to −2.3 against the reference's −3.8 to −5.1). So the darker task 5 build looks nearer the truth, but I did not dig further.
- Whether the floor at t′ 0.50 under blend has a lattice. Its own-excess peaks rise 2 → 8, which matches today's 8. Visually it is today's doorway light pool, not dots, with the leak's flat wash gone; I did not judge it against a reference.
- `low` fidelity: nothing exists yet.

**AC 19 caveat.** The fixture scene gates every map light, so on the setpiece AC 19's direct includes `setpiece_room_lamp` and `setpiece_room_corner_spot` (dynamic) as if baked. Its roof direct reads 0 at the knots either way. The dumps here filter `is_dynamic`.
