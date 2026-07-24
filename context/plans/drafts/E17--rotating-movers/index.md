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
optionally translating along its path, a player standing on it revolves and (when
the platform is authored to reorient riders) turns with it, and leaving it imparts
the tangential velocity of the spin. Spin rate is runtime-controllable through a new
authoring command that ramps the platform toward a target rate. Rotation is a
deterministic function of replicated phase (spin angle, current rate, target rate),
so rotating bases predict and reconcile with the same mechanism A established for
linear movers.

## Scope

### In scope

- Continuous spin authored on `kinematic_mover`: a local-space `spin_axis`, an
  authored initial `spin_speed` (degrees/second), and a static `spin_accel`
  (degrees/second², default 0 = snap). Spin composes with linear waypoint
  translation — the mover translates its position along the path **and** rotates its
  orientation about the axis.
- **Runtime spin-rate control (ramp-capable).** Spin rate is dynamic, not purely
  static: the driver holds a replicated *current rate* and *target rate*, and each
  active tick advances the current rate toward the target by `spin_accel * dt`
  (snapping when accel is 0), then advances the spin angle by the current rate. A new
  `set_spin_rate` command verb sets the target rate; a signed target reverses spin,
  and an instant set is the degenerate case (accel 0, or a large accel). This is a
  scripting-primitive addition (SDK type, validation, reaction constructor).
- A **pure rotator** (turntable/carousel/rotating door): a mover that only spins,
  authored with a single-waypoint `path` (its own origin), valid only when authored
  spin is non-zero. Non-spinning movers keep A's ≥2-waypoint requirement unchanged.
- Deterministic angular driver: a `spin_angle_rad` phase accumulator advanced each
  active tick by the current rate, wrapped to `[0, 2π)`, written into
  `Transform.rotation`. Spin (and the ramp) advance while the mover is
  `started && !completed`; `stop` freezes them, `start` resumes them.
- Angular kinematic reporting: the per-tick mover pose gains angular velocity and the
  tick's rotation delta, alongside the existing linear velocity and tick delta.
- **Position/planted carry (always on).** A player whose ground reference is
  `Mover(id)` revolves about the mover's spin axis (through the mover origin) by the
  tick's rotation delta, then translates by the linear tick delta — staying planted on
  the same spot of a turntable. This is non-optional; without it the rider slides off.
- **Yaw/orientation carry (authored toggle, default off).** A per-mover `carry_yaw`
  bool. When set, the rider's view yaw carries with the platform's rotation about
  world-up (turntables meant to reorient the player). When unset (the default), the
  platform revolves the rider's position but never touches their view or aim — aim
  authority is preserved by default. Platform pitch/roll never tilt the camera either
  way — the upright-FPS invariant holds. Applied at the look/camera seam on the owning
  client, riding the existing `facing_yaw` input replication; no new player-orientation
  wire field. The host does not independently apply yaw carry to a remote pawn: the carry
  is applied once, on the owning client, and reaches the host and other clients only
  through the already-carried replicated `facing_yaw` — never double-counted.
- **Leave/detach velocity policy.** On transition off a `Mover` reference, the player
  inherits the tangential linear velocity `angular_velocity × (player_pos − pivot)` at
  the leave position, in addition to A's linear release. The player gains no angular
  momentum (no player spin).
- Reconciliation for rotating bases: replicate `spin_angle_rad`, the current rate, and
  the target rate; clients predict spin and the ramp from them plus static axis/accel
  and reconcile in place within A's mover tolerance.
- A dev-map rotating platform (a carousel) proven single-player and over the in-memory
  net harness.

### Out of scope

- Blocking/crush by a rotating face (E17-E owns blocking/crush; this slice keeps A's
  displace-only push, extended to rotating geometry).
- Player angular momentum / imparted spin, ragdoll, or physical torque.
- Camera pitch/roll carry from platform tilt (upright invariant is deliberate).
- Decoupling the camera-relative locomotion basis from yaw carry. FPS locomotion is
  camera-relative by definition (device-agnostic — the look axis is mouse or right
  stick); the basis follows the carried camera yaw, and the `carry_yaw` toggle is the
  control. A basis decoupled from the camera is not built.
- Scale-carrying movers, non-uniform scale, or nested/parented mover hierarchies.
- Rotating movers as visibility/portal occluders (E17-F) and baked-shadow occlusion.
- Renderer work: the mover draw path already consumes the full interpolated `Transform`
  (position + slerped rotation) via `interpolated_transform` and a `Mat4` instance — no
  renderer change is needed once the driver writes `Transform.rotation`.

## Motion and command semantics

- **Spin pivot** is the mover origin (its first/`path` waypoint), in world space,
  translating with the mover. The world-space spin axis equals the authored local axis
  (a body rotating about its own axis leaves that axis fixed).
