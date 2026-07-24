# E17-D — Rotation and Orientation Carry

> **Status:** draft.
>
> **Epic:** 17 — Kinematic Geometry and Moving Platforms.
>
> **Sub-plan:** D. Depends on A (`context/plans/done/E17--kinematic-platform-foundation/`, done)
> and reuses A's command applier from C (`context/plans/done/E17--trigger-command-surface/`).
> Runs after linear carry is stable; the charter says it must not ship in the first foundation.

## Goal

Give kinematic movers angular motion: a mover spins about a local axis while
optionally translating along its path, a player standing on it revolves and turns
with it, and leaving it imparts the tangential velocity of the spin. Rotation is a
deterministic function of one new replicated phase field, so rotating bases predict
and reconcile with the same mechanism A established for linear movers.

## Scope

### In scope

- Continuous spin authored on `kinematic_mover`: a local-space `spin_axis` and a
  `spin_speed` (degrees/second). Spin composes with linear waypoint translation —
  the mover translates its position along the path **and** rotates its orientation
  about the axis.
- A **pure rotator** (turntable/carousel/rotating door): a mover that only spins,
  authored with a single-waypoint `path` (its own origin), valid only when spin is
  non-zero. Non-spinning movers keep A's ≥2-waypoint requirement unchanged.
- Deterministic angular driver: a `spin_angle_rad` phase accumulator advanced each
  active tick, wrapped to `[0, 2π)`, written into `Transform.rotation`. Spin advances
  while the mover is `started && !completed`; `stop` freezes it, `start` resumes it.
- Angular kinematic reporting: the per-tick mover pose gains angular velocity and the
  tick's rotation delta, alongside the existing linear velocity and tick delta.
- **Angular carry.** A player whose ground reference is `Mover(id)` revolves about the
  mover's spin axis (through the mover origin) by the tick's rotation delta, then
  translates by the linear tick delta — staying planted on the same spot of a turntable.
- **Standing-player orientation policy (yaw-only).** The rider's view yaw carries with
  the platform's rotation about world-up. Platform pitch/roll never tilt the camera —
  the upright-FPS invariant holds. Applied at the look/camera seam on the owning client,
  riding the existing `facing_yaw` input replication; no new player-orientation wire field.
- **Leave/detach velocity policy.** On transition off a `Mover` reference, the player
  inherits the tangential linear velocity `angular_velocity × (player_pos − pivot)` at
  the leave position, in addition to A's linear release. The player gains no angular
  momentum (no player spin).
- Reconciliation for rotating bases: replicate `spin_angle_rad`; clients predict spin
  from it plus static axis/speed and reconcile in place within A's mover tolerance.
- A dev-map rotating platform (a carousel) proven single-player and over the in-memory
  net harness.

### Out of scope

- Blocking/crush by a rotating face (E17-E owns blocking/crush; this slice keeps A's
  displace-only push, extended to rotating geometry).
- Player angular momentum / imparted spin, ragdoll, or physical torque.
- Camera pitch/roll carry from platform tilt (upright invariant is deliberate).
- Rotating the player's WASD movement *basis* independently of yaw carry — the movement
  basis follows the carried camera yaw, nothing more.
- Scale-carrying movers, non-uniform scale, or nested/parented mover hierarchies.
- Rotating movers as visibility/portal occluders (E17-F) and baked-shadow occlusion.
- New authoring commands for spin (start/stop/reverse gate spin via A/C's existing verbs;
  no `setSpinRate`-style live command this slice).
- Renderer work: the mover draw path already consumes the full interpolated `Transform`
  (position + slerped rotation) via `interpolated_transform` and a `Mat4` instance — no
  renderer change is needed once the driver writes `Transform.rotation`.

## Motion and command semantics

- **Spin pivot** is the mover origin (its first/`path` waypoint), in world space,
  translating with the mover. The world-space spin axis equals the authored local axis
  (a body rotating about its own axis leaves that axis fixed).
- **Gating.** Spin advances iff `started && !completed`. A once-mode translate+spin mover
  that completes its linear path also stops spinning (the whole mover is done). A pure
  rotator never sets `completed` (its degenerate path is non-traversable), so it spins
  until `stop`. A ping-pong translate+spin mover spins continuously while it shuttles.
- **Command verbs** (`start`/`stop`/`reverse`/`go_to_path_node`) affect spin only through
  the started/completed gate: `stop` zeros angular velocity and freezes the angle; `start`
  resumes. `reverse` and `go_to_path_node` retarget the *linear* path only — spin
  direction is static authoring and is **not** flipped by `reverse`.

