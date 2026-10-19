# movement--state-view-pose — plan of record

mode: compact
status: active
read at: 8aa2f18

## Corrections
- Path: "correct `movement.md` §1's claim that hot reload releases the vault pose" → current `movement.md` §1 already says a refresh, hot reload included, clears slide and vault with no release. Only the `view_feel::vault::evaluate` doc comment is stale; planning around it by replacing that module, so the stale comment goes with it.
- Since `3409c2fc`, `frame_loop` gained the teleport camera turn (`runtime_teleporters::apply_local_camera_turn`) and `run_client_predicted_tick`. The Eye stage, `presented_vault`, `ViewFeelDriver` and `assemble_frame_eye` are unchanged; `inputs.pitch` already carries the frame's aim pitch after look, so the driver needs only the landing spot added.
- Path: "read the mover pose at the eye's own interpolation share" → no Eye-stage helper exists. `EntityRegistry::interpolated_transform(mover_entity, alpha)` is the presented pose the audio anchors read in the same frame (`sound_events::anchors`); planning around it by resolving the vault's mover id to its entity with a `KinematicMover` scan (bounded by mover count, and only on frames that present a mover vault) and reading that pose at `frame_result.alpha`.
- Brief: "`frame_dt = 0` leaves every row's output unchanged … a vault's first frame". The shipped vault evaluator holds on a zero-length frame except where weight is a pure function of progress (a first frame with `easeIn` 0). Clarification, same meaning: a zero-length frame holds each row's weight and values while still marking a join and taking the reference. This matches the shipped evaluator on every migration case listed (the zero-length frame there is a vault's end).
- Brief Scripting surface: `fov` is [-90, 90] on every row, widening slide's shipped [0, 90]. Applied as written.
- AC P20 says a larger eyeDrop "steepens the line"; geometrically a lower eye sits closer to a landing below it, so its line is shallower (and a tall ledge reads as above a dropped eye under it). Clarification, same meaning — the line is measured from the dropped eye, observable the same frame; tested in that direction.
- AC P10 "no step beyond the new vault's own ease-in": with differing value sets the overlap moves at the pace of A's release (0.1 s minimum, shipped) plus B's ease-in. Clarification, same meaning — tested as no frame moving faster than that release and ease-in together, and no `w_A·|v_A − v_B|` jump on B's first frame (`back_to_back_vaults_at_different_ledges_and_a_rewind_never_step`).
- Graded-ledge AC with the brief's example `ledgeFocus { 0.7, 30 }`: every graded ledge saturates at −30°. Peak focus is set by the climb's end, where the eye stands over the landing spot and the floor-bounded line (~−72°) is nearly the same for every ledge; uncapped peaks differ by 1–5°. The dev player authors `{ strength: 0.55, maxPitch: 45 }`: peaks ≈ −38.1, −37.5, −36.7, −39.3° (0.6/1.0/1.5/2.2 m), beside the shipped −38.5° hold. The per-ledge difference is mostly timing (a tall ledge dips late), which the manual check covers. Reported to the owner.
- The graded-ledge proof models a running jump into the riser (first jump distance, in 0.1 m steps, that vaults): a standing jump against the riser clears the 0.6 m ledge, whose top is under the jump's 1.16 m peak.