- **Rate ramp.** The driver advances the current rate toward the target rate by
  `spin_accel * dt` each active tick, clamped so it never overshoots the target; when
  `spin_accel` is 0 the current rate snaps to the target on the next active tick. The
  spin angle then advances by the (post-ramp) current rate. The driver is a pure
  function of `{phase (angle, current rate, target rate), static (axis, accel), dt}`.
- **Gating.** Spin and the ramp advance iff `started && !completed`. A once-mode
  translate+spin mover that completes its linear path also stops spinning (the whole
  mover is done). A pure rotator never sets `completed` (its degenerate path is
  non-traversable), so it spins until `stop`. A ping-pong translate+spin mover spins
  continuously while it shuttles.
- **Command verbs** (`start`/`stop`/`reverse`/`go_to_path_node`/`set_spin_rate`):
  - `stop` = **freeze**: zeros reported angular velocity and freezes the angle; the
    current-rate and target-rate phase are retained so `start` resumes the same spin.
  - `start` resumes spin and the ramp from the retained phase.
  - `reverse` and `go_to_path_node` retarget the *linear* path only; they do not touch
    spin (spin reversal is `set_spin_rate` with a signed target, not `reverse`).
  - `set_spin_rate(target)` sets the **target rate** only; it does not change the
    started/completed gate. A signed target reverses spin through the ramp. Graceful
    spin-down is `set_spin_rate(0)` (a ramp to rest, distinct from `stop`'s hard
    freeze). Setting the target while stopped takes effect when the mover resumes.

## Acceptance criteria

- [ ] **AC1.** `sdk/TrenchBroom/postretro.fgd` `kinematic_mover` gains `spin_axis`,
  `spin_speed`, `spin_accel`, and `carry_yaw` keys (inspection/review gate — presence is
  checked by reading the FGD, not by a runnable test). `prl-build`
  compiles a map with a spinning mover into the `KinematicGeometry` section carrying
  the axis, speed, accel, and `carry_yaw` (this half is the runnable assertion).
- [ ] **AC2.** `prl-build` accepts a **pure rotator** (single-waypoint `path`, non-zero
  authored spin) and still rejects a non-spinning mover whose `path` resolves to fewer
  than two waypoints, each with a diagnostic. A non-finite `spin_speed`, a non-finite or
  negative `spin_accel`, or a non-finite/zero-length `spin_axis` paired with non-zero
  `spin_speed` is rejected.
- [ ] **AC3.** A previously compiled PRL with a version-1 `KinematicGeometry` section
  still loads; its movers default to zero spin, zero accel, and `carry_yaw = false`
  (version-2 adds the spin fields; the loader accepts both).
- [ ] **AC4.** The runtime drives a spinning mover deterministically: `Transform.rotation`
  advances by the current rate about `spin_axis`, wraps without drift, and re-simulating
  from a mid-spin phase reproduces the orientation trajectory exactly.
- [ ] **AC5.** The ramp is deterministic: with `spin_accel > 0` the current rate advances
  toward the target by `spin_accel * dt` (and snaps when `spin_accel = 0`); re-simulating
  from a mid-ramp phase (current + target rate) reproduces the rate and orientation
  trajectory exactly.
- [ ] **AC6.** `set_spin_rate(target)` sets the target rate; a signed target reverses the
  spin through the ramp; with `spin_accel = 0` the current rate snaps to the target on the
  next active tick; `stop` still hard-freezes and retains the rate phase (distinct from
  `set_spin_rate(0)`'s ramp to rest). The command mutates only deterministic phase (no
  clock/RNG/host state), and the SDK/reaction path validates a finite target rate.
- [ ] **AC7.** A player standing on a rotating turntable for ≥10 full revolutions stays
  planted on the same surface spot (XZ offset from the axis constant within ε, Y within ε
  of the surface) and does not slide off or accumulate drift — verified in the
  deterministic sim. (Position/planted carry, independent of `carry_yaw`.)
- [ ] **AC8.** With `carry_yaw` set, the rider's view yaw advances with the platform's
  world-up rotation each grounded tick; platform pitch/roll produce no camera pitch or
  roll (upright invariant) — holds identically single-player and on a connected client
  **by construction**: yaw carry is applied on the owning client at the look seam and
  reaches others only through A's already-tested `facing_yaw` replication, so no separate
  connected-client yaw test is required (single-player and connected are the same
  client-local code path).
- [ ] **AC9.** With `carry_yaw` unset (the default), the platform revolves the rider's
  position (AC7 still holds) but the rider's view yaw and aim are untouched — the camera
  yaw is unchanged by the platform's spin, preserving aim authority — holds identically
  single-player and on a connected client **by construction**, for the same reason as AC8:
  `facing_yaw` is untouched on the owning client and that (unchanged) value is what
  replicates.
- [ ] **AC10.** Leaving a rotating platform imparts the tangential linear velocity at the
  leave point (`angular_velocity × radius`) plus A's linear release, and no player angular
  velocity — verified deterministically single-player and in connected-client replay.