## Acceptance criteria

- [ ] `sdk/TrenchBroom/postretro.fgd` `kinematic_mover` gains `spin_axis` and `spin_speed`
  keys (inspection gate). `prl-build` compiles a map with a spinning mover into the
  `KinematicGeometry` section carrying the axis and speed.
- [ ] `prl-build` accepts a **pure rotator** (single-waypoint `path`, non-zero spin) and
  still rejects a non-spinning mover whose `path` resolves to fewer than two waypoints,
  each with a diagnostic. A non-finite `spin_speed` or a non-finite/zero-length `spin_axis`
  paired with non-zero `spin_speed` is rejected.
- [ ] A previously compiled PRL with a version-1 `KinematicGeometry` section still loads;
  its movers default to zero spin (version-2 adds the spin fields; the loader accepts both).
- [ ] The runtime drives a spinning mover deterministically: `Transform.rotation` advances
  by `spin_speed` about `spin_axis`, wraps without drift, and re-simulating from a mid-spin
  phase reproduces the orientation trajectory exactly.
- [ ] A player standing on a rotating turntable for ≥10 full revolutions stays planted on
  the same surface spot (XZ offset from the axis constant within ε, Y within ε of the
  surface) and does not slide off or accumulate drift — verified in the deterministic sim.
- [ ] The rider's view yaw advances with the platform's world-up rotation each grounded
  tick; platform pitch/roll produce no camera pitch or roll (upright invariant).
- [ ] Leaving a rotating platform imparts the tangential linear velocity at the leave point
  (`angular_velocity × radius`) plus A's linear release, and no player angular velocity —
  verified deterministically single-player and in connected-client replay.
