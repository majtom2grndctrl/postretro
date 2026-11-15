# Tasks 9b and 11: projection and regression gates, results

Measured 2026-11-11 on the Intel i9 MacBook Pro (AMD Radeon Pro 5300M, Metal; `-j 6` under `nice`), on this branch at b5ead67c2 plus the task 9b and 11 working tree. Every PRL is fresh, `prl-build <map> --no-tui -v -j 6`:

- **pre**: 83dfb5317's compiler (`t5/bin/prl-build-83df`), epoch 4.
- **parent**: task 2's compiler (`base/prl-build-parent`, the parity baseline): today's bake, two poses per mover, written as epoch 5.
- **new**: this tree's cargo `--release` `prl-build`.

Scripts, logs and PRLs: scratchpad `t11/` (`run.sh`, `batch.sh`, `scripts/`, `logs/`, `gates/`, `out/`).

## Task 9b: the projection counts mid poses

The Build Summary's `--release` bounce-bake estimate per mover is now id 34's recorded bake time T × 2 for the termini, plus T × (the mover's domain static patches / all static patches) per mid pose. It still reads "—" with no recorded time and "none" at `light_bounces 0`.

**Where the share comes from.** `PoseLookup` counts the mover's domain static patches once per mover (`NearFieldDomain::static_patches`, a point-in-box test per patch centroid and no ray). Every mid pose shares the mover's domain, so one count serves them all.
- Cached path: the count is the length of the list the pose memo key already builds.
- No-memo path (`--release`, `--no-cache`): the lookup counts it there.

The bake traces exactly these patches (`pose_bake.rs` `near_field`). AC 29's integration check asserts that the `--release` (no memo) and a cache-filled `high` build print the same share.

Sample, `mover-setpiece --release`:

```
Mover bounce fidelity: high (--release) build; 0 of 2 mover(s) approximate
  kinematic_mover 'setpiece_wall' (#0): 15 pose(s), high; --release bounce bake est. 4.1 s (id 34's recorded 1.1 s × (2 termini + 13 mid pose(s) × 13.0% of static patches)); mid-pose states skipped: bounce 1607 of 2914, depth 1894 of 4771
  kinematic_mover 'setpiece_room' (#1): 6 pose(s), high; --release bounce bake est. 2.5 s (id 34's recorded 1.1 s × (2 termini + 4 mid pose(s) × 6.1% of static patches)); mid-pose states skipped: bounce 2111 of 3093, depth 1468 of 3336
```

**Estimate against measured `--release` minus `--bake-fidelity low` cold** (`--no-cache`). Two rounds were run back to back, each a `--release` build then a `low` build.

| Map | Round | `--release` wall | `low` wall | Measured gap | Sum of estimates | Old ×2 rule |
|---|---|---|---|---|---|---|
| mover-setpiece | 1 | 24.54 s | 17.76 s | 6.78 s | 6.6 s | 4.4 s |
| mover-setpiece | 2 | 24.99 s | 18.18 s | 6.81 s | 6.7 s | 4.4 s |
| enemy-movers | 1 | 8.35 s | 3.72 s | 4.63 s | 4.6 s | 4.0 s |
| enemy-movers | 2 | 8.37 s | 3.99 s | 4.38 s | 4.2 s | 4.0 s |

Per mover, the estimate is set against the gap between the `[MoverLight]` "re-baked … N s" lines (`--release` minus `low`, rounds 1 / 2):

| Mover | Mid poses × domain share | Estimate | Measured gap |
|---|---|---|---|
| `setpiece_wall` | 13 × 13.0% | 4.1 / 4.2 s | 3.8 / 4.1 s (5.4 − 1.6, 5.7 − 1.6) |
| `setpiece_room` | 4 × 6.1% | 2.5 / 2.5 s | 2.8 / 3.5 s (4.3 − 1.5, 4.8 − 1.3) |
| `em_lift` | 2 × 3.5% | 1.1 / 1.0 s | 1.0 / 0.9 s |
| `em_sweeper` | 6 × 4.5% | 1.2 / 1.1 s | 1.3 / 1.4 s |
| `em_crusher_open` | 4 × 1.9% | 1.1 / 1.0 s | 1.0 / 1.0 s |
| `em_crusher_wall` | 7 × 3.4% | 1.2 / 1.1 s | 1.2 / 1.1 s |

- The new rule tracks the `--release` − `low` gap within 3% on both maps. The old ×2 rule undercounted the setpiece by 35%.
- The estimate tracks the extra cost of `high` over `low`, not a mover's whole `--release` bounce bake. That whole bake includes the bounce-one pass and the gathers a `low` build also runs: on the setpiece it is 5.4 s and 4.3 s per mover, against estimates of 4.1 s and 2.5 s.

## Acceptance

### AC 1: rest is unchanged at high fidelity

**Per-probe harness.**
- Command: `MOVER_REST_PAIRS=<parent>=<new>:…` (closet, setpiece, enemy-movers, all `--release`) `POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro-renderer rest_channel_output_equals_the_pre_change_build -- --ignored`.
- What it does: each build loads under `off` and under `sync-proof` (every cluster resident), with no state, then t′ 0 and 1. Every probe either build's channel touches is read back through that load's own indirection word. The comparison takes the base out as C10 does:
  - indirect: (with state) + c × (no state), with c = n − 1 − Σ w from that build's own pose marks;
  - each direct atlas: with − without.
  - On the whole load it also compares the moment set each probe samples: the tile its reference names, else its averaged moments.
- **Result: pass, bit for bit.**

| Map | Probes compared (pre-change / new) | Halves per atlas per t′ | Differing | Probes on a moved base | Sampled moments differing |
|---|---|---|---|---|---|
| mover-closet | 157 (157 / 157) | 22,032 | 0 | 0 | 0 |
| mover-setpiece | 11,403 (11,401 / 11,403) | 1,642,032 | 0 | 0 | 0 |
| enemy-movers | 3,887 (3,885 / 3,887) | 559,728 | 0 | 0 | 0 |

- The channel moves real halves at rest. In `off`, the setpiece moves 1,201,327 indirect and 408,151 per direct atlas at t′ 0, and 342,065 and 108,802 at t′ 1. Enemy-movers moves 285,041 / 37,837 at t′ 0 and 380,878 / 45,093 at t′ 1. The closet moves 15,504 indirect at t′ 0 and none at t′ 1, where the door is open. Both builds move the same counts.
- Probes the new build stores only through mid poses read their base at both termini:
  - (probe, mover) bounce pairs: 50 on the setpiece, 44 on enemy-movers;
  - depth probes: 257 and 276. Each one's sampled moments at t′ 0 and 1 equal its no-state moments, bit for bit.
- Three movers meet at some enemy-movers probes; the generalised base weight covers them.
- **Newly pinned cells: none on these maps.** Old and new PRLs differ only in id 57: ids 27, 34, 35, 41, 45, 50 and 59 are byte-identical on all three maps. The harness judges a probe whose no-state tile moved within one f16 step of the base-free output, and reports a probe stored at L0 by one build alone as a miss.
- Negative control: setpiece parent against the setpiece `low` build (different terminus bounce). The harness fails it at 1.2 M indirect halves, worst 4,034 steps, and finds 58 probes stored at L0 by one build alone.

**The pre-brief compiler's PRL.** The current loader refuses epoch 4 by name, so the harness cannot read `prl-build-83df`'s output directly. Two links tie the parent PRL to it:
- **Bytes.** `prl-build-83df` and the parent compiler differ only in id 57 on all three maps (+140 B, +8,272 B, +4,096 B: the epoch-5 mover records and bounce offsets).
- **Frames.** The pre-brief engine (`t5/bin/postretro-83df`) on the `prl-build-83df` PRL was set against this tree's engine on the parent PRL.
  - Set-up: every mover forced to t 0, 0.25, 0.5 and 1, under `sync-proof` and `off`, reflections off, 960×540.
  - Views: closet stations 2 and 3; setpiece stations 2 and 3 and the roof; sweeper west and east.
  - Result: **0 pixels differ in all 56 frame pairs** (`t11/scripts/frames.py`, `t11/frames.log`).

**End to end, pre-brief engine and PRL against this tree's engine and `--release` PRL, at t′ 0 and 1:**
- 23 of 28 (view, t′, mode) pairs are byte-identical.
- The other five differ at 1 or 2 pixels by 1 level: setpiece roof t′ 0 `off`, sweeper west t′ 0 `off`, and sweeper east t′ 1 under both modes.
- Hypothesis, not confirmed: the depth tile texture grows with the mid-pose-only live tiles. Its height on the setpiece goes from about 25 tile rows to 29 (991 → 1,248 live tiles), and on enemy-movers from about 18 to 27 (473 → 749). Every tile's normalized coordinates move, and hardware bilinear subtexel precision can shift a filtered moment by an ulp.
- What fits: the closet's texture keeps its size (28 live tiles in both builds), and its frames are identical. The per-probe stored moments are identical everywhere.
- No discriminating experiment was run.

### AC 2: `light_poses 2` stores and composes today's blend

- **Stores:** `mover_probe_channel_prl_parity` (`--ignored`, baseline `base/prl-build-parent`). `every_mover_at_light_poses_two_packs_byte_identical_to_the_parent_compiler` passes on all three maps at `--release`. The parent's bake equals the pre-brief one (AC 1, the frame link).
- **Composes, end to end:** the frame comparison above. The parent PRL carries two poses per mover. Composed by this tree's engine, its frames equal the pre-brief engine's at t′ 0, 0.25, 0.5 and 1, 0 pixels apart, in both modes.
- **Composes, renderer unit tests** (GPU, `POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro-renderer mover`, 122 passed):
  - bounce: `c3_bounce_replaces_id_34_with_the_lerp_of_its_states_and_deltas_still_add`, `an_unstored_terminus_composes_id_34_and_adds_no_difference`;
  - direct: `a_single_terminus_gate_composes_s0_ungated_and_their_mix`;
  - depth: `a_live_tile_blends_through_its_probes_base_exact_at_termini_and_midpoint`, `termini_to_termini_depth_runs_through_the_base`.

### AC 3: K1, K2, K3, C7, C10 on `--release`; C9 renderer half against the piecewise mix

The capture targets now compile their maps with `prl-build --release` (`compile_dev_map_with`). The renderer gates read the fresh `--release` PRLs.

| Gate | Command | Result |
|---|---|---|
| K1 | `cargo test -p postretro --features capture --test capture_mover_probe_channel -- --ignored` | **pass.** `sync-proof`: station 2 closed, outside the door, 0.0056 against the reference's 0.0059; station 3 0.0160 against 0.0171; open equals stripped, 0 px differing (13.8901, 29.7218). `off`: 0.0057 / 0.0060 and 0.0160 / 0.0178; open 0 px |
| K2 | same target | **pass.** Wall closed 27.556 against the static reference's 29.041; open 31.164 against stripped 31.158 |
| K3 | `cargo test -p postretro --features capture --test capture_mover_promotion_pop -- --ignored` | **pass.** Static: setpiece pop 5.025 at t 0 and 5.495 at t 1, against ungated 5.496 + 12; closet 0. Animated: 0.000 and 1.062 against 1.063 + 12 |
| C7 | `MOVER_C7_PRLS=closet:setpiece-bar0 … mover_channel_lag -- --ignored` | **pass.** Closet: 144 probes, 15,500 indirect halves moving, 0 off. Setpiece at bar 0: 11,358 probes; 1,189,801 indirect and 421,084 per direct atlas moving, 0 off. Residency lag 0 off |
| C10 (per probe) | `MOVER_C10_PRLS=closet:setpiece:setpiece-bar0:enemy-movers … c10_off_and_sync_proof … -- --ignored` | **pass**, enemy-movers included now that the harness takes any mover and pose count. Worst outputs: setpiece several movers 1.0 step, direct 0.99; enemy-movers several movers (up to three) 0.90, one mover reading id 34 0.92, direct 0.88. Depth tiles 0 halves apart in every case |
| C9, renderer | `MOVER_CLOSET_PRL=… POSTRETRO_SH_STREAMING=off … c9_the_closet_composes_and_writes_the_piecewise_mix_at_every_handed_t -- --ignored` | **pass.** 4 knots (0, ⅓, ⅔, 1), so 7 reference frames: every knot plus every pair's midpoint. 15,508 atlas halves and 3,890 depth-tile halves move, each within one f16 step of its piecewise mix over 10 frames |
| C9, game | `MOVER_CLOSET_PRL MOVER_CLOSET_RAMP_PRL … c9_a_tick … --ignored`, both modes | **pass.** 173 ticks, peak t′ 0.5038, largest step 0.00586. Ramp 0.6 / 0.9: 259 ticks, peak t′ 0.5192 |
| D2, D4/D5 | capture target; `d4_d5_…` under `off` and `sync-proof` | **pass.** D2 deltas 0; D4/D5 0 against the state's own 58 / 60 |

**C10 capture frame row fails (open).** `c10_streaming_off_and_sync_proof_compose_the_same_channel_output` misses one of its five cases: setpiece station 3, baked direct (mask 8), both movers at t 0.5. The modes differ by 21 levels with id 57, past its heuristic bar of stripped 17 + 2.
- Pre-brief engine and PRL: 18. Parent: 18. New: 21.
- With `--uncompressed-irradiance` (parent and new): 1 and 1.
- So with equal bases this build composes the two modes alike, as the per-probe C10 gate shows. The gap is the pre-existing BC6H base difference between the modes (`drafts/sh-streaming-mode-bc6h-parity.md`). Mid poses change the mid-travel gated term at t 0.5, which weights that difference differently.
- Not loosened. The proposed fix is the owner's call:
  - judge this row against the pre-change build's own mode gap at the same pose; or
  - drop the mask-8 mid-travel case, since the per-probe test is C10's proof (probe-channel plan, Corrections);
  - the root fix is the BC6H parity follow-up.

### AC 19: em_sweeper t′ 0.30 and the roof top at t′ 0.4, 0.6 within 3 levels

`cargo test -p postretro-level-compiler --bin prl-build tests::mid_pose_compose:: -- --nocapture`: 2 passed.

| Group | Probes | With mid poses: bounce / direct | `light_poses 2`: bounce / direct |
|---|---|---|---|
| em_sweeper west face, t′ 0.30 | 20 | 1.07 / 0.00 | 19.54 / 58.57 |
| roof top, t′ 0.40 | 64 | 1.78 / 0.00 | 11.09 / 23.34 |
| roof top, t′ 0.60 | 64 | 1.80 / 0.00 | 14.51 / 39.70 |

### AC 28: low against high at rest, bound 12 levels

`tests/mover_probe_channel_fidelity_rest.rs` is now the gate. It fails past `LOW_AGAINST_HIGH_BOUND_LEVELS` = 12 (owner, M2), and still prints the largest difference either way, with its probe and position. It stays `#[ignore]`: it bakes both maps cold at both fidelities.

Command: `cargo test -p postretro-level-compiler --test mover_probe_channel_fidelity_rest -- --ignored --nocapture`. **Result: pass, 1 test, 66 s.**

| Map | Mover | Terminus | Probes | Low darker by | Low brighter by | Past 1 / 3 levels |
|---|---|---|---|---|---|---|
| mover-closet | `closet_door` | t′ 0 | 144 | 0.00 | **10.75** at (325.0, 523.8, 15.4) | 144 / 102 |
| mover-closet | `closet_door` | t′ 1 | 144 | 0.00 | 0.00 | 0 / 0 |
| mover-setpiece | `setpiece_wall` | t′ 0 | 10,377 | 9.09 | **11.42** at (413.9, 1206.3, 319.8) | 10,149 / 4,205 |
| mover-setpiece | `setpiece_wall` | t′ 1 | 10,377 | **11.23** at (610.7, 1048.8, 44.2) | 0.00 | 137 / 90 |
| mover-setpiece | `setpiece_room` | t′ 0 | 3,048 | **6.91** | 0.43 | 2,400 / 412 |
| mover-setpiece | `setpiece_room` | t′ 1 | 3,048 | **7.84** | 4.21 | 2,059 / 428 |

The figures equal task 9's measurement. The largest, 11.42, is 0.58 levels under the bound.

### AC 29: Build Summary

Unit tests in `pipeline/mover_fidelity_summary.rs`:
- `each_mover_prints_its_poses_its_shown_fidelity_and_its_projected_release_time`: termini plus mid-pose shares, a two-pose mover, `light_bounces 0`, approximate marking.
- `with_no_recorded_id_34_time_the_projection_reads_a_dash`.

Integration: `mover_probe_channel_fidelity` checks that the `--release` and the cache-filled `high` builds print the same share term, `(2 termini + 4 mid pose(s) × …)`. Sample and measurements are above.

## Gates

Every command ran under `nice` with `-j 6`. GPU runs set `POSTRETRO_REQUIRE_GPU=1`.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo clippy -p postretro --all-targets --features capture -- -D warnings` (capture targets touched) | clean |
| `-p postretro-level-format mover_probe_channel` | 37 passed |
| `-p postretro-level-loader mover` | 52 passed |
| `-p postretro-level-compiler --bin prl-build mover` | 162 passed, 4 ignored |
| `-p postretro-renderer mover` (GPU) | 122 passed, 7 ignored |
| renderer ignored mover gates on the fresh PRLs: AC 1, C10, C7, ledger, streamed placement | 5 passed |
| renderer C9 (`off`) | 1 passed |
| renderer `a_travelling_movers_live_tile_plan_cost_on_a_real_map` (measurement; needs `POSTRETRO_SH_STREAMING=off`) | setpiece: wall 414 tiles, 0.44 ms; room 843 tiles, 0.95 ms per frame (debug profile, CPU) |
| `mover_probe_channel_{bounce, cache, depth_cap, fidelity, placement, pose_memo, poses, setpiece}` | 6, 11, 2, 6, 1 (+1 ignored, run by its sibling), 4, 2, 2 passed |
| `mover_probe_channel_prl_parity -- --ignored` (baseline `base/prl-build-parent`) | 3 / 3 passed |
| `mover_probe_channel_fidelity_rest -- --ignored` (AC 28) | 1 passed |
| AC 7 / 8 references, `tests::mid_pose::the_ -- --ignored` | 2 passed |
| AC 19, `tests::mid_pose_compose::` | 2 passed |
| capture `capture_mover_probe_channel -- --ignored` | 3 passed (K1, K2, D2), **1 failed (C10 frames, above)** |
| capture `capture_mover_promotion_pop -- --ignored` | 2 passed (K3 static, animated) |
| engine `--bin postretro` `d4_d5 c9_a_tick -- --ignored`, `off` and `sync-proof` | 2 + 2 passed |
| `-p postretro-level-compiler --tests --no-fail-fast` | 2,823 passed, 0 failed, 71 ignored, 35 targets |

**A stale test from task 10, fixed here.** The first full-suite run failed `mover_light_gate_compile::a_three_waypoint_mover_is_posed_at_its_first_and_last_waypoints_only`.
- It asserted that no line for the three-waypoint mover `tour` names the corner light. Task 10's mid-pose-only line now does: the light is gated only at a mid pose, as the brief's Decision says ("a light any pose blocks is gated", reversing the foundation's three-waypoint row).
- Task 10's gates ran the compiler's `mover` filter, not the full suite, which is how the failure went unseen.
- The test is now `a_three_waypoint_movers_terminus_rows_pose_its_ends_and_a_mid_pose_gate_counts_apart`. It keeps the negative assertion on the per-terminus rows: no terminus row names the corner light or `tour_b`. It adds a positive one: the one mid-pose-only line names the corner light, and not the overhead one.