- [ ] **AC11.** The swept displace-out-of-penetration assertion uses an **advancing+rotating**
  mover (linear tick delta present) pushing into a stationary player: it still displaces the
  player out of penetration (A's displace-only push, now over rotating geometry); no
  tunneling, no persistent overlap. (The swept-push path early-continues when the linear tick
  delta ≈ 0, so it does not model a *pure* rotator's sweep — a pure rotator's penetration is
  instead handled by the per-tick static-overlap displacement, with no persistent overlap;
  deep high-speed pure-rotational swept tunneling is out of scope this slice — displace-only
  push, crush/blocking deferred to E17-E.)
- [ ] **AC12.** In the deterministic net harness at the E15 profile (`LinkConfig { delay:
  45, jitter: 60, loss_probability: 0.05 }` + fixed seed), the client's predicted mover
  orientation tracks the host within a dedicated angular tolerance (radians, authored
  alongside the harness test and distinct from the linear `MOVER_TOLERANCE_M`, compared
  wrap-aware modulo 2π) with no accumulating correction; a `set_spin_rate` command fired
  mid-scenario reconciles
  the ramp with no accumulating correction; and a rider reconciles its revolved position
  in place without steady-state drift.
- [ ] **AC13.** `SNAPSHOT_VERSION` and `WIRE_VERSION` are bumped (the app-protocol/
  vocabulary id is intentionally unchanged — no new message); the wire carries
  `spin_angle_rad`, the current rate, and the target rate; wire drift/round-trip guards
  pass; a peer on the old version is refused at the handshake.
- [ ] **AC14.** No new `unsafe`; no non-renderer module imports `wgpu`; no renderer source
  changes (CI-grep / review gates, not runnable unit tests).
- [ ] **AC15.** Non-spinning movers and mover-less/static maps behave and reconcile
  exactly as before (existing mover and movement suites pass unchanged).

## Tasks

### Task 0a: Split the mover driver module (behavior-preserving)

`crates/postretro/src/kinematic_mover.rs` is ~1015 lines. Before adding the angular
channel, extract the command-applier and reaction/sequence registration surface
(`MoverCommandDiagnostics`, `apply_mover_command`, `apply_mover_command_to_*`,
`register_mover_reaction_primitives`, `register_sequenced_mover_primitives`, and their
tests) into a sibling module, leaving the deterministic geometry driver
(`run_kinematic_mover_tick`, `advance_mover_phase_one_tick`, `advance_mover`, and the
private helpers) in place. No behavior change; the angular driver in Task 1 lands in the
isolated driver half, and the `set_spin_rate` verb in Task 1b lands in the isolated
applier half. Keep `pub(crate)` visibility identical.

### Task 0b: Split the mover-carry helpers out of the movement substrate (behavior-preserving)

`crates/postretro/src/movement/substrate.rs` is ~841 lines. Extract the mover carry/push
helpers (`apply_mover_carry`, `apply_mover_release_velocity`, `displace_from_movers`,
`ground_ref_from_hit`) into a `movement/mover_carry.rs` submodule, called from the same
sites in `integrate_collision`. No behavior change; Task 2's angular carry then extends the
isolated helpers.

### Task 1: Angular driver channel, rate ramp, and phase

Add to `KinematicMoverComponent` (`crates/entities/src/components/kinematic_mover.rs`):
static `spin_axis: Vec3`, `spin_accel_rad_s2: f32`, and `carry_yaw: bool` (seeded at
construction, not replicated); and phase `spin_angle_rad: f32`, `spin_rate_rad_s: f32`
(current rate), `spin_target_rate_rad_s: f32` (target rate) — all three replicated. Grow
`KinematicMoverComponent::new` with the new args in order `(spin_axis, initial spin rate,
spin_accel_rad_s2, carry_yaw)` — the initial rate seeds both `spin_rate_rad_s` and
`spin_target_rate_rad_s`; `spin_angle_rad` seeds to 0. Update every call site (loader in
`runtime_movers.rs`, driver/command tests, wire-convert fixtures). The `runtime_movers.rs`
loader call site passes zero/default spin (`Vec3::ZERO`, `0.0`, `0.0`, `false`) until Task 4
threads the real record values.

Extend the deterministic driver (Task 0a's isolated half) so each active tick
(`started && !completed`, non-zero current-or-target rate): (1) advances
`spin_rate_rad_s` toward `spin_target_rate_rad_s` by `spin_accel_rad_s2 * dt`, clamped so
it never overshoots, snapping when `spin_accel_rad_s2` is 0; (2) advances
`spin_angle_rad += spin_rate_rad_s * dt`, wraps to `[0, 2π)`; (3) writes
`Transform.rotation = Quat::from_axis_angle(spin_axis, spin_angle_rad)`. Report the tick's
angular kinematics: extend `MoverPose` and `MoverTickState` (`collision/moving.rs`,
`crates/postretro/src/kinematic_mover.rs`) with `angular_velocity: Vec3` (world axis × signed current rad/s,
ZERO when inactive), `tick_rotation_delta: Quat` (the rotation applied this tick,
`Quat::IDENTITY` when inactive), and `carry_yaw: bool` (surfaced from the static field so
the Task 2 look seam gates without a second component lookup). Two sites build a `MoverPose`
and must populate the new fields: the pose bridge `MoverTickStateTable::pose` /
`MoverPoseSource` (`crates/postretro/src/kinematic_mover.rs:80`) and the client replay pose
builder `mover_pose_for_current_phase` (`:129`, the Task 5 derivation site). `stop` zeros the reported
angular velocity along with linear (freeze) but retains the rate phase; `start` resumes;
`reverse`/`go_to_path_node` do not touch spin. Keep the driver a pure function of
`{phase (angle, current rate, target rate), static path (incl. axis/accel), dt}`. Tests
(in-memory): spin determinism and wrap (same seed → same orientation; no unbounded
growth); ramp determinism and mid-ramp replay (current-rate advance toward target; snap
when accel 0); mid-spin replay reproduces orientation; spin+translate composition;
pure-rotator spins with a non-traversable path and never completes; once/ping-pong gating;
`stop`/`start` freeze/resume with rate phase retained.

### Task 1b: `set_spin_rate` command verb and SDK surface

Add `SetSpinRate(f32)` to the `MoverCommand` enum
(`crates/entities/src/components/kinematic_mover.rs`; serde `set_spin_rate`), carrying the
target rate in deg/s (authoring unit). The `rate` arg is deg/s at every surface — the
command, the `moverSetSpinRate` primitive, and the SDK `setSpinRate` all carry deg/s,
matching the FGD `spin_speed` key; only the applier converts to rad/s. In
`apply_mover_command` (Task 0a's isolated applier
module), handle `SetSpinRate(target)` by writing `spin_target_rate_rad_s` (converting
deg/s → rad/s, mirroring the load-time `spin_speed_deg_s → rad/s`); it touches only that
phase field — not `started`/`completed`. A non-finite target is rejected (warn-and-skip),
matching the applier's other invalid-input handling. This is a **scripting-primitive
addition** (`context/lib/index.md` principle 6, `context/lib/scripting.md` §12): register
`moverSetSpinRate` in both `register_mover_reaction_primitives` and
`register_sequenced_mover_primitives` alongside the existing
`moverStart`/`moverStop`/`moverReverse`/`moverGoToPathNode` routes, deserializing args
`{ rate: number }` (deg/s) via a `MoverSetSpinRateArgs { rate: f32 }` struct (the same
shape as `MoverGoToPathNodeArgs`) and validating a finite `rate`. Add the SDK handle method
`setSpinRate(rate: number): SequenceStep[]` to `MoverEntityHandle`
(`sdk/lib/entities/movers.ts`), emitting `{ id, primitive: "moverSetSpinRate", args:
{ rate } }`; regenerate `sdk/types/postretro.d.ts`/`.d.luau` and pass the typedef drift
test. Tests: the reaction/sequence primitive routes through the shared applier (matching the
existing `script_mover_primitive_matches_shared_kvp_command_path` pattern); signed target
reverses spin through the ramp; accel-0 snap; non-finite target is skipped;
`set_spin_rate(0)` ramps to rest while `stop` hard-freezes.

### Task 2: Angular carry, orientation policy, and detach velocity

Extend the Task 0b carry helpers. In `apply_mover_carry`, when the previous ground is
`Mover(id)` with a rotating pose, revolve the player position about the spin axis through
the pivot (`pose.transform.position − pose.tick_delta`, the start-of-tick origin) by
`pose.tick_rotation_delta`, then add `pose.tick_delta` — revolve-then-translate, before the
step-up probe and sweep in `integrate_collision`. This position/planted carry runs
regardless of `carry_yaw`. In `apply_mover_release_velocity`, on leaving a `Mover`
reference add the tangential linear velocity `pose.angular_velocity × (player_pos − pivot)`
once, in addition to A's linear `pose.linear_velocity`; add no angular velocity to the
player. `pivot` here is the same start-of-tick origin used by carry
(`pose.transform.position − pose.tick_delta`). `apply_mover_release_velocity`
(`crates/postretro/src/movement/substrate.rs:562`) currently carries no player position in
its signature — widen it to receive the current player position, available at the call site
`substrate.rs:481`.

Implement yaw orientation carry at the look/camera seam that feeds
`MovementInput.facing_yaw`. The carry is injected where `facing_yaw` is assembled from the
camera yaw — the `facing_yaw: camera.yaw` assignment in `crates/postretro/src/main.rs`
(~`main.rs:895`), near the camera-yaw integration (~`main.rs:1902`) — not in `input/look.rs`
(`LookInputs` has no access to the ground ref, mover pose, or camera yaw); grep
`facing_yaw: camera.yaw` to relocate the site. This is where the mover pose table and the
player's ground ref are reachable. (The `MovementInput.facing_yaw` field itself is declared
in `crates/postretro/src/movement/mod.rs:56`.) While the owning player is grounded on a spinning mover **whose
`carry_yaw` is set** (read from `pose.carry_yaw`), add the world-up component of the mover's
tick rotation (`tick_rotation_delta` projected onto world-up → a yaw angle) to the camera
yaw before `facing_yaw` is resolved, so it rides existing input replication. When
`carry_yaw` is unset the seam adds nothing — the camera yaw and aim are untouched. The look
seam runs in the Input stage, one stage ahead of the Game-logic stage that produces this
tick's mover pose; it reads the previous tick's settled mover pose via the ground ref's
`Mover(id)` (grounding is itself a prior-tick fact) and carries that prior-tick
`tick_rotation_delta`. This one-tick lag is accepted and required: carrying the same-tick
delta would move the read into game logic and need a new player-orientation wire field,
which is out of scope. Pitch/roll of the platform contribute nothing to the camera (upright
invariant). Thread the new `MoverPose` angular fields (incl. `carry_yaw`) through the replay
pose source unchanged in shape. Tests (deterministic sim): ≥10-revolution planted-rider
carry (constant axis-relative XZ offset, Y within ε) with `carry_yaw` both on and off; yaw
carry advances with world-up spin only when `carry_yaw` is set and is zero when it is unset
or for a pure pitch/roll axis; aim/view unchanged in the `carry_yaw`-off case; tangential
detach velocity direction and magnitude; no player angular momentum; push displacement over
a rotating face. Note: the mover collider is already posed with `Transform.rotation` in the
collision isometry (`collision/moving.rs`), so `displace_from_movers` and the carry sweep
test against the rotated collider with no new posing code — the rotating-face push follows
automatically once the driver writes `Transform.rotation`.

### Task 3: PRL format, FGD, and compiler

Append `spin_axis: [f32; 3]`, `spin_speed_deg_s: f32`, `spin_accel_deg_s2: f32`, and
`carry_yaw: bool` to `KinematicMoverRecord`
(`crates/level-format/src/kinematic_geometry.rs`) after the existing fields; bump
`KINEMATIC_GEOMETRY_VERSION` 1 → 2. Relax `from_bytes` to accept versions `{1, 2}` — a
version-1 body has no spin fields and decodes to zero spin (`spin_axis = [0,0,0]`,
`spin_speed_deg_s = 0`, `spin_accel_deg_s2 = 0`, `carry_yaw = false`); version-2 reads them.
Add the FGD keys `spin_axis(string)` (local `"x y z"`, default `"0 0 0"`),
`spin_speed(float)` (deg/s, default `"0"`), `spin_accel(float)` (deg/s², default `"0"`),
and `carry_yaw` (bool, default `false`). Compiler (all in `crates/level-compiler/`): parse
the axis string, speed, accel, and carry_yaw in `parse.rs` (`parse_kinematic_mover`),
threading the fields through its `PendingKinematicMover` and relaxing the <2-waypoint
rejection in `resolve_kinematic_path` for the pure-rotator case; thread the fields through
`map_data.rs` (`MapKinematicMover`); set the record fields in `kinematic_geometry.rs`
(`encode_kinematic_geometry_section`, which maps `MapKinematicMover → KinematicMoverRecord`).
`pack.rs` needs no change — it only serializes the already-built section (the byte
serialization is in `crates/level-format/src/kinematic_geometry.rs`, named above). Validate
finite `spin_speed` and finite non-negative `spin_accel`, and when `spin_speed` is non-zero
require a finite non-zero `spin_axis` (normalize at compile) and allow the mover's `path` to
resolve to a single waypoint (its origin) — a **pure rotator**; when `spin_speed` is zero
keep A's ≥2-waypoint requirement. The per-record decoder `read_mover`
(`crates/level-format/src/kinematic_geometry.rs:163`) has no version parameter today —
thread the section version into it so v1 skips the spin fields (default) and v2 reads them.
Serialization/round-trip tests for both section versions; the existing test
`empty_section_round_trips_with_version_and_zero_counts` (`:588`) asserts an exact version
byte literal (`[1,0,0,…]`) and must be updated for the 1→2 bump.

### Task 4: Runtime loading and spawn

`spawn_from_geometry` consumes `LoadedKinematicMover` (`crates/level-loader/src/prl.rs`), not
`KinematicMoverRecord` directly. Append `spin_axis: Vec3`, `spin_speed_deg_s: f32`,
`spin_accel_deg_s2: f32`, and `carry_yaw: bool` to `LoadedKinematicMover` and map them in
`impl From<KinematicMoverRecord> for LoadedKinematicMover` (`prl.rs`), mirroring the existing
`speed → speed_mps` precedent. Thread `spin_axis` (normalized), `spin_speed_deg_s → rad/s`
(the initial rate seed), `spin_accel_deg_s2 → rad/s²`, and `carry_yaw` from the loaded
`LoadedKinematicMover` into `KinematicMoverComponent::new` at the spawn site
(`spawn_from_geometry`, `crates/postretro/src/runtime_movers.rs`), seeding `spin_angle_rad`
to 0 and both current/target rate to the initial rate. A pure rotator resolves its
single-waypoint chain to a one-element `waypoints`/`waypoint_names`; the runtime chain
resolver `resolve_waypoint_chain` (`crates/postretro/src/runtime_movers.rs:373`) currently
rejects any chain of fewer than two waypoints, so relax that rejection **only when authored
spin is non-zero** (the pure-rotator case), keeping the ≥2-waypoint rejection intact for
zero-spin movers so the zero-spin invariant holds; confirm the driver then treats the
one-element chain as non-traversable (spins in place). Host and client both spawn from the record as
in A. No new update-order stage — the existing fixed-tick mover system now advances rotation
and the ramp as well.

### Task 5: Networking — spin phase replication and rotating-base harness

Append `spin_angle_rad: f32`, `spin_rate_rad_s: f32`, and `spin_target_rate_rad_s: f32` to
`WireKinematicMoverState` (`crates/net/src/wire.rs`) after `target_segment`; add all three
to `all_finite`. Update raw↔typed conversion, baseline/delta, and drift/round-trip tests. Map the three fields
at the mover-phase serialize site `kinematic_mover_state_to_wire`
(`crates/postretro/src/netcode/replication.rs:266`) and the client-side seed
`seed_kinematic_mover_phase` (`crates/postretro/src/netcode/client.rs:2090`) — not
`wire_convert`, which only handles `SimCommand`↔`InputCommand`. Bump `SNAPSHOT_VERSION` 10 → 11 in
`crates/net/src/wire.rs` (update the drift guard near `wire.rs:1847`,
`PRE_E21_SNAPSHOT_VERSION`) and `WIRE_VERSION` 10 → 11 in `crates/net/src/transport.rs`
(update the drift guard near `transport.rs:735`). Client apply seeds the predictive driver's
`spin_angle_rad`, `spin_rate_rad_s`, and `spin_target_rate_rad_s` from the replicated phase
so orientation and the ramp reconcile rather than free-run; the client re-runs the ramp
identically from the replicated current+target rate and the locally-held static accel
(host-authoritative target — the host evaluates the firing trigger per `networking.md`).

The mover-history buffer already stores the full authoritative `Transform` per tick, so a
rider's replay reads the historical rotation for *position*. The replay `MoverPose`'s
`tick_rotation_delta` and `angular_velocity`, however, are derived analytically on the client
in `mover_pose_for_current_phase` (`crates/postretro/src/kinematic_mover.rs:129`, already
imported by `netcode/client.rs`) from the replicated **current rate**, the locally-held static
`spin_axis` (from the local PRL), `dt`, and the replicated `started`/`completed` flags (both
already on `KinematicMoverState`) — not reconstructed from consecutive history quaternions.
It must populate `angular_velocity`/`tick_rotation_delta` from the replicated current rate +
local static axis + dt under the identical active-tick gate; left unwired it returns
ZERO/IDENTITY and client orientation silently won't reconcile (only AC12's angular tolerance
would catch it). The client applies the identical active-tick gate as the host driver: the angular fields use the
post-ramp current rate only while `started && !completed && rate ≠ 0`; otherwise
`angular_velocity = Vec3::ZERO` and `tick_rotation_delta = Quat::IDENTITY`. Deriving from
`{current rate, axis, dt}` alone would read the wire's retained non-zero current rate for a
stopped or completed mover as spurious carry/release; gating on `started`/`completed` matches
the host's zero/identity report instead, which protects AC10 and AC12 for the
stop/complete-during-ride cases. `carry_yaw` is likewise held locally from the PRL and never
crosses the wire. The existing harness builder `with_moving_platform` constructs a linear
translator, not a rotator — the rotating-platform scenario needs a new rotating-platform
fixture in the harness. The command-injection seam already exists:
`predict_reconcile_harness.rs` exposes `host_registry`/`host_mover`, so a test fires
`set_spin_rate` by mutating the host mover directly, as the existing remote-trigger tests do.
Extend the
prediction/reconciliation harness with a rotating-platform scenario at the E15 profile:
assert the client's predicted orientation tracks the host within an angular tolerance with no
accumulating correction (reuse `assert_non_accumulating`); fire a `set_spin_rate` command
mid-scenario and assert the ramp reconciles with no accumulating correction; assert a rider
reconciles its revolved position in place; and include a leave-mid-ride assertion that the
tangential release matches the single-player policy.

