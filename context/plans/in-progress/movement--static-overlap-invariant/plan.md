# movement--static-overlap-invariant — plan of record

mode: compact
status: active
read at: fd8982c

## Resolved block — Acceptance row B1 (owner restated)

B1 reads: "Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and
running across a coplanar seam keep the displacement recorded at b0a5c0f, within 1 mm
per tick."

At b0a5c0f the stair climb and the single step-up are not clear of static geometry, so
no pose that satisfies the invariant can match their recorded displacement on every tick.

Evidence: task 1 fixtures on unmodified source, canonical test capsule (r 0.4, half
height 0.8, step 0.3), walking +X at 7 m/s, 1 m treads.
- **Stairs at 0.3 m rise:** every step embeds the pawn in the step corner: 7.0 cm,
  3.4 cm, 5.6 cm on the three steps reached in 40 ticks.
- **Single 0.3 m step-up:** 7.0 cm on its approach tick.
- **Cause:** step-up fires while the riser is still up to 0.7 m ahead, because its probe
  distance is `max(v·dt, step + radius)`. The lifted pawn advances one tick's travel
  without reaching the riser. Ground-stick's swept down-cast then meets the step's top
  edge with a steep normal, and the centre-ray fallback drops the pawn 0.35 m straight
  into the corner. This is research bypass 2, reached by ordinary stairs.
- **Clean at b0a5c0f, max tolerance 0.5 mm:** the ramp, wall slide and seam run.

Prototype of the fix: a swept move cast through the skin dispatcher at a 0.5 mm target
distance, replacing the free advance and the ground-stick drop.
- **Ramp, wall slide, seam:** match b0a5c0f exactly, 0.0 mm per tick.
- **Stairs and step-up, horizontal:** within 0.25 mm per tick.
- **Stairs and step-up, end pose:** within 0.6 mm.
- **Stairs and step-up, vertical:** differs only on the embed ticks, by up to 13.6 cm,
  because the pawn stays on the step edge instead of sinking into it.

Owner decision: B1 takes the between-neighbours restatement, revised so step height
is never a literal. Step height is per-descriptor tuning, so the baseline fixtures build
stair rise and ledge height from `desc.ground.step_height`. The brief's B1 row now
carries the authorized wording. The 0.3 m and 7 cm figures above are evidence from the
canonical test descriptor only.

### Other task-1 findings (no restatement needed)
- **A1 lip fixture:** a 0.3 m chamfered lip at 25° embeds a pawn walking onto it by
  3.7 cm (free advance). At the ramp foot of a 2 m ramp the embed grows every tick, to
  22 cm. A bare 90° lip edge never leaves the skin band. Its tilted contact is only
  in-band, so the A1 fixture uses the chamfer.
- **Free advance on a steady ramp:** the advance stays inside the 20 mm skin band (about
  8 mm horizontal per tick on 20°). A skin-target sweep would cut it and lose about
  9.6 mm on a tick; a tangent sweep gains about 10 mm per tick. The 0.5 mm-target sweep
  keeps it exactly, so that is the conversion shape.
