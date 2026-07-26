# E17 Follow-Up — Mover-Relative Rider Presentation

> **Status:** draft.
>
> **Depends on:** `context/plans/done/E17--rotating-movers/` and its two
> play-test fixes: intrinsic remote-player locomotion velocity and render-interpolated
> `carry_yaw`.

## Goal

Keep riders, cameras, animation probes, and moving platforms in one render-time frame.
Remote riders remain attached while actor interpolation is delayed; local riders follow
the mover's arc between fixed ticks; mover reconciliation cannot inject a presentation
step that the simulation did not produce.

## Scope

### In scope

- Time-stamped remote `GroundRef` presentation history using the existing
  `WirePlayerMovementState.ground`.
- Mover-local rebasing of delayed remote riders onto the current visual mover.
- Exact `carry_yaw` presentation semantics: position always rebases; orientation
  advances by the historical-to-current world-up mover yaw once only when enabled.
- Landing, detach, mover-change, missing-history, and invalid-frame fallbacks.
- One render-time mover pose consumed by remote rebase, connected-client foot probes,
  local rider interpolation, mover beauty/shadow collection, and reconciliation
  diagnostics.
- Mover-aware local camera and local pawn position interpolation. Attached points
  follow mover rotation/translation instead of the chord between world endpoints.
- Measurement of mover snapshot-reconciliation discontinuities. Add render-only
  smoothing only when the fixed harness crosses the pinned measurement gate.
- CPU-only regression coverage plus manual host/client play-test instructions.

### Out of scope

- Changes to mover simulation, carry, detach velocity, collision authority, or the
  fixed-tick update order.
- New snapshot fields, wire versions, PRL fields, FGD keys, or script primitives.
- Delaying current predicted movers to the remote-actor interpolation time.
- Remote rollback, server rewind, or physics interpolation.
- Camera pitch/roll carry, angular momentum, crush/blocking, nested movers, or scale
  carry.
- Reworking animation selection. The E17 bug fix continues to use replicated intrinsic
  velocity for remote-player locomotion and lower-body heading.
- General temporal smoothing for arbitrary entities.

## Acceptance criteria

- [ ] **AC1.** A stationary remote player riding a rotating and translating mover stays
  at a constant mover-local point through at least ten revolutions under the E15
  conditioned-link profile. The rider and rendered mover differ by at most `1e-4` m
  from the expected composed point in the CPU harness; no accumulating drift occurs.
- [ ] **AC2.** Remote rider orientation advances by exactly the historical-to-current
  world-up mover yaw when `carry_yaw` is enabled, and by zero mover yaw when disabled.
  Pitch/roll never enter player orientation, and repeated sampling never double-applies
  yaw.
- [ ] **AC3.** Remote ground history survives sparse Transform-only and
  movement-only deltas. Intervals that land, detach, or change movers use world-space
  interpolation; the first stable same-mover interval attaches. Missing or invalid
  actor/mover history falls back to the sampled world pose without panic or stale
  attachment.
- [ ] **AC4.** Connected-client foot probes for a rebased remote rider query the same
  render-time mover pose used to place and draw that rider. A planted foot on the
  rotating-platform fixture keeps a hit and a stable model-space contact height within
  `1e-3` across snapshot arrivals and render alphas.
- [ ] **AC5.** For local host, single-player, and connected-client riders, the rendered
  pawn point and camera eye follow the mover arc when both fixed-tick endpoints name
  the same mover. A half-tick quarter-turn matches the geometric midpoint on the arc,
  not the chord midpoint; radius is constant within `1e-4`.
- [ ] **AC6.** Local eye height remains world-up and the existing client reconcile
  offset remains world-space during mover-aware interpolation. Landing, detach,
  mover change, fly-camera, and missing-pawn cases retain the existing world-space
  interpolation with no stale mover attachment.
- [ ] **AC7.** Mover reconciliation diagnostics report simulation position/angular
  correction, same-alpha visual step, render alpha, and whether a fixed tick ran. The
  fixed-seed rotating-platform and synthetic zero-tick harnesses enforce the gate from
  `research.md`: raw steps above 0.002 m or 0.1 degrees require smoothing; otherwise
  no correction store is added.
- [ ] **AC8.** When AC7 requires smoothing, all mover-relative presentation consumers
  use the corrected render-time mover pose, the correction-frame visual step is at most
  `1e-4` m / `1e-4` rad, and the correction converges without accumulating drift. When
  AC7 does not require smoothing, the checked-in harness records that outcome and no
  smoothing path exists.