Note on crate boundaries: `kinematic_mover_state_to_wire`, `seed_kinematic_mover_phase`,
`assert_non_accumulating`, and `MOVER_TOLERANCE_M` are grouped above with the wire edits by
topic, but they live under `crates/postretro/src/netcode/` — `kinematic_mover_state_to_wire`
in `netcode/replication.rs`, `seed_kinematic_mover_phase` in `netcode/client.rs`,
`assert_non_accumulating` and `MOVER_TOLERANCE_M` in `netcode/predict_reconcile_harness.rs` —
distinct from the `WireKinematicMoverState`/`all_finite` edits in `crates/net/src/wire.rs`.
`MOVER_TOLERANCE_M` is not a shared constant; it is a per-test local `const = 0.16` declared
inside separate test fns, and it is linear (meters) — the angular assertion does not reuse it.
The angular assertion uses a dedicated tolerance in radians, authored alongside the harness
test, compared wrap-aware (modulo 2π) so the `[0, 2π)` wrap boundary isn't read as a ~2π
error.

### Task 6: Demo map, diagnostics, and documentation

Add a rotating carousel (and optionally a translate+spin variant) to a dev map. The demo
carousel must avoid pinch/crush geometry per `context/lib/movement.md` §6 (displace-only
push, crush deferred to E17-E; dev maps must avoid pinch points) — no geometry that traps a
rider between the rotating face and a wall. Optionally exercise `set_spin_rate` (a triggered
spin-up/spin-down) and a `carry_yaw` turntable in the demo. Diagnostics (non-gated): extend
the mover debug overlay to draw the spin axis and current orientation; include spin (axis,
current/target rate, accel, `carry_yaw`) in the level-load mover summary, with rate and accel
reported in deg/s and deg/s² (matching the authoring units, for author readability). The
overlay is
emitted through the existing (non-renderer) mover debug-overlay draw path — no new renderer
source. Update context docs where the durable contract changed: `movement.md` §6 (Moving
bases — angular carry, the `carry_yaw` yaw-orientation toggle, tangential detach; retract
the "never add angular velocity" linear-only note), `build_pipeline.md` (the spin/carry
FGD/compiler/PRL fields and section version 2), `networking.md` (the `spin_angle_rad` /
current-rate / target-rate phase fields and rotating-base reconciliation), `scripting.md`
§10.6 (the `set_spin_rate` mover command / `moverSetSpinRate` primitive; state that the
documented unit is deg/s), and `entity_model.md` if the mover component contract note
references linear-only motion.

