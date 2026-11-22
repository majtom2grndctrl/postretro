# E17--mover-relative-rider-presentation — plan of record

mode: resumable
status: approved
read at: be041cf8

## Owner rulings

- **Rows 24/29 (was blocked).** Option A. A connected client's pawn mesh (shadow and mirror) always carries the same world-space presentation offset as its eye, so the mesh and eye agree on every frame and a hand-off never steps the mesh. Brief updated: the local-eye Decision's mesh clause, and row 24 now reads "…draw an eye bit-equal to before. Their pawn mesh is bit-equal to before, except that a connected client's mesh now carries the same world-space presentation offset as its eye while one glides." Source fact behind it: `present_local_pose` has no production caller, so today the client mesh carries no offset.
- **Plan approved** by the owner with that ruling.

## Corrections

- **Path: mover history.** The brief cites `MoverHistoryBuffer` and `client/replay_poses.rs` for "the newest authoritative mover sample at or before" a sample's tick. Current source:
  - `MoverHistorySample` has no authoritative/predicted flag.
  - Each apply pushes the authoritative sample plus one predicted sample at the fast-forward tick, and `record` replaces only an equal back tick. The deque is therefore not tick-sorted (T, T+5p, T+2, T+7p…).
  - `resolve_replay_tick` prefers an exact tick, which can be a stale predicted sample. Its per-tick read is a `&mut` memo; the plain lookup `pose_at_tick` is `#[cfg(test)]`.
  - Planning around it: tag history samples as authoritative or predicted at `prepare_mover_apply`, which already knows which is which. Add a read-only `authoritative_pose_at_or_before(mover, tick)` for presentation, separate from replay. Replay behaviour is unchanged.
  - P8 becomes: no authoritative sample at or before the tick → missing.
- **Path: sparse samples.** P9 says sparse fields are "filled at record time". Only `teleport` and the movement-only sample's `transform` are. Velocity, stance and capsule resolve at sample time through `movement_field_at_or_before`, which never crosses a teleport generation. Ground is stored as an `Option` on each sample and resolved the same way, so a late ground sample corrects later sparse samples by construction.
- **Path: remote sample shapes.** `EntityBuffer::sample_at` also returns held-oldest, held-newest (starved) and extrapolated poses, not only bracketed intervals. See *Delegated answers*.
- **Path: ground on the client.** `wire_ground_ref_to_foundation_checked` lives in `movement_state.rs` and serves only the local pawn's merge. Remote movement payloads are never merged, and the grounded bit comes from `wire_locomotion_stance`. Remote ground is read from the wire record at apply, next to `unknown_ground_mover_id`, into the interpolation sample.
- **Path: mover corrections.**
  - `MoverCorrection` already carries the full `authoritative_state`, but not the pre-correction pose. The plan captures the pre-correction *presented* pose at apply, which the bounds snap (P7) and seeding both need.
  - `client_receive_and_apply` dedupes corrections per mover per frame, last one wins. Smoothing composes by seeding from the pose drawn the frame before, which is what "two corrections on one frame compose" means.
- **Path: foot probes.**
  - Foot probing already left `sim/mod.rs`. It lives in `crates/sim/src/sim/pose_inputs.rs` (`update_foot_ground_probes`, `_for`, `write_foot_ground_probes`), called from `tick.rs` in `SimStage::SteerPose`. That split is done, so the planned split is dropped.
  - Per-tick probes read the rider's current `Transform`, not the alpha pose. Moving them per frame changes the rider-pose source as well as the mover-pose source, which the planted-foot row requires.
- **Path: `light_bridge.rs`** now lives at `crates/postretro/src/scripting/frame_systems/light_bridge.rs` (`follow_transform_position`). It still reads the accessor.
- **Path: unlisted accessor consumers.** `frame_eye.rs` `presented_landing` (vault landing on a mover) and `ghost_links/voices.rs` read `interpolated_transform`, so they follow smoothing by construction. `presented_landing` joins the agreement test (row 31).
- **Path: cull.** `KinematicMoverRenderCollector::collect_with_mirror` culls on the current-tick bounds. Its SH regions are already the union of the current and interpolated AABBs, so they cover the presented pose once the correction lives in the accessor. The cull gains the presented AABB.
- **Path: sound anchors.** `sound_events/anchors.rs` `fire_time_point` reads the current `Transform`; door sounds use it too (`sound_events/movers.rs`). It moves to the presented pose for a mover carrying a correction. `presented_point` already reads the accessor.
- **Decision: hot paths.** It says the cost is "measured in the per-frame Presentation CPU stage". Today:
  - client remote placement and client foot probes run in `FrameStage::Presentation`;
  - the host seat's placement and probes run in `HostSend`, after serialization;
  - single-player and host probes run in `SimStage::SteerPose`;
  - accessor readers run in `RenderPrep` and `Audio`.

  Planning around it: every new per-frame pass runs in Presentation. That covers the smoothing advance, the client rebase, all foot probes, and the host seat's rebase and probes. The host seat computes its poses and feet into reusable scratch there, and only writes them after serialization in HostSend. The accessor's added term is one sparse lookup per read in the stages that already read it. A sparse-hit counter reports it, since it has no stage of its own. Same meaning: hot-path cost is bounded and measured per frame. The owner may still overrule.