- [ ] **AC9.** Presentation work never mutates authoritative pawn movement,
  `GroundRef`, mover phase, collision poses, or outgoing replication. Non-riders,
  non-moving bases, transform-only remote entities, and mover-less maps preserve their
  existing presentation.
- [ ] **AC10.** Focused tests, the rotating mover predict/reconcile harness, `cargo
  fmt --check`, clippy with warnings denied, and the full workspace test suite pass.
  No new `unsafe`; no non-renderer code imports `wgpu`.

## Tasks

### Task 1: Split presentation responsibilities

Perform behavior-preserving splits before feature edits. Move
`MoverHistorySample`/`MoverHistoryBuffer` and their tests from
`netcode/client.rs` into `netcode/mover_history.rs`. Move the remote interpolation
sampling, enemy/player playback caches, `ClientPresentationInputs`, and associated
methods into a `RemotePresentationState` owned by `ClientReplication` in
`netcode/remote_presentation.rs`; keep existing `ClientReplication` entry points as
thin forwarding methods where callers need stability. Move the camera-follow,
local-ground, mover-yaw carry, and render-yaw helpers from `main.rs` into
`player_camera.rs`, including the already-implemented render-yaw residual unchanged.
Move `update_foot_ground_probes` and its private helpers/constants from `sim/mod.rs`
into `sim/foot_ik.rs`. Update module declarations and existing tests only. No behavior,
public boundary, frame order, or file outside these seams changes. Verifies AC9 and
unblocks every later task.

### Task 2: Establish one render-time mover pose

Add an App-owned, reusable per-frame mover-presentation table in the kinematic-mover
game layer. Refresh it once after fixed-tick catch-up and before remote interpolation.
For each loaded mover, resolve the base visual transform through
`EntityRegistry::interpolated_transform(entity, frame_alpha)` and retain the fixed-tick
kinematic fields from `MoverTickStateTable`; the table implements `MoverPoseSource`.
Single-player and host frames use that base transform directly. Connected clients may
apply Task 5's optional render correction while refreshing the same table.

Thread read-only access to this table through `client_sample_interpolation` /
`ClientReplication::sample_into_registry`, the connected-client call to
`sim::update_presentation_pose_inputs`, and
`KinematicMoverRenderCollector::collect`. Authoritative `simulate_tick` and movement
collision keep `MoverTickStateTable`. This explicitly gives remote rebase, foot probes,
local camera/pawn presentation, mover beauty, mover shadow instances, and mover
occluder AABBs one pose owner. Verifies AC4 and AC9.

### Task 3: Rebase delayed remote riders

Extend `TransformSample` and `PresentedPose` in `netcode/interpolation.rs` with the
validated remote ground reference. `ClientReplication::apply_components_to` supplies
it from `WirePlayerMovementState.ground`; Transform-only deltas inherit the latest
ground at or before their tick, and movement-only deltas stamp ground beside the held
Transform. The interpolation buffer exposes a mover attachment only when both
bracketing samples name the same mover; it emits no attachment across landing, detach,
or mover change.

Extend `MoverHistoryBuffer` with fractional render-tick transform sampling by resolving
the floor/ceil poses through its existing deterministic replay and lerp/slerp between
them. `RemotePresentationState::sample_into_registry` receives both mover history and
Task 2's current presentation table. For a stable mover attachment, convert the sampled
remote position through historical-mover inverse to local space, then through the
current visual mover. Apply the historical-to-current world-up yaw once only when the
locally loaded `KinematicMoverComponent.carry_yaw` is true; preserve scale and all
orientation otherwise. Fall back to the original sampled world transform for every
invalid/missing case. Run connected-client foot probes against Task 2's table after the
rebased transform write. Verifies AC1–AC4 and AC9.

### Task 4: Interpolate the local rider through mover space

Extend `InterpolableState`/`FrameTiming` with an optional local-rider attachment sample:
mover id, pawn capsule-center point in mover-local space, world-up eye height, and the
existing world-space client correction offset. Capture it at each current
`frame_timing.push_state` site after camera follow; the App call site obtains pawn
identity and `GroundRef` from the registry and the mover frame from
`MoverTickStateTable`. Keep the ordinary world camera/pawn endpoints beside it for
fallback.