- **Bypass reproductions at b0a5c0f:**
  - Ground-stick fallback over a 0.35 m drop: 9.5 cm.
  - Unswept step-up rise under a ceiling 0.3 m over the standing head: 6.4 cm. The pawn
    also never climbs a step whose top has room for it (B2's case).
  - Floor push into a ceiling 19 mm over the resting head, step height 0: 1.1 mm.
  - Stuck-stop rollback at the default 1 mm threshold sinks at most 0.7 mm, all inside
    the skin band. The A7 fixture needs a larger descriptor `stuck_stop_threshold`, or
    gets named in its test with that reason.
- **Pre-existing, outside this brief:** stairs whose tread is shorter than
  `step_height + radius` (0.7 m for the canonical capsule) never step up at walk speed.
  The lifted forward probe hits the next riser, so the pawn creeps along the first riser
  edge at about 2 mm per tick.



No crate source changed between the brief's `read at` (b0a5c0f) and fd8982c, so the
grounded Decision reads stand. Source re-read this session: `movement/substrate.rs`,
`movement/mod.rs` (tick), `movement/teleport.rs`, `movement/dispatch.rs`,
`intents/{crouching,normal,slide}.rs`, `swim_transition.rs`, `collision/mod.rs`,
`collision/moving.rs`, `sim/src/agent/mod.rs`, `sim/src/agent_steering.rs`,
`sim/src/sim/host_movement.rs`, `netcode/src/prediction.rs`, `sim/src/sim/cpu_stages.rs`.

## Corrections
- Toolchain: the workspace requires rustc 1.99; the container shipped 1.97 → `rustup update stable` (1.99.1). No source impact.
- "Timing stage precedent: the `Environment` CPU stage" → that stage is fed by an
  `EnvironmentQueryTimer` attached to the `EnvironmentView`. The overlap check is not an
  environment query, so it gets its own accumulator on `CollisionWorld` (atomic, enabled
  per tick by `simulate_tick` only while timing is on) feeding a new `SimStage::OverlapCheck`
  under `Tick`, spanning `Movement` and `SteerAgents` exactly as `Environment` does.
- "the movement code returns an overlap report" → the player tick returns it on
  `MovementEvents` (a tick-local, never-serialized struct; no wire field), the agent
  controller on its `SlideResult`.

- The overlap measure and the check use the engine's exact capsule–triangle contact
  (`collision/capsule_triangle.rs`), not parry's `intersection_test`. On the fixtures'
  100 m triangles parry reported a capsule 0.515 mm clear as overlapping by 2.4 mm (its
  GJK drift beside large triangles, the reason the skin cast exists). Task-1 pre-fix
  reproductions were re-measured exactly against the task-2 source: all still exceed
  0.5 mm (0.74 mm to 7.7 cm), and the baseline's embed flags are unchanged.
- Swept moves stop `OVERLAP_TOLERANCE` (0.5 mm) short of a surface, so every in-band
  move b0a5c0f made stays exactly as it was; only surface-crossing moves are clipped.
  The step-up rise is the exception: it stops a full skin short, like a slide sweep,
  because a lifted capsule 0.5 mm under a ceiling meets the ceiling's coplanar seams on
  its forward probe (the skin cast's documented seam limitation) and refuses the step.
- B1 comparator: the tick after a b0a5c0f embed tick measures its displacement from the
  last tick that was clear at b0a5c0f, since the embedded pose no longer exists. Every
  clear tick still matches within 1 mm; this restates the reference pose, not the bound.
- A5 low step: the step's edge is a walkable contact, so the pawn rides it until the
  ceiling stops it, its feet still below the top. "Below the step" is asserted as feet
  below the top, centre short of the riser.

## Delegated answers
- The check's variable name — `POSTRETRO_OVERLAP_CHECK`: `1` turns the check on in any
  build, `0` turns it off in any build, unset keeps the build default (on with debug
  assertions, off in release). Matches the `POSTRETRO_*_TIMING` naming of the other
  diagnostic switches; the `0` value exists so the measured AC can compare set/unset in
  one build.

## Pre-fix fixture results (A2)

Run on unmodified movement source (task 1 commit). Every row below is committed as
`#[should_panic]` naming its bug; the fix flips it to a plain test. Depths are the
exact capsule–triangle measure the shipped check uses, applied to the
behaviour-preserving source at 938a7d0 (task 2, which moves no body differently from
b0a5c0f); the first recording used parry's intersection depth, which drifts by
millimetres beside large triangles. D3 and D4's pre-fix depths were measured on
post-task-4 source with task 5 removed, since task 1 predates their fixtures.

| Fixture | Bypass | Worst overlap at b0a5c0f |
|---|---|---|
| `a_pawn_walking_onto_a_chamfered_lip_ends_every_tick_clear` (A1) | free advance | 1.2 mm |
| `free_advance_then_ground_stick_at_a_ramp_foot_ends_clear` (A8, P5) | free advance, then ground-stick | grows every tick; 5.1 mm on the first |
| `a_floor_push_under_a_ceiling_within_a_skin_ends_clear` (A4) | floor push, step height 0 | 1.1 mm |
| `a_step_without_room_for_the_standing_capsule_leaves_the_pawn_below_it_and_clear` (A5) | step-up rise | 7.3 mm |
| `a_step_up_then_a_floor_push_under_an_overhang_ends_clear` (A9, P4) | step-up approach (centre-ray drop at the riser) | 1.5 mm |
| `ground_stick_beside_a_riser_over_a_drop_ends_clear` (A6) | centre-ray drop | 6.75 cm |
| `a_still_pawn_over_a_ledge_edge_ends_clear_after_ground_stick` (A10, P7) | centre-ray drop | 7.69 cm |
| `a_corner_wedge_on_the_steepest_walkable_ramp_ends_clear_after_the_rollback` (A7) | stuck-stop rollback | 1.0 mm, with `stuck_stop_threshold` 0.1 m (at the default 1 mm it cannot leave the skin band, noted in the test) |
| `a_step_with_room_for_the_capsule_but_not_the_lift_margin_still_steps_up` (B2) | step-up headroom | 0.74 mm, and never climbs |
| `a_stair_climb_keeps_its_recorded_displacement` (B1) | centre-ray drop | 6.9 cm |
| `a_step_up_keeps_its_recorded_displacement` (B1) | centre-ray drop | 7.0 cm |
| agent `an_agent_walking_into_a_step_without_headroom_stays_below_it_and_clear` (A5) | agent step-up rise | reproduces |
| agent `agent_ground_stick_beside_a_riser_over_a_drop_ends_clear` (A6) | agent centre-ray drop | reproduces |
| agent `an_idle_agent_settling_over_a_ledge_edge_ends_clear` (A11, P6) | agent centre-ray drop, idle call site | reproduces |
| agent `an_agent_still_steps_up_with_room_for_its_capsule_but_not_the_lift_margin` (B2) | agent step-up headroom | clear, but never climbs |

Clean at b0a5c0f, as expected: the ramp, wall-slide and seam baselines, and every vault
fixture (`vaults_never_end_a_tick_overlapping_static_geometry`: flat lip and a lip
chamfered at exactly `max_slope`; on the path, aborted, oblique, held and neutral after
exit). A sloped vault top is refused by the probe at any tilt that tall (the capsule
over it needs `rest / cos` of room), so A3's walkable-limit case is a chamfered lip.

## AC-to-proof

Fixtures live in `crates/physics/src/movement/overlap_tests.rs` (pawn), `overlap_baseline_tests.rs`
(B1), `overlap_check.rs` (A14, gate resolver), `teleport_tests.rs` (B4),
`crates/sim/src/agent/overlap_tests.rs` (agents, A15, D1/D5 agent side) and
`crates/netcode/src/prediction.rs` (C1). Every movement and agent test runs with the check
armed as an assertion.

| AC | Proof | Status |
|---|---|---|
| A1 lip-edge reproduces first | `a_pawn_walking_onto_a_chamfered_lip_ends_every_tick_clear` (committed `should_panic` in 92537d4, plain since f87df31) | automated |
| A2 per-bypass fixtures overlap at b0a5c0f | table above | automated (recorded) |
| A3 vault fixtures | `vaults_never_end_a_tick_overlapping_static_geometry` | automated |
| A4 floor push / ground-stick under ceiling (P7) | `a_floor_push_under_a_ceiling_within_a_skin_ends_clear` (step 0 and positive) | automated |
| A5 step with too little headroom, pawn and agent | `a_step_without_room_for_the_standing_capsule_leaves_the_pawn_below_it_and_clear`, agent `an_agent_walking_into_a_step_without_headroom_stays_below_it_and_clear` | automated |
| A6 ground-stick beside wall, pawn and agent | `ground_stick_beside_a_riser_over_a_drop_ends_clear`, agent `agent_ground_stick_beside_a_riser_over_a_drop_ends_clear` | automated |
| A7 corner wedge stuck-stop rollback | `a_corner_wedge_on_the_steepest_walkable_ramp_ends_clear_after_the_rollback` | automated |
| A8 two bypasses one tick (P5) | `free_advance_then_ground_stick_at_a_ramp_foot_ends_clear` | automated |
| A9 step-up then floor push under overhang (P4) | `a_step_up_then_a_floor_push_under_an_overhang_ends_clear` | automated |
| A10 still pawn over ledge edge (P7) | `a_still_pawn_over_a_ledge_edge_ends_clear_after_ground_stick` | automated |
| A11 idle agent over ledge edge (P6) | `an_idle_agent_settling_over_a_ledge_edge_ends_clear` | automated |
| A12 rider walks into lip (P2) | `a_rider_walking_into_a_chamfered_lip_while_its_platform_moves_ends_clear` | automated |
| A13 mover push toward wall (P1) | `a_pawn_pushed_out_of_a_mover_toward_a_static_wall_does_not_trip_the_assertion` | automated |
| A14 check resumes after mover embed (P8) | `after_a_mover_caused_embed_the_first_tick_that_starts_clear_is_checked_again`, `an_armed_check_panics_on_a_breach` | automated |
| A15 warning text, once per load, reload re-arms | `an_overlap_warns_once_per_level_load_naming_descriptor_texture_and_position` | automated |
| B1 displacement baseline; embed ticks bounded between neighbours; rise from descriptor step height | `a_ramp_walk_…`, `a_stair_climb_…`, `a_step_up_…`, `a_wall_slide_…`, `a_run_across_coplanar_seams_keeps_its_recorded_displacement` | automated; ramp, wall slide and seam match exactly, so no seam slide loses free-advance distance |
| B2 headroom for capsule not lift margin still steps up | `a_step_with_room_for_the_capsule_but_not_the_lift_margin_still_steps_up`, agent `an_agent_still_steps_up_with_room_for_its_capsule_but_not_the_lift_margin` | automated |
| B3 step height zero | `with_step_height_zero_a_pawn_neither_steps_up_nor_snaps_to_the_floor`, agent `with_step_height_zero_an_agent_neither_steps_up_nor_snaps_to_the_floor` | automated |
| B4 teleport arrival beside wall lands grounded | `teleport_classic_arrival_beside_a_wall_still_lands_grounded` | automated |
| B5 pawn steps onto mover top; agent ignores it | `a_pawn_steps_up_onto_and_snaps_to_a_movers_top`, `an_agent_ignores_a_mover_top_its_pawn_neighbour_steps_onto` | automated |
| B6 controllers keep call rules; own walkable slope | `a_falling_agent_beside_a_step_still_steps_up`, `an_airborne_pawn_beside_a_step_does_not_step_up`, `a_pawn_tuned_to_a_shallower_walkable_slope_classifies_floors_by_its_own_slope` | automated |
| B7 carried into ceiling behaves as before; no assert | `a_pawn_carried_into_a_static_ceiling_rides_on_and_never_trips_the_assertion` (carry code is untouched; the fixture proves the ride and the silent assertion) | automated |
| B8 existing suites pass armed | `cargo test` physics 396, sim 1477, netcode 800, ai 245, postretro 1812 + 3, all armed | automated |
| C1 predictor replay reproduces host poses exactly | `predictor_replay_reproduces_host_poses_exactly_over_a_vault_step_up_and_ground_stick` | automated |
| D1 grep gate: no unswept position writes | `the_substrate_writes_no_body_position_outside_a_swept_move`, `the_agent_controller_writes_no_body_position_outside_a_swept_move` | automated |
| D2 stand-up / slide exit under low ceiling | `a_stand_up_under_a_ceiling_too_low_…`, `a_slide_exit_under_a_ceiling_too_low_…` | automated |
| D3 airborne stand-up above floor (P11) | `an_airborne_stand_up_just_above_a_floor_stays_crouched_until_the_feet_have_room` (pre-fix: 0.4 m in the floor) | automated |
| D4 larger crouch capsule entry under low ceiling | `a_crouch_entry_larger_than_the_standing_capsule_under_a_low_ceiling_stays_standing_and_clear` (pre-fix: 0.3 m in the ceiling) | automated |
| D5 grep gate: agent holds no step-up/snap logic | `the_agent_controller_holds_no_step_up_or_snap_to_floor_of_its_own` | automated |
| D6 default off in release, on with debug assertions, variable on in release | `the_check_defaults_on_with_debug_assertions_and_off_in_release`, `the_variable_turns_the_check_on_or_off_in_any_build` | automated |
| D7 diff gate: no wire / descriptor field | `git diff fd8982c -- crates/net crates/foundation crates/entities crates/netcode/src/wire*` is empty | checked |
| D8 measured cost on `stress-env-volumes` | `measurements/overlap-check-cost/` — `sim_overlap_check` 0.024 ms per tick (0.040 ms over the first 600 ticks) with the check on; absent with it off; `sim_tick` unchanged; dumps byte-identical. Under 0.1 ms, so every moving body is checked | measured headless (no GPU here: the tick's stage timing turned on by a local patch, recorded with the result); a windowed release run is the owner's |
| M1–M4 | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Riskiest premise first: test-only overlap helper, the lip-edge fixture, the displacement baseline and every per-bypass fixture, run on pre-fix source; commit baseline + lip expected-failure | integrating executor | — | done: fixtures + baseline committed pre-fix |
| 2 | Behaviour-preserving: swept-move helper and shared step-up / snap-to-floor primitives in a sibling module; player substrate and agent controller call them; agent copies deleted | integrating executor | 1 | done: `movement::swept`; physics 374 + sim 1471 tests unchanged, fixtures still reproduce |
| 3 | Overlap check: tolerance query, armed assertion, build/env gate, report with texture + TrenchBroom position, warn-once on `CollisionWorld`, caller logging (host, prediction, agents), timing stage | integrating executor | 2 | done: `collision::overlap`, `movement::overlap_check`, `sim::report_static_overlap`, `SimStage::OverlapCheck` |
| 4 | Convert every unswept move at its source; flip lip fixture | integrating executor | 3 | done: every task-1 fixture flipped; physics 376, sim 1471, netcode 799, ai 245 pass armed |
| 5 | Resize growth clearance: anchored stand-up, growing entry | integrating executor | 3 | done: crouch stand-up and crouch-jump sweep in the anchor's growth direction; crouch and slide entry test a growing crouch capsule. Without it D3 ends 0.4 m in the floor, D4 0.3 m in the ceiling |
| 6 | Remaining AC fixtures, grep gates, determinism replay | integrating executor | 4, 5 | done |
| 7 | Measurement on `stress-env-volumes` | integrating executor | 3 | done: `measurements/overlap-check-cost/` |
| 8 | Preflight, review panel, fixes | integrating executor | 6, 7 | done: two review rounds; fixes in 63b19aa and after |