- **Path: eye steps not listed.** On a client, `life_state_eye.render` rewrites `presented_aim.position` after `presented_aim_pose` and before `assemble_frame_eye`. The attached-eye placement runs before it, so the posture ease composes on the placed eye along world-up.
- **Path: splits.** `crates/netcode/src/client.rs` remote presentation spans about lines 121–386 (types) and 2486–3205 (methods); mover history and apply span about 540–625 and 3263–3377. Both still need splitting. `main.rs` has not been split: the yaw-carry block is about lines 1449–1646 and the eye block about 1996–2040. They move into `frame_eye.rs`'s neighbourhood.

## Delegated answers

- **Smoothing time constant.** Exponential approach with τ = 25 ms, as `1 − exp(−dt/τ)` per frame so the rate does not depend on frame rate. The pawn offset's glide is 0.7 per 16.7 ms tick, τ ≈ 47 ms, so this is faster. Task 6 reports it with the measured steps, and the settle epsilon matches `OFFSET_SETTLED_EPSILON_M` (1e-4 m).
- **Held and extrapolated remote samples** (clarification within "Transitions fall back to world space"). A held or extrapolated pose is a degenerate interval whose two ends are the newest (or oldest) sample. It attaches when that sample names a loaded mover with an authoritative pose for its tick, so a starved rider holds its deck point instead of detaching or extrapolating carry as world velocity. Otherwise it keeps today's world-space hold or extrapolation.
- **Order of the first tasks.** The brief's Path says to split first; the skill says to test the riskiest premise first. Task 1 is the thin client rebase slice, built in new modules (`client/rider_rebase.rs`, plus fields added at `interpolation.rs`'s boundary) with only a hook line in `client.rs`. The behaviour-preserving splits follow as task 2, before any further growth of `client.rs` or `main.rs`.

## AC-to-proof