## Delegated answers
- Horizontal-distance floor for the focus angle — 0.5 m (`LEDGE_FOCUS_MIN_HORIZONTAL_M`). It bounds the line at atan(eye height / 0.5) (−71.6° at the dev 1.5 m eye) and stops the line whipping toward −90° as the eye passes over the landing spot, in the climb's last share where the row is already easing out. Revisited against the graded-ledge proof below.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Surface: TS and Luau accept the example, reject the listed cases naming the path, leftover `slide`/`vault` name their `pose` replacement, omission yields no pose | `scripting-core` `tests::movement_view_pose::*` (JS/Luau parity), `sdk/type-tests/view-pose.ts` via `tsc -p sdk/type-tests` | achievable as stated |
| Migration: `pose.slide` reproduces shipped slide frame for frame, floor included | `view_feel::pose` migration test against `view_feel::slide::evaluate` in one build, then removal | achievable as stated |
| Migration: `pose.vault` reproduces shipped vault pose over the listed cases | `view_feel::pose` migration test against `view_feel::vault::evaluate` in one build, then removal | achievable as stated |
| Rate row holds for the state; every exit eases monotonically, a no-edge `Normal` included | `view_feel::pose` unit test | achievable as stated |
| P1 slide exit then vault: both ease, rows add, no step | `view_feel::pose` unit test | achievable as stated |
| P2 summed eye drop floored at 0.05 m | `frame_eye` test | achievable as stated |
| P17 frame-rate independence 30/60/240; zero-length frame holds output, end and first frame | `view_feel::pose` unit test | achievable as stated |
| Vault row neutral on the tick the path reaches the top at 30/60/144/240 Hz, easing a tick before | `view_feel::pose` unit test (carried from vault) | achievable as stated |
| Early ends release from last presented pose, focus included, over max(path × easeOut, 0.1 s), monotonic | `view_feel::pose` unit test | achievable as stated |
| P3 vault ending during a release releases the blended pose | `view_feel::pose` unit test | achievable as stated |
| P10 back-to-back vaults at different ledges and a rewind: no step beyond ease-in | `view_feel::pose` unit test | achievable as stated |
| Focus predicates both sides, never upward, capped, finite on the final tick | `view_feel::pose` unit test | achievable as stated |
| Literal pitch at or below the line adds no focus; above, reference + offset approaches and never passes | `view_feel::pose` unit test | achievable as stated |
| P5/P14 mouse 1:1 from the frame after capture, through climb and release; new vault or rewind takes a new reference | `view_feel::pose` unit test | achievable as stated |
| P6 full climb starts and ends neutral; a join eases focus in | `view_feel::pose` unit test | achievable as stated |
| Frame from a vaulting pawn's live movement state shows its own landing focus; two heights differ | `frame_eye` test driving `physics::movement` ticks into the driver builder | achievable as stated |
| P15 reference is the capture frame's rendered aim | `view_feel::pose` unit test | achievable as stated |
| P16 scale 0 at start, restored mid-climb, uses the entry reference | `view_feel::pose` unit test | achievable as stated |
| P14/P18 join after reconcile or reset takes the join frame's aim | `view_feel::pose` unit test | achievable as stated |
| P21 abort + new vault in one frame under 5% keeps the reference | `view_feel::pose` unit test | achievable as stated |
| Focus never turns yaw; same offset at any facing or bearing | `frame_eye` test | achievable as stated |
| P20 line measured from the dropped eye; more eyeDrop steepens it the same frame | `frame_eye` test | achievable as stated |
| Dev `pose.vault` gives ≥3 distinct peak focus angles over 0.6/1.0/1.5/2.2 m ledges | `startup::movement_vault_surface_tests` driving real vaults through the dev manifest | achievable as stated |
| P8/P12/P19 mover vault matches static at same relative geometry, follows tilt at the frame's share, reference does not turn | `frame_eye` / driver-builder test with a registry mover | achievable as stated |
| Pitch clamp ±89° and FOV band with every row + impulse | `frame_eye` test | achievable as stated |
| Aim, host aim and shot direction bit-identical over a slide and an `allowFire` vault, pose present vs absent | `frame_eye` test over the driver builder + `build_post_movement_command` | achievable as stated |
| Viewmodel transform excludes row pitch/roll; eye drop moves eye, listener and viewmodel together | `frame_eye` test | achievable as stated |
| P4 scale 1/0.5/0, restore mid-hold, scale 0 through end restored mid-release | `view_feel::pose` unit test | achievable as stated |
| P9 refresh / hot reload snap; pawn change, no driver, level install reset | `frame_eye` + `startup::staged_manifest_lifecycle` tests | achievable as stated |
| Remote pawn vaulting or sliding adds no pose locally | driver-builder test with local and remote pawns | achievable as stated |
| Diff gate: `WIRE_VERSION`, `SNAPSHOT_VERSION`, wire fixtures unchanged; committed typedefs match | `git diff main -- <paths>` empty; `sim` `typedef::tests::committed` | achievable as stated |
| `player.ts` authors `pose` and loads; grep gate; reference docs cover `pose` and the `allowFire` reticle offset | `startup::movement_vault_surface_tests` + `rg` gate | achievable as stated |
| Manual: graded ledges settle toward the ledge, hold, level out, no pop; tall ledge dips late; pool exit never pulls up | owner, in-engine, `vault-test` | manual |
| Manual: mouse look stays responsive and natural through a climb | owner, in-engine | manual |
| Manual: reduced motion shows no pose; slide and vault feel unchanged with focus off | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Pose types in foundation beside the shipped blocks; `view_feel::pose` vault row with ledge focus, release value blend and reference capture; driver landing spot (mover at the eye's share); `frame_eye` composition. Migration tests against the shipped slide and vault evaluators in one build | integrating executor | — | done — `frame_eye::pose_tests::pose_{slide,vault}_reproduces_*` pass beside the shipped evaluators in this milestone's build |
| 2 | Strict `pose` parsing (JS + Luau shared validator), leftover-key rejection, typedef registry, regenerated SDK typedefs, TS type test | worker (scripting surface) | 1 (types) | done — `movement_view_pose` 14 tests, scripting-core 896 + 2, sim typedef 44, `tsc -p sdk/type-tests` 0 errors |
| 3 | Remove shipped `slide`/`vault` blocks, evaluators, parsers and their types; dev `player.ts`; docs; vault-test README; grep gate | integrating executor | 1, 2 | done — migration proof ran in `dcf91cf` before removal; grep gate clean; `tsc` 0 errors |
| 4 | Remaining runtime ACs: graded-ledge dev proof, mover, aim bit-identity, remote pawn, lifecycle | integrating executor | 3 | done — `frame_eye::pose_tests::driver::*`, `startup::movement_vault_surface_tests::the_dev_players_ledge_focus_differs_across_the_graded_ledges` |
| 5 | Preflight, review panel, fix loop, results | integrating executor | 4 | |