- [ ] A mover advancing/rotating into a stationary player still displaces the player out of
  penetration (A's displace-only push, now over rotating geometry); no tunneling, no
  persistent overlap.
- [ ] In the deterministic net harness at the E15 profile (`LinkConfig { delay: 45,
  jitter: 60, loss_probability: 0.05 }` + fixed seed), the client's predicted mover
  orientation tracks the host within A's `MOVER_TOLERANCE_M`-equivalent angular tolerance
  with no accumulating correction, and a rider reconciles its revolved position in place
  without steady-state drift.
- [ ] `SNAPSHOT_VERSION` and `WIRE_VERSION` are bumped (the app-protocol/vocabulary id is
  intentionally unchanged — no new message); wire drift/round-trip guards pass; a peer on
  the old version is refused at the handshake.
- [ ] No new `unsafe`; no non-renderer module imports `wgpu`; no renderer source changes.
- [ ] Non-spinning movers and mover-less/static maps behave and reconcile exactly as before
  (existing mover and movement suites pass unchanged).

## Tasks

### Task 0a: Split the mover driver module (behavior-preserving)

`crates/postretro/src/kinematic_mover.rs` is ~1015 lines. Before adding the angular
channel, extract the command-applier and reaction/sequence registration surface
(`MoverCommandDiagnostics`, `apply_mover_command`, `apply_mover_command_to_*`,
`register_mover_reaction_primitives`, `register_sequenced_mover_primitives`, and their
tests) into a sibling module, leaving the deterministic geometry driver
(`run_kinematic_mover_tick`, `advance_mover_phase_one_tick`, `advance_mover`, and the
private helpers) in place. No behavior change; the angular work in Task 1 then lands in
the isolated driver half. Keep `pub(crate)` visibility identical.

### Task 0b: Split the mover-carry helpers out of the movement substrate (behavior-preserving)

`crates/postretro/src/movement/substrate.rs` is ~841 lines. Extract the mover carry/push
helpers (`apply_mover_carry`, `apply_mover_release_velocity`, `displace_from_movers`,
`ground_ref_from_hit`) into a `movement/mover_carry.rs` submodule, called from the same
sites in `integrate_collision`. No behavior change; Task 2's angular carry then extends the
isolated helpers.

### Task 1: Angular driver channel and phase

Add to `KinematicMoverComponent` (`crates/entities/src/components/kinematic_mover.rs`):
static `spin_axis: Vec3` and `spin_speed_rad_s: f32` (seeded at construction, not
replicated), and phase `spin_angle_rad: f32` (replicated). Grow
`KinematicMoverComponent::new` with the two static args; update every call site (loader in
`runtime_movers.rs`, driver/command tests, wire-convert fixtures). The `runtime_movers.rs`
loader call site passes zero/default spin (`Vec3::ZERO`, `0.0`) until Task 4 threads the
real record values. Extend the deterministic
driver (Task 0a's isolated half) so each active tick (`started && !completed`, non-zero
spin) advances `spin_angle_rad += spin_speed_rad_s * dt`, wraps to `[0, 2π)`, and writes
`Transform.rotation = Quat::from_axis_angle(spin_axis, spin_angle_rad)`. Report the tick's
angular kinematics: extend `MoverPose` and `MoverTickState` (`collision/moving.rs`,
`kinematic_mover.rs`) with `angular_velocity: Vec3` (world axis × signed rad/s, ZERO when
inactive) and `tick_rotation_delta: Quat` (the rotation applied this tick, `Quat::IDENTITY`
when inactive). `apply_mover_command` `Stop` zeros the reported angular velocity along with
linear (freeze); `Start` resumes; `reverse`/`go_to_path_node` do not touch spin. Keep the
driver a pure function of `{phase (incl. spin_angle_rad), static path (incl. axis/speed),
dt}`. Tests (in-memory): spin determinism and wrap (same seed → same orientation; no
unbounded growth); mid-spin replay reproduces orientation; spin+translate composition;
pure-rotator spins with a non-traversable path and never completes; once/ping-pong gating;
`stop`/`start` freeze/resume.

### Task 2: Angular carry, orientation policy, and detach velocity

Extend the Task 0b carry helpers. In `apply_mover_carry`, when the previous ground is
`Mover(id)` with a rotating pose, revolve the player position about the spin axis through
the pivot (`pose.transform.position − pose.tick_delta`, the start-of-tick origin) by
`pose.tick_rotation_delta`, then add `pose.tick_delta` — revolve-then-translate, before the
step-up probe and sweep in `integrate_collision`. In `apply_mover_release_velocity`, on
leaving a `Mover` reference add the tangential linear velocity
`pose.angular_velocity × (player_pos − pivot)` once, in addition to A's linear
`pose.linear_velocity`; add no angular velocity to the player. `pivot` here is the same
start-of-tick origin used by carry (`pose.transform.position − pose.tick_delta`). Implement yaw orientation
carry at the look/camera seam that feeds `MovementInput.facing_yaw`
(`crates/postretro/src/movement/mod.rs:56`; look integration in `input/look.rs`): while the
owning player is grounded on a spinning mover, add the world-up component of the mover's
tick rotation (`tick_rotation_delta` projected onto world-up → a yaw angle) to the camera
yaw before `facing_yaw` is resolved, so it rides existing input replication. The look seam runs
in the Input stage, one stage ahead of the Game-logic stage that produces this tick's
mover pose; it reads the previous tick's settled mover pose via the ground ref's
`Mover(id)` (grounding is itself a prior-tick fact) and carries that prior-tick
`tick_rotation_delta`. This one-tick lag is accepted and required: carrying the
same-tick delta would move the read into game logic and need a new
player-orientation wire field, which is out of scope. Pitch/roll of
the platform contribute nothing to the camera (upright invariant). Thread the new
`MoverPose` angular fields through the replay pose source unchanged in shape. Tests
(deterministic sim): ≥10-revolution planted-rider carry (constant axis-relative XZ offset,
Y within ε); yaw carry advances with world-up spin and is zero for a pure pitch/roll axis;
tangential detach velocity direction and magnitude; no player angular momentum; push
displacement over a rotating face. Note: the mover collider is already posed with
`Transform.rotation` in the collision isometry (`collision/moving.rs`), so
`displace_from_movers` and the carry sweep test against the rotated collider with no
new posing code — the rotating-face push follows automatically once the driver
writes `Transform.rotation`.

### Task 3: PRL format, FGD, and compiler

Append `spin_axis: [f32; 3]` and `spin_speed_deg_s: f32` to `KinematicMoverRecord`
(`crates/level-format/src/kinematic_geometry.rs`) after the existing fields; bump
`KINEMATIC_GEOMETRY_VERSION` 1 → 2. Relax `from_bytes` to accept versions `{1, 2}` — a
version-1 body has no spin fields and decodes to zero spin (`spin_axis = [0,0,0]`,
`spin_speed_deg_s = 0`); version-2 reads them. Add the FGD keys `spin_axis(string)` (local
`"x y z"`, default `"0 0 0"`) and `spin_speed(float)` (deg/s, default `"0"`). Compiler
(`parse.rs`/`pack.rs`): parse the axis string and speed onto the mover record; validate
finite `spin_speed`, and when `spin_speed` is non-zero require a finite non-zero `spin_axis`
(normalize at compile) and allow the mover's `path` to resolve to a single waypoint (its
origin) — a **pure rotator**; when `spin_speed` is zero keep A's ≥2-waypoint requirement.
Serialization/round-trip tests for both section versions.