At render, when previous and current samples name the same mover, interpolate the two
local pawn points and compose them through Task 2's mover pose. Add interpolated eye
height along world-up and the interpolated reconcile offset in world space. Otherwise
use existing `InterpolableState::lerp`. Supply the resulting eye to
`RenderCamera::new`. Pass the resulting local-pawn world position and pawn `EntityId` to
`MeshRenderCollector::collect_with_hit_zones`; `collect_inner` overrides only that
entity's interpolated position while retaining registry-derived rotation and scale.
All `push_state` callers, `hold_state`, menu/fly-camera paths, and tests explicitly
write `None` attachment. Verifies AC5, AC6, and AC9.

### Task 5: Measure and, only if required, smooth mover reconciliation

Extend the existing `MoverCorrection` observation at
`ClientReplication::apply_components_to`. Thread the current render alpha through
`frame_order::run_snapshot_apply_stage`,
`ReplicatedStateFrame::apply_received_snapshots`, `App::net_poll_and_apply`, and
`client_receive_and_apply`; update the persistent-atmosphere harness implementation.
Before and after applying a mover correction, sample
`EntityRegistry::interpolated_transform` at that same alpha. Record position and wrapped
angular correction, visual step, alpha, and whether the frame has fixed ticks. Surface
bounded `dev-tools` diagnostics and add a pure render-time harness over the rotating
fixture and synthetic zero-tick corrections.

Apply the pinned gate in `research.md`. If it does not cross, retain diagnostics/tests
and add no correction state. If it crosses, add a client-owned per-mover render
correction store seeded so the first post-apply Task 2 pose equals the pre-apply visual
pose. Decay translation and quaternion error once per render frame; clear on despawn,
level unload, mover rebind, non-finite input, or snap-class discontinuity. Only Task 2
reads the store. Registry transforms, mover history, collision, and replication never
do. Check in the measured gate outcome as the harness expectation. Verifies AC7–AC9.

### Task 6: Integrated verification and play-test guide

Extend the existing rotating-platform predict/reconcile fixture rather than adding a
production asset. Cover a stationary remote rider, `carry_yaw` on/off, sparse movement
and Transform deltas, landing/detach, a local half-tick arc, foot contact, snapshot
correction arrival on zero-tick frames, and non-rider controls. Add a concise manual
recipe to the E17 dev-map documentation: host and client ride together, face each
other, exercise spin reversal, jump/detach, and compare host/client views under the E15
conditioned-link profile. Run preflight. Verifies AC1–AC10.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the source seams all feature work extends.

**Phase 2 (sequential):** Task 2 — establishes the shared render-time mover pose.

**Phase 3 (sequential):** Task 3 — consumes Task 2 for remote rebasing and foot probes.

**Phase 4 (sequential):** Task 4 — consumes Task 2 for local camera/pawn interpolation.

**Phase 5 (sequential):** Task 5 — measures the integrated pose path and conditionally adds correction smoothing.

**Phase 6 (sequential):** Task 6 — verifies the complete lifecycle and manual play-test.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| One render-time mover pose feeds every mover-relative presentation consumer | Task 2 | Remote rebase, foot probes, local arc, mover beauty/shadow, optional correction must not resample independently | AC1, AC4, AC5, AC8 |
| Presentation never changes simulation authority | Task 2 | Task 3 rebase, Task 4 attachment history, Task 5 correction store are render-only | AC9 |
| Remote attachment is time-matched before rebasing | Task 3 | Actor and historical mover use the same fractional server tick; current mover is used only after local-space conversion | AC1, AC3 |
| `carry_yaw` advances remote orientation once | Task 3 | Historical player yaw already includes carry through its sample tick; only the remaining mover yaw delta is applied | AC2 |
| Ground transitions never retain a stale mover | Task 3, Task 4 | Any endpoint disagreement disables attachment for that interval | AC3, AC6 |
| Camera eye height and pawn correction stay world-space | Task 4 | Only capsule-center local position composes through mover pitch/roll | AC5, AC6 |
| Reconcile smoothing exists only on measured evidence | Task 5 | The fixed threshold and checked-in harness outcome control whether a correction store is present | AC7, AC8 |

## Open questions

None. Task 5 has a pinned measurement gate, not an implementation-time design choice.
The measured result selects one of two fully specified outcomes.