## Sequencing

**Phase 1 (concurrent):** Task 0a, Task 0b — the two behavior-preserving splits, disjoint files.
**Phase 2 (sequential):** Task 1 — angular driver channel, rate ramp, and pose reporting on the split driver. Everything downstream reads the angular pose fields.
**Phase 3 (concurrent):** Task 1b — `set_spin_rate` verb + SDK surface in Task 0a's applier module (consumes Task 1's target-rate phase); Task 2 — angular carry / orientation / detach on the split carry helpers (consumes Task 1's `MoverPose` fields). Disjoint files.
**Phase 4 (sequential):** Task 3 — PRL/FGD/compiler, establishing the on-disk spin/carry data the loader consumes.
**Phase 5 (sequential):** Task 4 — load/spawn, first time compiled spin reaches the driver.
**Phase 6 (sequential):** Task 5 — networking; consumes Task 1's phase fields, Task 1b's command, and Task 2's release policy.
**Phase 7 (sequential):** Task 6 — demo map, diagnostics, docs, QA.

## Boundary inventory

| Name | Rust | PRL / wire / serde | TypeScript / Luau | FGD |
|---|---|---|---|---|
| spin axis | `KinematicMoverComponent.spin_axis: Vec3` (static, normalized) | PRL `KinematicMoverRecord.spin_axis: [f32;3]` (§2) | n/a | `spin_axis` (string `"x y z"`) |
| spin accel | `spin_accel_rad_s2: f32` (static) | PRL `spin_accel_deg_s2: f32` (§2) | n/a | `spin_accel` (float, deg/s²) |
| carry yaw | `carry_yaw: bool` (static; surfaced on `MoverPose.carry_yaw`) | PRL `KinematicMoverRecord.carry_yaw: bool` (§2) | n/a | `carry_yaw` (bool, default false) |
| spin angle | `spin_angle_rad: f32` (phase, replicated) | wire `WireKinematicMoverState.spin_angle_rad: f32` | n/a | n/a |
| current rate | `spin_rate_rad_s: f32` (phase, replicated) | wire `WireKinematicMoverState.spin_rate_rad_s: f32` | n/a | seeded from `spin_speed` |
| target rate | `spin_target_rate_rad_s: f32` (phase, replicated) | wire `WireKinematicMoverState.spin_target_rate_rad_s: f32` | `MoverEntityHandle.setSpinRate(rate)` → `moverSetSpinRate` | seeded from `spin_speed` |
| set-spin-rate command | `MoverCommand::SetSpinRate(f32)` (deg/s), `apply_mover_command` | rides existing command/reaction replication (mutates target-rate phase) | `moverSetSpinRate` primitive, args `{ rate }` | n/a |
| angular kinematics | `MoverPose`/`MoverTickState` `angular_velocity: Vec3`, `tick_rotation_delta: Quat` | not on the wire (derived from current rate + static axis + dt) | n/a | n/a |
| pure rotator | single-element `waypoints`/`waypoint_names`, non-traversable | PRL `path` resolving to 1 waypoint + non-zero spin | n/a | `path` → one `kinematic_waypoint` |
| yaw orientation carry | added into `MovementInput.facing_yaw` at the look seam, gated by `carry_yaw` | rides existing `WireMovementInput`/command replication | n/a | n/a |

## Wire format

One `bitcode` delta on an existing type; no new snapshot record kind, no new PRL section.

- **`WireKinematicMoverState`** gains `spin_angle_rad: f32`, `spin_rate_rad_s: f32`, and
  `spin_target_rate_rad_s: f32`, appended after `target_segment` in declaration order
  (bitcode owns the bit layout). `all_finite` adds all three; phase-tag validation
  (`direction`/`mode`) is unchanged. Update raw↔typed conversion, baseline/delta, and
  drift/round-trip tests. This is still one existing-type layout change, just with more
  appended fields.
- **PRL `KinematicGeometrySection`** bumps `KINEMATIC_GEOMETRY_VERSION` 1 → 2; the section is
  little-endian and self-contained as before. Two `f32`×3 (axis + wait, unchanged) plus
  `spin_axis` (`f32`×3), `spin_speed_deg_s` (`f32`), `spin_accel_deg_s2` (`f32`), and
  `carry_yaw` (one byte) are appended to each `KinematicMoverRecord`. `from_bytes` accepts
  `{1, 2}`; version 1 decodes to zero spin / zero accel / `carry_yaw = false`.
- Bump `SNAPSHOT_VERSION` 10 → 11 in `crates/net/src/wire.rs` and `WIRE_VERSION` 10 → 11 in
  `crates/net/src/transport.rs` (both existing-type layout changes — networking.md's
  two-gate handshake).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Driver is a pure function of `{phase incl. spin_angle_rad + current/target rate, static incl. axis/accel, dt}` — host and client reproduce identical orientation and ramp | Task 1 | any command verb (`stop`/`start`/`reverse`/`set_spin_rate`) must not add clock/RNG/host state to spin | AC4, AC5, AC6, AC12 |
| Ramp advances current→target by `spin_accel * dt` each active tick (snap when accel 0); `set_spin_rate` sets target only; `stop` freezes without zeroing the rate phase | Task 1 (ramp), Task 1b (verb) | the ramp clamps to avoid overshoot; `stop` retains rate phase so `start` resumes | AC5, AC6 |
| Position/planted carry is always on; carry order is revolve-about-pivot → translate → collision, each tick | Task 2 | `integrate_collision` runs carry before step-up/sweep; reversing the order slides the rider | AC7 |
| Yaw carry runs only when `carry_yaw` is set; only world-up spin carries to the camera; pitch/roll never tilt it; `carry_yaw` off leaves view/aim untouched | Task 2 (seam), Task 3/4 (thread `carry_yaw`) | the look-seam gate reads `pose.carry_yaw` and projects `tick_rotation_delta` onto world-up only | AC8, AC9 |
| Detach adds tangential *linear* velocity once, no player angular momentum | Task 2 | `apply_mover_release_velocity` on `Mover → !Mover` transition | AC10 |
| Orientation and ramp reconcile from replicated `{spin_angle_rad, current rate, target rate}`, no accumulating drift | Task 1 (phase), Task 5 (wire + seed) | client apply seeds all three before predicting; a missing seed free-runs orientation or the ramp | AC12, AC13 |
| Version-1 PRLs still load (zero spin/accel, `carry_yaw` false); non-spinning movers unchanged | Task 3 | `from_bytes` version gate; compiler ≥2-waypoint rule stays for zero-spin movers | AC3, AC15 |

## Open questions

None block this draft. Deferred by design:

- Rotating-face blocking/crush semantics — E17-E; this slice keeps A's displace-only push
  (clean epic boundary).
- Whether spin acceleration should be a per-command parameter (an accel supplied with each
  `set_spin_rate`) rather than the static per-mover `spin_accel` shipped here — static accel
  covers the set-piece cases (spin-up/spin-down/reversal at a tuned rate); add a per-command
  accel only if a set-piece needs distinct ramp rates on one mover. The shipped design is
  static-per-mover.