### Task 4: Runtime loading and spawn

`spawn_from_geometry` consumes `LoadedKinematicMover` (`crates/level-loader/src/prl.rs`), not
`KinematicMoverRecord` directly. Append `spin_axis: Vec3` and `spin_speed_deg_s: f32` to
`LoadedKinematicMover` and map them in `impl From<KinematicMoverRecord> for
LoadedKinematicMover` (`prl.rs`), mirroring the existing `speed → speed_mps` precedent.
Thread `spin_axis` (normalized) and `spin_speed_deg_s → rad/s` from the loaded
`LoadedKinematicMover` into `KinematicMoverComponent::new` at the spawn site
(`spawn_from_geometry`, `crates/postretro/src/runtime_movers.rs`), seeding `spin_angle_rad`
to 0. A pure rotator resolves its single-waypoint chain to a one-element `waypoints`/
`waypoint_names`; confirm the driver treats it as non-traversable (spins in place). Host and
client both spawn from the record as in A. No new update-order stage — the existing
fixed-tick mover system now advances rotation as well.

### Task 5: Networking — spin phase replication and rotating-base harness

Append `spin_angle_rad: f32` to `WireKinematicMoverState` (`crates/net/src/wire.rs`) after
`target_segment`; add it to `all_finite`. Update raw↔typed conversion, baseline/delta, and
drift/round-trip tests. Map it in `wire_convert` alongside the other phase fields. Bump
`SNAPSHOT_VERSION` 10 → 11 in `crates/net/src/wire.rs` (update the drift guard near
`wire.rs:1847`, `PRE_E21_SNAPSHOT_VERSION`) and `WIRE_VERSION` 10 → 11 in
`crates/net/src/transport.rs` (update the drift guard near `transport.rs:735`). Client apply
seeds the predictive driver's `spin_angle_rad` from the replicated phase so orientation
reconciles rather than free-runs. The mover-history
buffer already stores the full authoritative `Transform` per tick, so a rider's replay reads
the historical rotation for *position*. The replay `MoverPose`'s `tick_rotation_delta` and
`angular_velocity`, however, are derived analytically on the client from the locally-held
static `spin_axis`/`spin_speed` (held from the local PRL) and `dt` — not reconstructed from
consecutive history quaternions — so they match the host's analytic value exactly, which
AC7's connected-client assertion requires. Extend the
prediction/reconciliation harness with a rotating-platform scenario at the E15 profile:
assert the client's predicted orientation tracks the host within an angular tolerance with
no accumulating correction (reuse `assert_non_accumulating`), and that a rider reconciles its
revolved position in place; include a leave-mid-ride assertion that the tangential release
matches the single-player policy.

Note on crate boundaries: `wire_convert`, `assert_non_accumulating`, and
`MOVER_TOLERANCE_M` are grouped above with the wire edits by topic, but they live under
`crates/postretro/src/netcode/` — `wire_convert` in `netcode/wire_convert.rs`,
`assert_non_accumulating` and `MOVER_TOLERANCE_M` in
`netcode/predict_reconcile_harness.rs` — distinct from the `WireKinematicMoverState`/
`all_finite` edits in `crates/net/src/wire.rs`. `MOVER_TOLERANCE_M` is not a shared
constant; it is a per-test local `const = 0.16` declared inside separate test fns. The new
angular tolerance either reuses that existing per-test `0.16` value or is hoisted to a
shared const if one is wanted.

### Task 6: Demo map, diagnostics, and documentation

Add a rotating carousel (and optionally a translate+spin variant) to a dev map. Diagnostics
(non-gated): extend the mover debug overlay to draw the spin axis and current orientation;
include spin in the level-load mover summary. The overlay is emitted through the existing
(non-renderer) mover debug-overlay draw path — no new renderer source. Update context docs where the durable contract
changed: `movement.md` §6 (Moving bases — angular carry, yaw orientation policy, tangential
detach; retract the "never add angular velocity" linear-only note), `build_pipeline.md` (the
spin FGD/compiler/PRL fields and section version 2), `networking.md` (the `spin_angle_rad`
phase field and rotating-base reconciliation), and `entity_model.md` if the mover component
contract note references linear-only motion.

## Sequencing