Row numbers follow the brief's Acceptance list in order.

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Ten revolutions planted, `1e-4`, client view and host seat (P11) | `remote_rider_stays_planted_on_spinning_travelling_mover_client_view`, `..._host_seat` on a new spin+travel fixture with a second rider, mandated profile | achievable as stated |
| 2 | Spin start, stop, reversal mid-interval | same fixture, scripted spin changes | achievable as stated |
| 3 | `carry_yaw` yaw rule; deterministic; pitch/roll no effect | rebase unit tests | achievable as stated |
| 4 | Same mover attaches; differing ends draw today's world pose | rebase unit tests, both sides | achievable as stated |
| 5 | Sparse deltas: Transform-only and movement-only attach | interpolation buffer tests | achievable as stated |
| 6 | Late ground sample decides later samples (P9) | interpolation buffer test | achievable as stated |
| 7 | Fallbacks: missing ground or history, P8, unloaded, non-finite, zero-scale | rebase unit tests | achievable as stated |
| 8 | Teleport cut; first attach after the cut | interpolation and rebase test | achievable as stated |
| 9 | Planted foot within `1e-3`: remote on both seats, local pawn and NPC on SP/host, vertical mover | foot-probe tests against the presented pose source | achievable as stated |
| 10 | Correction frame ≤1e-4 m and rad; same convergence time at 30/60/240 Hz | registry smoothing tests plus frame-loop harness | achievable as stated |
| 11 | Zero-tick correction never steps (P1) | frame harness | achievable as stated |
| 12 | One pose for every reader on the correction frame (P2) | agreement test | achievable as stated |
| 13 | Before/after worst same-frame step recorded | measurement test, figures in this plan | achievable as stated |
| 14 | Compose; bounds snap incl. rotation-only; whole snap (P18); clears | smoothing tests | achievable as stated |
| 15 | Client eye on corrected mover within `1e-4` (P3); `carry_yaw` yaw; mispredict glides world (P17) | client eye harness | achievable as stated |
| 16 | `carry_yaw` zero-tick yaw (P13) | client eye harness | achievable as stated |
| 17 | Eye ≤ own motion on correction frame (P14) | client eye harness | achievable as stated |
| 18 | Jump off or step to another mover mid-glide (P15) | hand-off tests | achievable as stated |
| 19 | One snapshot corrects mover and moves ground (P5, P15) | hand-off tests | achievable as stated |
| 20 | Land on a mover mid-glide (P4, P15) | hand-off tests | achievable as stated |
| 21 | Teleport off mid-glide cuts clean (P6) | hand-off tests | achievable as stated |
| 22 | Rotation-only snap cuts the eye (P7) | hand-off tests | achievable as stated |
| 23 | Despawn or rebind mid-glide (P12, P16) | hand-off tests | achievable as stated |
| 24 | Static ground or airborne: eye bit-equal; mesh bit-equal except a client mesh carries the eye's world offset (restated) | eye and mesh equality tests | achievable as restated |
| 25 | Quarter turn: arc midpoint; constant radius | local eye tests on SP, host and client | achievable as stated |
| 26 | Pitched or rolled mover: eye height along world-up | local eye test | achievable as stated |
| 27 | Transitions, holds, cuts and missing pawn draw today's blend (P19) | local eye tests | achievable as stated |
| 28 | Teleport-class correction on a gliding mover moves the eye once (P19) | local eye test | achievable as stated |
| 29 | Client mesh ≤ own motion on hand-off frames | mesh hand-off test | achievable as stated |
| 30 | `carry_yaw` hand-off yaw; command yaw unchanged (P20) | hand-off tests | achievable as stated |
| 31 | Every consumer agrees with the presented pose under smoothing | agreement test: beauty, shadow, occluder, light, ghost, sound, rebase, probe, eye, mesh, `presented_landing` | achievable as stated |
| 32 | Presented pose in view draws; SH and fired sound follow | collector and anchor tests | achievable as stated |
| 33 | Authority bit-equal with features disabled; `WIRE_VERSION` unchanged | A/B harness run with a presentation-off switch | achievable as stated |
| 34 | `carry_yaw` legs relative to aim equal; off → equal outright | remote locomotion test | achievable as stated |
| 35 | Non-riders, static, no-ground and mover-less maps bit-equal | equality tests | achievable as stated |
| 36 | Every remote riding, several movers: bounds, zero allocation, Presentation stage cost before/after | stress fixture plus `alloc_probe` (installed in netcode's tests), cost figures here | achievable as stated (see Corrections, hot paths) |
| M1 | Co-op carousel under the lag proxy; README recipe | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Thin slice: client-view remote rebase.** Tag history samples authoritative or predicted and add the at-or-before lookup (P8). Add ground to interpolation samples, resolved at sample time (P9). Add the rebase in the mover frame between bracketing samples, placed through `interpolated_transform`. Add a harness fixture: a spin+travel mover and a second player riding it, mandated profile. Measure the ten-revolution bound before and after. Rows 1 (client half), 4–8. | integrating executor | — || done: `remote_rider_stays_planted_on_spinning_travelling_mover_client_view` — worst drawn offset over ten revolutions 0.6273 m world-space (before) → 0.000006 m rebased (host carry drift 0.000006 m); `rider_rebase::tests` (6), `interpolation::rider_tests` (6), `client::rider_history_tests` (2); netcode lib 1125 pass; `postretro --tests --features dev-tools` checks clean |
| 2 | **Behaviour-preserving splits, one commit each.** Move remote presentation and mover history/apply out of `client.rs` into `client/` modules. Move the camera-follow, eye and yaw-carry helpers out of `main.rs`. | integrating executor | 1 | done: `client/remote_presentation.rs` (presentation methods) and `client/mover_history.rs` (history, apply plan, tick helpers) out of `client.rs`, production part now ~2.6k lines; `local_eye.rs` (yaw carry, aim pose, eye follow, mover correction at the eye) out of `main.rs`. netcode lib 1125 pass; 28 moved-helper tests in `postretro` pass; both configurations check clean |
| 3 | **Host-seat rebase.** Record ground and the end-of-tick mover pose with each sample at the same seam and label (P11). Compute rebased poses and feet in Presentation scratch; write them after serialization. Row 1 (host half), row 2. | integrating executor | 2 || done: host records ground and the ridden mover's end-of-tick pose under the sample's tick label (`RiderMoverPoses`, P11) and rebases in `present_client_pawns`. `..._host_seat`: 0.0465 m before → 0.000006 m after over ten revolutions; `remote_rider_stays_planted_through_spin_stop_reversal_and_restart` (row 2): client 0.000017 m, host 0.000019 m; netcode lib 1131 pass. Moving the host-seat compute into the Presentation stage moves to task 7 with its foot probes |
| 4 | **Remote `carry_yaw` and legs.** Carry yaw relative to the mover's world-up yaw. Turn intrinsic velocity by the rebase yaw before the pose pass. Rows 3, 34. | integrating executor | 3 || done: rebase carries yaw relative to the mover's world-up yaw (the local eye's twist rule, now `kinematic_mover::world_up_yaw`), keeps pitch/roll, reports `yaw_turn`; both seats turn intrinsic velocity by it. `rider_rebase::tests::carry_yaw` (4: relative blend + drawn yaw once, determinism, pure pitch or roll never enters, legs relative to aim); `host_presentation::rider_tests` (row 34 at the avatar seam: carry on keeps legs relative to aim, carry off equal outright); netcode lib 1133 pass; `postretro` checks clean |
| 5 | **Registry render-only correction** (`entities`). Sparse per-entity correction applied after the blend in `interpolated_transform`. Advance once per frame at frame-rate-independent τ. Clear on despawn, unload, rebind and non-finite input. No allocation in steady state. Rows 10, 14 (registry part). | integrating executor | 1 || done: `PoseCorrection` (rigid, about the origin) + registry `correct_transform` (shifts both blend endpoints; glide composes the inverse; snap clears), fresh-then-decay `advance_presentation_corrections`, cleared on despawn and non-finite input; netcode `advance_mover_correction_glides` (τ 25 ms, `exp(−dt/τ)`), called at the head of the Presentation stage. `presentation_correction::tests` (7: zero-tick and next-tick hold P1, compose, converge, whole snap P18, clears, others bit-equal, bounds displacement); `mover_glide` same remainder at 30/60/240 Hz; workspace `--tests` checks clean |
| 6 | **Client mover-correction seeding.** Capture the pre-correction presented pose at apply. Seed from the previous frame's drawn pose, so corrections compose. Snap when any point of the mover's bounds moves by the teleport floor, including rotation-only corrections. Fix zero-tick previous-slot handling (P1, P18). Point the cull and the fire-time sound anchor at the presented pose. Write the before/after step measurement. Rows 10–14, 32. | integrating executor | 5 || done: apply corrects through `correct_transform` (glide when already bound, whole on first bind or rebind); `settle_mover_correction_snaps` clears a composed glide moving any point within the collider's reach by ≥ 3 m (translation + 2·reach·sin(θ/2)); cull also admits the presented AABB while a glide runs; sound fire-time anchor carries the glide. Proofs: `mover_correction_holds_the_drawn_deck_on_the_frame_it_lands` (row 13, conditioned spin changes: worst same-frame step 0.7397 m / 0.3000 rad before → 0.000001 m / 0.000000 rad after); `zero_tick_mover_correction_step_before_and_after` (row 13 synthetic, P1: 0.2600 m → 0); `snap_tests` (rows 14, 22: floor, rotation-only P7); `mover_presented_in_view_draws_while_its_corrected_pose_is_out_of_view`, `mover_sound_fires_at_the_drawn_pose_while_a_correction_glides` (row 32); netcode lib 1142 pass; postretro sound/mover tests 96 pass |
| 7 | **Presented `MoverPoseSource` and per-frame foot probes on every seat.** Move SP/host rider probes out of `SteerPose` into a per-frame Presentation pass over riders only; non-riders keep the per-tick probe. Point the host-seat and client probes at the presented source. Row 9. | integrating executor | 3, 6 | |
| 8 | **Local eye rides the deck.** Endpoints carry the capsule centre and ground. When both name one mover, blend in the mover frame and place through the presented pose, with the world offset and eye height added along world-up. On a client the mesh (shadow and mirror) always carries the world offset, attached or not (owner ruling). View yaw takes the presented correction's world-up yaw. Every `push_state`/`hold_state` caller: holds never attach. Rows 24–28, 31. | integrating executor | 6 | |
| 9 | **Own-pawn share and hand-offs.** Keep the mover-explained share out of the reconcile offset. Hand the residual at the eye's point between deck and world offset on land, leave, change, despawn and rebind. Add the render-only view-yaw offset. Teleports cut; a snapping deck cuts. Rows 15–23, 29, 30. | integrating executor | 8 | |
| 10 | **Authority and scope proofs and the stress run.** Presentation-off A/B bit-equality, including serialized snapshots and `WIRE_VERSION`. Non-rider equality. An every-remote-riding fixture across several movers. `alloc_probe` in netcode tests. Presentation stage cost before and after. Rows 33, 35, 36. | integrating executor | 4, 7, 9 | |
| 11 | **Manual recipe and context.** Co-op carousel recipe in `content/dev/maps/kinematic-platform.README.md`. Row M1 setup. | integrating executor | 10 | |

Hot paths every task honours: the registry accessor, rider placement, rider foot probes and the smoothing advance. None allocates per frame in steady state, and every per-frame pass runs inside `FrameStage::Presentation`.

Out of scope, noted for a follow-up: `crates/sim/src/sim/tick.rs.orig` is a tracked stray merge artifact (from merge 3b4be03c) and is not compiled.