**Phase 1 (concurrent):** Task 0a, Task 0b — the two behavior-preserving splits, disjoint files.
**Phase 2 (sequential):** Task 1 — angular driver channel and pose reporting on the split driver. Everything downstream reads the angular pose fields.
**Phase 3 (sequential):** Task 2 — angular carry / orientation / detach on the split carry helpers, consuming Task 1's `MoverPose` fields.
**Phase 4 (sequential):** Task 3 — PRL/FGD/compiler, establishing the on-disk spin data the loader consumes.
**Phase 5 (sequential):** Task 4 — load/spawn, first time compiled spin reaches the driver.
**Phase 6 (sequential):** Task 5 — networking; consumes Task 1's phase field and Task 2's release policy.
**Phase 7 (sequential):** Task 6 — demo map, diagnostics, docs, QA.

## Boundary inventory

| Name | Rust | PRL / wire / serde | TypeScript / Luau | FGD |
|---|---|---|---|---|
| spin axis | `KinematicMoverComponent.spin_axis: Vec3` (static, normalized) | PRL `KinematicMoverRecord.spin_axis: [f32;3]` (§2) | n/a | `spin_axis` (string `"x y z"`) |
| spin speed | `spin_speed_rad_s: f32` (static) | PRL `spin_speed_deg_s: f32` (§2) | n/a | `spin_speed` (float, deg/s) |
| spin phase | `spin_angle_rad: f32` (phase, replicated) | wire `WireKinematicMoverState.spin_angle_rad: f32` | n/a | n/a |
| angular kinematics | `MoverPose`/`MoverTickState` `angular_velocity: Vec3`, `tick_rotation_delta: Quat` | not on the wire (derived from `spin_angle_rad` + static) | n/a | n/a |
| pure rotator | single-element `waypoints`/`waypoint_names`, non-traversable | PRL `path` resolving to 1 waypoint + non-zero spin | n/a | `path` → one `kinematic_waypoint` |
| yaw orientation carry | added into `MovementInput.facing_yaw` at the look seam | rides existing `WireMovementInput`/command replication | n/a | n/a |

## Wire format

One `bitcode` delta on an existing type; no new snapshot record kind, no new PRL section.

- **`WireKinematicMoverState`** gains `spin_angle_rad: f32`, appended after `target_segment`
  in declaration order (bitcode owns the bit layout). `all_finite` adds the field; phase-tag
  validation (`direction`/`mode`) is unchanged. Update raw↔typed conversion, baseline/delta,
  and drift/round-trip tests.
- **PRL `KinematicGeometrySection`** bumps `KINEMATIC_GEOMETRY_VERSION` 1 → 2; the section is
  little-endian and self-contained as before. Two `f32`×3 + one `f32` are appended to each
  `KinematicMoverRecord`. `from_bytes` accepts `{1, 2}`; version 1 decodes to zero spin.
- Bump `SNAPSHOT_VERSION` 10 → 11 in `crates/net/src/wire.rs` and `WIRE_VERSION` 10 → 11 in
  `crates/net/src/transport.rs` (both existing-type layout changes — networking.md's
  two-gate handshake).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Driver is a pure function of `{phase incl. spin_angle_rad, static incl. axis/speed, dt}` — host and client reproduce identical orientation | Task 1 | any command verb (`stop`/`start`/`reverse`) must not add clock/RNG/host state to spin | AC 4, AC 9 |
| Carry order is revolve-about-pivot → translate → collision, each tick | Task 2 | `integrate_collision` runs carry before step-up/sweep; reversing the order slides the rider | AC 5 |
| Upright-FPS: only world-up spin carries to the camera; pitch/roll never tilt it | Task 2 | the look-seam yaw carry projects `tick_rotation_delta` onto world-up only | AC 6 |
| Detach adds tangential *linear* velocity once, no player angular momentum | Task 2 | `apply_mover_release_velocity` on `Mover → !Mover` transition | AC 7 |
| Orientation reconciles from replicated `spin_angle_rad`, no accumulating drift | Task 1 (phase), Task 5 (wire + seed) | client apply seeds spin before predicting; a missing seed free-runs orientation | AC 9, AC 10 |
| Version-1 PRLs still load (zero spin); non-spinning movers unchanged | Task 3 | `from_bytes` version gate; compiler ≥2-waypoint rule stays for zero-spin movers | AC 3, AC 12 |

## Open questions

None block this draft. Deferred by design:

- Live spin-rate commands (`setSpinRate`-style) — spin is static authoring this slice; add a
  closed verb later if a set-piece needs runtime spin changes (would reuse C's applier shape).
- Rotating-face blocking/crush semantics — E17-E; this slice keeps A's displace-only push.
- Whether the movement *basis* should decouple from yaw carry (strafe-relative-to-world while
  the camera turns) — kept coupled here; revisit only if a control complaint surfaces.
