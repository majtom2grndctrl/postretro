# E17 - Kinematic Platform Foundation

> **Status:** ready — substrate-proof revision reviewed (structural + implementability), awaiting implementation.
>
> **Epic:** 17 - Kinematic Geometry and Moving Platforms.
>
> **Fits after:** E15 Phase 3/3.5 networking and movement prediction/reconciliation.

## Goal

Build the first deterministic moving-world slice: a map-authored brush platform
or elevator compiles into runtime-transformable geometry, moves along a linear
waypoint path, renders as dynamic world geometry, blocks and carries the player,
and replicates enough authoritative phase for connected clients to predict and
reconcile the platform with the same predict/reconcile mechanism the pawn uses
— a separate phase-seeded instance, not the pawn's predictor.

This is the substrate plan. It proves one moving brush payload end-to-end before
the epic adds triggers, rotating carry, doors, dynamic portals, kinematic
clusters, or destruction. The riskiest integration — deterministic driver,
combined collision query, carry, replay — is proven in-memory before PRL, FGD,
renderer, and wire plumbing attach (see **Sequencing**).

## Scope

### In scope

- A brush entity classname `kinematic_mover` in the FGD and compiler.
- Point waypoint entities for a simple path: `kinematic_waypoint`.
- Compile-time extraction of `kinematic_mover` brushes into a new PRL kinematic
  geometry section, separate from static worldspawn BSP, static BVH, static
  collision, lightmaps, SDF, portals, and navmesh.
- Runtime load/spawn of mover entities with `Transform` plus a new
  engine-owned kinematic mover component.
- Deterministic linear movement over a waypoint list. Required modes:
  one-shot and ping-pong.
- Existing fixed frame order only: input, game logic, audio, render, present.
- A renderer-owned draw path for kinematic brush payloads. All WGPU calls stay
  in the renderer crate (`crates/renderer`).
- Collision queries that consider both static `CollisionWorld` geometry and
  active mover colliders.
- Player carry for linear movers: standing on a mover follows its linear delta;
  side/top collision works; leaving the mover preserves explicit movement intent
  plus the carry-release velocity policy in this plan.
- Displace-only mover push: a mover advancing into the player displaces the
  player out of penetration; movers never yield or stop on contact.
- Widen the player's replicated grounded state into a ground reference
  (`Airborne` / `World` / `Mover`) so carry is one authoritative, predicted
  value rather than a separately-derived base.
- Server-authoritative, client-predicted mover state: the host owns motion; each
  client predicts it from the replicated phase and reconciles in place, via the
  same prediction/reconciliation mechanism as the pawn (its own phase-seeded
  instance).
- A small dev map proving one platform or elevator in single-player and over the
  in-memory net harness.

### Out of scope

- Rotating platforms, angular carry, and player orientation changes.
- Crush damage, mover blocking/stopping on contact, and pinch resolution when a
  push is blocked by static geometry (E17-D).
- Touch/use trigger volumes and co-op trigger ownership. The first mover starts
  through `start_on_spawn`.
- Script-driven per-tick motion. Scripts may declare future commands, but Rust
  owns tick evaluation.
- Doors-as-occluders, dynamic portals, dynamic BSP, PVS mutations, and
  visibility blockers.
- Kinematic clusters/sub-worlds, destruction, fracture, rigid-body debris, and
  Rapier integration.
- Navmesh movement over movers.
- Baked static shadows/lightmap occlusion cast by movers.

## Design

### Authoring

Authors create a `kinematic_mover` brush entity and at least two
`kinematic_waypoint` point entities. The mover's `path` points at the
first waypoint's `name`; each waypoint may point at the next waypoint via
`next`.

Waypoint `origin`s are world-space. The mover's authored brush position is its
start pose and must coincide with the first (`path`) waypoint — the compiler
warns on mismatch. Local vertices are stored relative to the mover origin; at
runtime `Transform.position` walks the chain (`path` -> `next` -> ...), so
motion begins at the first waypoint and the platform never teleports on spawn.

First-slice FGD keys:

- `kinematic_mover.name`: stable mover name.
- `kinematic_mover.path`: first waypoint name.
- `kinematic_mover.speed`: meters per second, finite and positive.
- `kinematic_mover.wait_ms`: optional endpoint wait in milliseconds, finite and
  non-negative.
- `kinematic_mover.move_mode`: `once` or `ping_pong`.
- `kinematic_mover.start_on_spawn`: boolean, defaults true.
- `kinematic_mover._tags`: space-delimited script/query tags, matching existing FGD
  convention.
- `kinematic_waypoint.name`: stable waypoint name.
- `kinematic_waypoint.next`: optional next waypoint name.

No trigger key ships in this plan. That keeps the first platform independent of
E18 trigger ownership and lets the platform substrate land before level-event
semantics.

### Runtime Model

Add an engine-owned mover component after `ComponentKind::Brain`:
`ComponentKind::KinematicMover = 13`. The component is not directly queryable
by scripts in this plan, but it is deliberately shaped to *become* queryable in
E17-B: register it as a `world.query` component kind and it slots into the same
selection path lights and fog already use (see **Future Scripting Seam**). This
plan must not foreclose that — no per-mover state may live somewhere a future
`WorldQueryComponent` registration could not reach.

The component stores the deterministic driver state:

- compiled mover id;
- resolved waypoint positions (world-space, seeded at spawn — static path data);
- speed (m/s) and endpoint wait (ms) — static path config, seeded at spawn;
- mode;
- segment index;
- direction sign;
- segment elapsed milliseconds;
- wait remaining milliseconds;
- current linear velocity;
- flags for started/completed (mover blocking/crush is E17-D — no `blocked` state this slice).

The phase fields replicate; the static path data (positions, speed, wait) does
not — every peer loads it from the same PRL. It rides on the component so the
driver stays a pure function of component state and a future `world.query`
registration can reach it.

The mover system runs in the fixed-tick game-logic phase after
`snapshot_transforms` and before player movement consumes collision for that
tick. It updates each mover's `Transform` and records previous/current deltas so
movement carry and renderer interpolation share the same tick state.
Mover-into-player push resolves in that window too, owned by the movement
substrate as the first step of the player-movement stage — no new update-order
stage (see **Collision and Movement Carry**).

The same deterministic driver runs authoritatively on the host and predictively
on each connected client. A client seeds the driver from the replicated phase —
every `WireKinematicMoverState` phase field, endpoint waits
(`wait_remaining_ms`) and `started`/`completed` flags included — and
extrapolates the closed-form path forward, reconciling in place when a snapshot
arrives. It
uses the same predict/reconcile *mechanism* as the pawn — its own phase-seeded
instance, not the pawn's command-ring predictor — mapped by `NetworkId`, with no
provisional client copy (`networking.md`). Motion is a pure function of
{replicated phase, static waypoints, mode, speed}: the client computes
everything itself, **endpoint reversals included** — there is no
non-deterministic input in this plan. Each snapshot re-anchors the client's
phase, so prediction cannot accumulate drift. The only future external input is
E17-B triggers (out of scope here), which will arrive authoritatively.

### PRL and Static World Separation

Today brush entities with brushes are excluded from generic `MapEntity`
dispatch unless a dedicated subsystem consumes them; `fog_volume` is the
precedent. This plan adds that dedicated subsystem for `kinematic_mover`.

`kinematic_mover` brushes must be removed from the static world path:

- not in `MapData::brush_volumes`;
- not in world `GeometrySection`;
- not in static `Bvh`;
- not in static `CollisionWorld`;
- not in static lightmap/SDF occluder bakes;
- not in portal or navmesh construction.

The compiler emits their render and collision payloads as local-space geometry.
The runtime applies the entity transform each tick.

### Rendering

The renderer owns all GPU resources for kinematic geometry. The first path may
draw movers in a dedicated dynamic-geometry pass after opaque world geometry and
before skinned meshes/fog, or may extend an existing renderer-owned dynamic mesh
path if that is cleaner after code inspection. It must not insert mover vertices
into the static world indirect/BVH path.

Lighting can reuse the dynamic-object lighting model: baked indirect/static
direct SH where already available for dynamic objects plus dynamic direct
lights. Movers do not receive static lightmap UVs in this first slice. They do
not cast baked static shadows. Dynamic shadow casting by movers is optional and
should be left out unless already cheap through the chosen draw path.

Keep the Radeon Pro 5500M/WGPU floor: no mesh shaders, no hardware ray tracing,
no new required adapter feature, and no ninth bind group. If a new pass needs
bindings, use an existing compatible layout or a pass-local layout within the
renderer's current limit discipline.

### Collision and Movement Carry

`CollisionWorld` currently owns one static `parry3d::TriMesh`; movement calls
`cast_capsule`/`cast_ray` against it. Add a moving-collider query layer rather
than replacing the static world. The movement substrate should ask one query
surface for nearest static or mover hit and receive:

- hit normal and time of impact;
- source kind: static or mover;
- mover entity id / mover id when applicable;
- mover linear velocity and tick delta;
- contact surface classification.

The aggregator is hand-rolled by design: parry 0.17 has no scene-level query
pipeline — `QueryPipeline` and collider sets are Rapier APIs, and Rapier stays
out (`movement.md`). Keep the API tiny and directly tested: nearest-hit ordering
across sources, and exact static-only degeneracy — with no active movers it
reproduces today's `cast_capsule`/`cast_ray` results.

Carry rides on a **generalized ground reference.** Today the player's replicated
`is_grounded: bool` (`WirePlayerMovementState`) is a bare boolean. Widen it to a
ground reference — `Airborne`, `World`, or `Mover(mover_id)` — so "what am I
standing on" is one authoritative, replicated, predicted value, not a separately
derived carry base. `mover_id` is the compile-time PRL key (a plain `u32`): the
reference is a foundation-local handle, so `PlayerMovementComponent` (foundation)
never names `NetworkId`, which lives up in `postretro-net`. `mover_id` is stable
across peers — both loaded the same PRL — so the ground reference replicates
directly, no `NetworkId` round-trip; the upper crate resolves `mover_id` to the
local mover entity to read its delta.

The grounded/airborne distinction is preserved. Two invariants constrain the
widening:

- change only the *player* grounded state — the wire `WirePlayerMovementState`
  field, the foundation `PlayerMovementComponent` field, and their readers; the
  AI `AgentComponent.is_grounded` is a separate field and must not change;
- the movement scope's scripting `grounded` primitive stays a `bool`, projected
  as `ground != Airborne`, so the primitive surface (a contract, see `index.md`)
  does not change.

The first carry policy is linear only:

- when the ground reference is `Mover(id)`, the player inherits that mover's tick
  delta before or during collision integration so the capsule stays on the
  platform;
- because both the mover and the pawn are predicted from authoritative state,
  the client reproduces the same ground reference and the same delta the host
  computed — carry is predicted and reconciled in place, never interpolated or
  locally guessed;
- horizontal player intent remains relative to world axes for this plan;
- on transition off a `Mover` reference, preserve player-controlled velocity and
  add the mover velocity once; never add angular velocity (rotation is out of
  scope).

Push complements carry. Movers are unstoppable kinematics: they follow the
authored path regardless of contact and do not collide with the static world or
each other — path validity is the author's responsibility. Carry covers the
grounded cases (a rising elevator lifts its rider; a horizontal platform moves a
standing player with it). Push covers the rest: a mover face advancing into the
player's capsule displaces the player out of penetration along the contact
normal, in the same tick as the mover's motion — owned by the movement
substrate as the first step of the player-movement stage, before it consumes
collision — so a tick never ends with the player inside a mover. Push is a
deterministic function of mover phase and player state, so it predicts, replays,
and reconciles exactly like carry.

If the displacement is blocked by static geometry — a pinch — this slice does
not resolve it: crush/blocking policy is E17-D. Dev maps in this slice must not
author pinch points; the movement substrate logs an unresolved pinch in dev
builds, and hitting one in the dev map is a stop condition (see **Sequencing**).

Keep the custom-kinematic movement invariant: no rigid-body player, no Rapier
world, no per-tick script.

### Networking

Server-authoritative, client-predicted — the same predict/reconcile *mechanism*
as the pawn (a separate phase-seeded instance, not the pawn's command-ring
predictor). The host owns mover motion and registers each mover in
`ReplicableSet`; every connected client predicts the mover locally from the
replicated phase and reconciles in place, mapped by `NetworkId`, with no
provisional client copy (`networking.md`). Prediction — not the
remote-interpolation buffer used for remote pawns — is deliberate: a mover's path
is a closed deterministic function of replicated phase plus static waypoints
(reversals included), so a rider and a bystander both see a smooth, lag-free
platform; corrections are small and bounded, driven only by the client's
server-tick estimate under jitter, and never accumulate.

Add a wire payload for mover state in `postretro-net`:

- `COMPONENT_KIND_KINEMATIC_MOVER_STATE = 13`, numeric-equal to
  `ComponentKind::KinematicMover`.
- `RawComponentPayload` gains a `kinematic_mover` option slot.
- `ComponentPayload` gains `KinematicMoverState`.
- Bump `SNAPSHOT_VERSION` (currently 7 → 8) and the protocol gate.

Wire mover fields (a `bitcode`-derived struct like every wire type — see
**Wire Format** for framing and validation):

```text
WireKinematicMoverState {
  mover_id: u32,
  segment_index: u16,
  direction: i8,          // -1 or 1
  mode: u8,               // once=0, ping_pong=1
  segment_elapsed_ms: f32,
  wait_remaining_ms: f32,
  started: bool,
  completed: bool,
  velocity: [f32; 3],
}
```

These phase fields double as the client's prediction seed — no extra wire is
needed for mover prediction. All floats must be finite at validation; invalid
direction/mode values reject the payload before apply.

Carry replicates through the player, not a side channel. Widen
`WirePlayerMovementState.is_grounded: bool` to a ground reference carrying
`Airborne` / `World` / `Mover(mover_id)` (see **Collision and Movement Carry**).
The wire struct is `bitcode`, so this is a bitcode-layout change on an existing
type: update its `bitcode` encode/decode, `all_finite`/validation (finiteness and
a valid enum tag only), raw↔typed conversion, baseline/delta, and drift tests,
alongside the `SNAPSHOT_VERSION`/wire-version bump. The net crate is
registry-blind, so the "is this a loaded mover" check does **not** live in wire
validation — the `mover_id`-unknown rejection is engine-side in `crate::netcode`
client-apply, which owns the mover set. The foundation-side
`PlayerMovementComponent` mirrors the widening on its `serde` derive.

Snapshots also carry `Transform` for the mover as the reconciliation anchor.
Local replay for a player riding a mover reads the authoritative mover-history
samples for the replay ticks, so the pawn replays against the same platform pose
the host used — never a client-authored divergent path.

## Acceptance Criteria

- [ ] `sdk/TrenchBroom/postretro.fgd` includes `kinematic_mover` and
  `kinematic_waypoint` with the keys above (inspection gate — no automated FGD
  test exists).
- [ ] `prl-build` compiles a map with one `kinematic_mover` brush and two waypoints
  into a PRL with a kinematic geometry section. The mover brush is absent from
  static geometry, static BVH, static collision, portals, lightmap/SDF occluder
  bakes, and navmesh input.
- [ ] `prl-build` rejects a `kinematic_mover` whose `path` resolves to fewer
  than two waypoints (no zero-length path), with a diagnostic.
- [ ] The runtime loads that PRL and spawns one mover entity per record (host and
  connected client alike; the client binds by `mover_id` rather than re-spawning)
  with `Transform` and `KinematicMover`, and drives it deterministically along
  the waypoint path, honoring `once` (stops at the final waypoint) and
  `ping_pong` (reverses at each endpoint).
- [ ] The mover renders with its authored material/texture and interpolates
  between fixed ticks via the existing interpolated-transform accessor;
  perceived smoothness is dev-map QA.
- [ ] A player can stand on a moving linear platform/elevator for at least 10
  round trips in the dev map without visible jitter, falling through, accumulating
  vertical drift, or sliding off while providing no movement input — checked in
  the deterministic sim (Y within ε of the surface, XZ within the platform
  footprint across the trip); "visible jitter" is dev-map QA.
- [ ] Player collision against the mover works from top and sides; a wall-like
  side contact slides or blocks according to the existing movement substrate.
- [ ] A mover advancing into a stationary player displaces the player out of
  penetration — no tunneling, no persistent overlap — verified in the
  deterministic sim; this plan's dev maps author no pinch points, and an
  unresolved pinch logs in dev builds.
- [ ] Leaving a moving platform applies the plan's release-velocity policy
  consistently in single-player and connected-client replay.
- [ ] Re-simulating a recorded ride from a mid-ride state against recorded
  mover poses reproduces the live trajectory exactly — the deterministic-sim
  replay proof, no networking involved.
- [ ] In the deterministic net harness at the E15 profile —
  `LinkConfig { delay: 45, jitter: 60, loss_probability: 0.05 }` plus a fixed
  harness `seed`, ~45..105 ms one-way, 5% loss — the client's predicted mover
  pose tracks the host's within a
  small bounded tolerance at every tick (the same phase-seeded deterministic
  driver, re-anchored each snapshot — no interpolation lag), the mover
  reconciler's per-tick correction stays within that tolerance and does not
  accumulate, and a local player riding it reconciles in place without persistent
  correction drift.
- [ ] The player's replicated ground state distinguishes `Airborne` / `World` /
  `Mover(mover_id)`; a client riding a mover carries via the authoritative
  `Mover` reference and reconciles it, not via a locally-guessed base.
- [ ] No new `unsafe` is introduced (review/grep gate).
- [ ] No non-renderer module imports `wgpu` or creates GPU resources
  (review/grep gate; crate boundaries enforce it today).
- [ ] The mover draw path requires no new adapter feature and requests no
  additional bind-group slot (`max_bind_groups` stays 8).
- [ ] With no active movers the combined query layer preserves static-only
  movement behavior: existing movement and substrate tests pass with assertions
  unchanged (call sites may thread the new query parameter).
- [ ] Existing static maps with no movers load and render unchanged.

## Tasks

Tasks 1-2 are the substrate proof: the deterministic driver, the combined
collision query, carry, push, and replay land in-memory — no PRL, no FGD, no
renderer, no wire — with waypoints and mover geometry constructed in tests.
They produce production code the later tasks extend, not a throwaway spike.
Format, renderer, and networking attach only after the proof holds (see
**Sequencing**).

### Task 1: Component, deterministic driver, and moving-collider query layer

Add `KinematicMoverComponent` in `crates/entities/src/components/`, and
`ComponentKind::KinematicMover = 13` in `crates/entities/src/registry.rs`
(after `Brain = 12`); update `ComponentKind::COUNT`, `ComponentValue`, registry
storage, and serde. Exhaustive matches gain arms — compiler-enforced:
`component_kind_name` and the `ComponentValue` JS/Lua conversion impls
(`crates/entities/src/ffi.rs`), `ComponentValue::kind()`
(`crates/entities/src/registry.rs`), and the engine-side `component_kind_discriminant`
map plus its drift test (`crates/postretro/src/netcode/mod.rs`); none of these
touch `crates/net`, and `component_kind_from_name` keeps its `_ => None` arm
(movers are not script-queryable this plan). The matching net-side constant
lands in Task 6. The component stores
{ compiled mover id, resolved waypoint positions, `speed_mps`, `wait_ms`, mode,
segment index, direction sign, `segment_elapsed_ms`, `wait_remaining_ms`,
current linear velocity, `started`, `completed` } — the wire payload mirrors
the phase fields; the static path data (positions, speed, wait) is seeded at
construction and never replicated.

Add a fixed-tick mover system that evaluates deterministic linear motion using
tick `dt`, path segment length, speed, waits, and mode. The system must run
after the transform snapshot stage (`snapshot_transforms`) and before
player-movement collision in the game-logic stage. Modes: `once` stops at the
final waypoint; `ping_pong` reverses at each endpoint. Both are required.

The driver is a pure function of the component's {phase, static path} plus
`dt`: it runs
identically on the host (authoritative) and each client (predicted), so given
the same phase seed both reproduce the same path and reconciliation is exact.
Keep it free of wall-clock, RNG, and host-only state so client prediction cannot
diverge. `wait_ms` pauses at path endpoints only — the reversal point for
`ping_pong`, the final waypoint for `once` — not at intermediate waypoints;
`wait_remaining_ms` counts that pause down.

The mover system owns and publishes each tick's mover kinematic state (transform,
linear velocity, tick delta) into an engine-owned side-table keyed by `mover_id`.
The query layer below and Task 6's history buffer are downstream consumers of
this side-table; creating it here keeps the write before its readers.

Build local-space parry trimeshes for mover colliders and add a query layer that
returns the nearest hit across static world and active movers — hit normal and
TOI, source kind, mover id, mover linear velocity and tick delta, contact
surface classification. It is hand-rolled by design (see **Collision and
Movement Carry**); keep the API tiny. The layer reads the side-table and
transforms each mover's local trimesh by its current pose, taken from a pose
source — the live side-table by default, but swappable so Task 2's replay can
feed a historical pose. In this task mover trimeshes come from test-constructed
geometry; Task 4 later feeds PRL geometry through the same path.

Tests, all in-memory: driver determinism (same seed, same trajectory; `once`
stops at the final waypoint; `ping_pong` reverses; endpoint waits honored) and
aggregator correctness — nearest-hit ordering across static and mover sources,
TOI/normal/source-id fields, and exact static-only degeneracy: with no active
movers the layer reproduces direct `cast_capsule`/`cast_ray` results.

### Task 2: Ground reference, carry, push, and replay widening

Adapt `movement/substrate.rs` to the Task 1 query layer without losing existing
static-world behavior: with no movers active, existing movement tests must pass
unchanged.

Implement linear carry on a generalized ground reference:

- widen the player's grounded state to a ground reference — name the enum
  `GroundRef`, variants `Airborne` / `World` / `Mover(mover_id)`, where
  `mover_id` is a plain `u32` (the compile-time PRL key) so the reference stays
  foundation-local and never names `NetworkId` (which lives in `postretro-net`)
  — on `PlayerMovementComponent` (foundation-resident:
  `crates/foundation/src/movement/player_movement.rs`);
- update every reader of the *player* `is_grounded` bool to the widened form,
  but leave the unrelated AI `AgentComponent.is_grounded` alone, and keep the
  movement-scope `grounded` scripting primitive projecting a `bool`
  (`ground != Airborne`) so the primitive surface does not change. Removing the
  bool makes reader coverage compiler-enforced; most readers just map to
  `grounded == ground != Airborne`, but a few need judgment — `view_feel` (the
  head-bob grounded flag), `movement_state_to_wire` (the wire merge), the
  wire-to-component merges (`merge_wire_into_movement_state`, the reconcile
  apply — the wire is still a bool until Task 6, so they map it to
  `World`/`Airborne`), and `crates/scripting-core`'s movement-state refresh —
  and `substrate.rs` is where the value is *written*, now distinguishing mover
  vs world floor from the new query layer. The widened field is named
  `ground: GroundRef` on `PlayerMovementComponent` (matching the wire `ground`,
  so serde/round-trip line up). At the wire boundary, `movement_state_to_wire`
  keeps emitting the existing `is_grounded: bool` as `ground != Airborne` until
  Task 6 widens the wire field — this task must not emit the widened form
  against a field that does not exist yet;
- detect grounded contact on a mover surface and set the reference to
  `Mover(mover_id)`;
- while the reference is `Mover(mover_id)`, resolve it to the local mover and
  apply that mover's tick delta to the player;
- on transition off a `Mover` reference, preserve player-controlled velocity and
  add the mover velocity once; never add angular velocity (rotation is out of
  scope). This release carry is engine-internal movement-substrate logic, not a
  `movement.md` §6 declarative carry-rule;
- handle platform reversal and endpoint waits without jitter.

Implement the displace-only push from **Collision and Movement Carry** as the
first step of the player-movement stage in `substrate.rs`: a mover overlapping
the player's capsule displaces the player out of penetration along the contact
normal before player motion integrates. An unresolved pinch (displacement
blocked by static geometry) logs in dev builds; resolving it is E17-D.

Widen the pawn's replay path: `replay` — today movement-only, reading a static
`&CollisionWorld` — must widen so a replay tick can read each mover's pose at
that tick, driven through Task 1's pose source. The widening reaches the
collide-and-slide core (`movement::tick` / `integrate_collision`), so the
signature change fans out past `predict_tick` and the reconcile caller to every
`movement::tick` call site — `sim::simulate_tick`, `sim::host_movement`,
`netcode::client_predict_tick`, `client_receive_and_apply` — each passing the
live pose source, which with no movers loaded degenerates to static-only. In
this task historical poses come from a test-recorded ring; Task 6 later feeds
authoritative snapshot samples through the same seam.

Add deterministic-sim tests for static-only behavior, moving top contact, moving
side contact, endpoint wait, reversal, release velocity, ground-reference
transitions (`World` <-> `Mover` <-> `Airborne`), a >=10-round-trip
standing-carry test asserting Y stays within ε of the surface and XZ within the
platform footprint (AC 6's determinism check), push displacement (a mover
advancing into a stationary player: no tunneling, no persistent overlap), and a
ride replay: re-simulating from a mid-ride state against recorded mover poses
reproduces the live trajectory exactly.

This task completes the substrate proof. Check the stop conditions in
**Sequencing** before any Phase 3 work opens.

### Task 3: PRL format, FGD, and compiler extraction

Add `kinematic_geometry` to `postretro-level-format` with
`SectionId::KinematicGeometry = 43` — the next free id after
`ShadowmaskAtlas = 42` (`SectionId` is `#[repr(u32)]`). Also extend the
hand-written `SectionId::from_u32` match: the loader reads sections by explicit
`SectionId` constant, so the new variant is the load-bearing piece, but
`from_u32` feeds the format round-trip tests and must not lag the enum.
Add serialization tests.

Section shape:

```text
KinematicGeometrySection {
  version: u16 = 1,
  movers: Vec<KinematicMoverRecord>,
  waypoints: Vec<KinematicWaypointRecord>,
}

KinematicMoverRecord {
  mover_id: u32,
  name: String,
  tags: Vec<String>,
  origin: [f32; 3],
  path: String,
  speed: f32,
  wait_ms: f32,
  move_mode: u8,          // once=0, ping_pong=1
  start_on_spawn: bool,
  vertices: Vec<geometry::Vertex>,
  indices: Vec<u32>,
  face_meta: Vec<geometry::FaceMeta>,
}

KinematicWaypointRecord {
  name: String,
  next: String,           // empty string means no next waypoint
  origin: [f32; 3],
}
```

Mirror the existing `GeometrySection` vertex and face-meta encoding so material
lookup keeps using `TextureNames`. Use existing string encoding patterns from
`MapEntitySection`. Encoding pins (mirroring those sections): little-endian
throughout; each list `u32`-count-prefixed (empty = `u32(0)`); each `String`
`u32`-length + UTF-8 (empty = `u32(0)`, absent `next` = empty string);
`start_on_spawn` a single `0`/`1` byte; `move_mode` a `u8`.

Store `vertices` **origin-relative** — subtract the mover origin, which is the
first (`path`) waypoint; the compiler warns if the authored brush position
differs. At runtime the mover's `Transform.position` (walking the waypoint
chain) is applied to these local verts, and Task 5 draws them under that
`Transform`; world-space storage would double-offset every mover.

Compiler work:

- add FGD definitions;
- collect `kinematic_mover` brush entities in `parse.rs` before the brush-entity
  skip path;
- project each mover brush to textured geometry: the world path's `brush_hulls` /
  `face_vertices` / `face_indices` primitives are directly reusable, but
  texture/side assignment is currently inline in the monolithic world per-brush
  loop — factor that side/texture projection out (or duplicate it) for the mover
  set. The `fog_volume` precedent only skips the brush and computes an AABB, so
  mirror the world-geometry projection, not that skip;
- emit mover verts with `lightmap_uv` / `lightmap_layer` zeroed (movers skip the
  lightmap bake);
- source `KinematicMoverRecord.tags` from the `_tags` KVP;
- collect `kinematic_waypoint` point entities from the generic entity stream or
  a dedicated route, but do not spawn them as runtime generic entities;
- validate finite positive speed, finite non-negative waits, known mode, and
  a `path` that resolves to at least two waypoints (>=1 segment);
- emit warnings for orphan waypoints;
- pack the new section in `pack.rs`;
- emit the `KinematicGeometry` section only when the map has movers;
- a `kinematic_mover` is a brush *entity*, so its brushes are naturally absent from
  `world_brush_ids` / `brush_volumes` (they live in the entity-brush set), exactly
  like `fog_volume` — nothing is actively excluded. `brush_volumes` is the single
  source that world geometry, BVH, collision, lightmap/SDF, portals, and navmesh
  all derive from, so that natural absence covers all six. Add a regression test
  asserting the mover brush is absent from `world_brush_ids` / `brush_volumes` and
  from the packed static `GeometrySection`.

### Task 4: Runtime loading and spawn

Load section 43 (`KinematicGeometry`) into `LevelWorld` — the struct lives in
`crates/level-loader/src/prl.rs`; the section-read/population path is
`crates/level-loader/src/prl_loader.rs` (an absent or empty section means no
movers; mover-less maps load unchanged).

At level load, spawn one entity per mover record with:

- `Transform` at the record origin;
- `KinematicMoverComponent` seeded from the path — resolve the name-linked
  waypoint chain (`path` -> `next` -> ...) into world-space positions at load,
  carried on the component with `speed_mps`/`wait_ms`;
- seed the `started` flag from `start_on_spawn`;
- copy the record's `tags` onto the spawned entity's tag storage (movers
  bypass classname dispatch, which normally copies `_tags`).

Both host and client spawn the mover entity from the PRL record at load — the
client needs it for geometry and local prediction. Do **not** register movers
in `ReplicableSet` here: registration is Task 6, landing with the client-apply
bind, so a mover is never on the wire before the client can route it (a
`Transform` baseline arriving without the bind would materialize a duplicate
entity).

Feed the loaded mover collision geometry through Task 1's local-space trimesh
path.

This task also owns the runtime wiring that makes loaded movers live: invoke
Task 1's fixed-tick mover system from the game-logic tick (`simulate_tick` —
after `snapshot_transforms`, before player movement), and point the live
player-movement stage's combined query at the loaded mover set via Task 1's
pose source. Call-site wiring only in `main.rs`; logic in focused modules. The
Task 1 driver itself is consumed unchanged — this task adds wiring, not motion
logic. The runtime-drive AC and tick-to-tick mover motion depend on this
wiring.

### Task 5: Renderer-owned kinematic brush draw path

Add renderer-owned GPU resources and a draw path for kinematic brush payloads
in the renderer crate (`crates/renderer/src/render/`). The game/runtime side
(`crates/postretro`, which has no wgpu dependency) passes plain CPU records and
per-frame draw instances only; it never touches WGPU. Mover geometry and the spawned mover entities come
from Task 4's `LevelWorld` load of section 43; Task 5 consumes that carrier
read-only and defines no loader of its own (hence it sequences after Task 4).

Requirements:

- upload kinematic mover local vertices/indices/material ranges at level load;
- per frame, collect visible mover instances using current/interpolated
  transforms;
- draw with the authored material textures;
- keep movers out of the static world BVH/indirect buffer;
- avoid new required adapter features; request no additional bind-group slot
  (`max_bind_groups` stays 8);
- movers are stage-0-snapshotted registry entities driven locally (host:
  authoritative; client: predicted), so their fixed-tick transforms come from
  the driver, not the remote-interpolation buffer; draw them via the existing
  interpolated-transform accessor for between-tick render smoothing;
- the renderer's only two inputs are mover geometry (uploaded from the PRL
  section at load) and per-frame interpolated transforms from the
  snapshotted mover entities;
- light movers via the dynamic-object lighting model (baked indirect/SH +
  dynamic direct) -- material-only/unlit is not sufficient;
- mover verts already carry zeroed `lightmap_uv` / `lightmap_layer` from Task 3
  (movers skip the bake) — Task 5 consumes them and does not write verts;

First-slice culling may be conservative: visible if the mover origin or AABB is
inside the camera-visible leaf or a nearby visible leaf. It may draw a few
extra movers; it must not disappear while the player can see or stand on it.

### Task 6: Network payload, client apply, and replay harness

Extend `postretro-net` and `postretro` replication for `KinematicMoverState`
and the widened `WirePlayerMovementState` ground reference. Bump
`SNAPSHOT_VERSION` and the transport wire-version/`protocol_id` gate (both a new
wire type and a changed existing struct alter the bitcode layout -- see
networking.md's two-gate handshake); update raw payload validation, finite
checks, raw-from-typed conversion, baseline/delta tests, and engine/net
discriminant guards. This task consumes the `GroundRef` shape defined in Task 2.
`WireKinematicMoverState` carries { `mover_id: u32`, `segment_index: u16`,
`direction: i8` (-1/1), `mode: u8` (once=0/ping_pong=1),
`segment_elapsed_ms: f32`, `wait_remaining_ms: f32`, `started: bool`,
`completed: bool`, `velocity: [f32; 3]` } — this exact field set and order is
the bitcode layout.

Register each load-spawned mover in the host's `ReplicableSet` (engine-side:
`crates/postretro/src/netcode/replication.rs`, not `crates/net`) — registration
lands here, with the client bind, so the two sides ship together and a mover is
never on the wire without a client route. Host snapshot production collects
`Transform` then `KinematicMoverState` for
registered mover entities. On each client, apply (`ClientReplication`'s
snapshot-apply path in `crates/postretro/src/netcode/client.rs`) **binds** the
incoming mover
`NetworkId` to the client's load-spawned local mover **by `mover_id`** (the wire
state carries `mover_id`) — it does **not** materialize a fresh entity from the
baseline, which would double-spawn (an AC requires exactly one mover entity). It
then seeds that mover's predictive driver from the replicated phase and
reconciles it in place. Route by payload: a baseline carrying
`KinematicMoverState` binds by `mover_id` and never materializes; movers carry
no `entity_class` (they bypass classname dispatch), so the `KinematicMoverState`
payload is the discriminator. One seam here is net-new, not reuse of the pawn's
path: the mover needs its **own** predictor/reconciler instance — phase-seeded
and input-free, distinct from the pawn's command-ring `ClientPrediction`. The
widened `replay` it relies on already landed in Task 2; this task feeds it
authoritative data.

Client apply writes each tick's authoritative mover sample into an engine-owned
mover-history buffer keyed by `mover_id`; the widened replay reads those samples
through Task 1's pose source, so the pawn replays against the same platform pose
the host used. When a replay tick has no authoritative mover sample
(loss/jitter), fill it by advancing the deterministic driver from the nearest
authoritative phase — the same prediction, not an interpolation. This history
buffer (past ticks, for replay) is a **distinct** structure from Task 1's live
per-tick mover-state side-table (current tick only). Also update the
`populated`-count check in `RawComponentPayload::validate` for the new slot,
and have the mover reconciler surface a per-tick correction metric (analogous to
the pawn's `CorrectionClass`) so the harness can assert the correction stays
within a small bounded tolerance and does not accumulate.

Extend the in-memory prediction/reconciliation harness with a moving-platform
scenario at the E15 latency/loss profile: assert the client-predicted platform
tracks the host within a small bounded tolerance (no interpolation lag) and that
a rider reconciles without steady-state drift or accumulating correction. The
scenario also has the rider leave the platform mid-ride and asserts the
release-velocity outcome matches the single-player policy (the connected-client
half of the release AC).

### Task 7: Demo map, diagnostics, and documentation

Add a small dev map or extend an existing dev map with one simple elevator or
linear platform. Add concise diagnostics:

- optional debug-line AABB/path overlay for movers;
- log one summary at level load: mover count, waypoint count, vertex/index
  totals.

Update context docs only where implementation changed the durable contract:

- `context/lib/build_pipeline.md` for the PRL/FGD/compiler path;
- `context/lib/entity_model.md` for `KinematicMover` and the new fixed-tick
  mover stage in the §5 update-order table (after `snapshot_transforms`, before
  player-movement collision);
- `context/lib/movement.md` for the generalized ground reference, moving-base
  carry, and displace-only push (crush is E17-D) — and retract §7's "Networked
  movement (prediction, rollback)" non-goal, now that pawn (E15) and mover
  prediction exist;
- `context/lib/rendering_pipeline.md` for the dynamic kinematic draw path;
- `context/lib/networking.md` for the mover payload, client mover prediction, and
  the widened ground reference.

## Sequencing

Phase 1 is sequential: Task 1. The driver, the mover-state side-table, and the
query aggregator, proven in-memory.

Phase 2 is sequential: Task 2. Carry, push, and replay against the proven
driver. This completes the substrate proof — the plan's highest-risk
integration, crossing the kinematic driver, collision-query composition, and
prediction/replay at once. Hold Phase 3 until its suites are green and no stop
condition has fired.

Phase 3 is sequential: Task 3. It establishes the PRL/FGD format the loader
consumes.

Phase 4 is sequential: Task 4. Load and spawn — the first time the proven
substrate runs against compiled content.

Phase 5 runs Task 5 and Task 6 in parallel — renderer versus net/netcode,
disjoint modules. Both add call-site wiring to `main.rs`; keep those additions
small and mergeable.

Phase 6 is final integration: Task 7, plus any manual QA.

Do not split this plan into a wave with the trigger/event spec. The first
platform touches too many substrate boundaries; land it alone, then draft the
trigger/event plan against the actual mover API.

### Stop conditions

Any of these pauses the plan — surface it and redesign; do not layer fixes
forward:

- static-only movement behavior regresses at any phase (the Task 1/2 degeneracy
  suites or any existing movement test);
- ride replay drifts, or per-tick reconciliation corrections accumulate, beyond
  tolerance (Task 2 proof or Task 6 harness);
- a `kinematic_mover` brush appears in any static input — world geometry, BVH,
  collision, lightmap/SDF occluders, portals, or navmesh;
- a tick ends with the player in unresolved penetration outside the documented
  pinch deferral.

## Rough Sketch

- `crates/level-format/src/kinematic_geometry.rs`: section structs, encoding,
  validation helpers.
- `crates/level-format/src/lib.rs`: `SectionId::KinematicGeometry = 43` (+ the
  hand-written `from_u32` match arm).
- `sdk/TrenchBroom/postretro.fgd`: `kinematic_mover`, `kinematic_waypoint`.
- `crates/level-compiler/src/parse.rs`: collect `kinematic_mover` brush entities and
  `kinematic_waypoint` points instead of skipping them.
- `crates/level-compiler/src/map_data.rs`: store kinematic mover source data.
- `crates/level-compiler/src/pack.rs`: emit the new section.
- `crates/level-loader/src/prl.rs`: load section 43 (`KinematicGeometry`) into `LevelWorld`.
- `crates/entities/src/components/`: new `KinematicMoverComponent` (engine
  components live here, per scripting.md §12).
- `crates/entities/src/registry.rs`: `ComponentKind` / `COUNT` / `ComponentValue` /
  registry-storage wiring.
- `crates/postretro/src/sim/` or a new game-logic system: fixed-tick mover
  evaluation before player movement.
- `crates/postretro/src/collision/`: moving-collider query layer beside
  `CollisionWorld`.
- `crates/foundation/src/movement/player_movement.rs`: widen `is_grounded` to
  `GroundRef` (`Airborne` / `World` / `Mover(u32 mover_id)`) on
  `PlayerMovementComponent`; update its player-side readers only.
- `crates/foundation/src/movement/scope.rs`: keep the `grounded` IR primitive a
  `bool` (`ground != Airborne`) — no primitive-surface change.
- `crates/postretro/src/movement/substrate.rs`: consume the combined query,
  `Mover`-referenced carry, and mover push.
- `crates/renderer/src/render/`: renderer-owned mover buffers/draws (the
  wgpu-owning crate; `crates/postretro` has no wgpu dependency).
- `crates/net/src/wire.rs`, `crates/net/src/replication.rs`,
  `crates/postretro/src/netcode/`: mover payload + widened ground reference,
  validation, a new phase-seeded mover predictor/reconciler (separate from the
  pawn's command-ring `ClientPrediction`), mover-history buffer, widened `replay`
  that reads it, drift guards, harness (`ReplicableSet` registration is
  engine-side in `crates/postretro/src/netcode/replication.rs`, not in
  `crates/net`).

Oversized-file warning: `main.rs` is already large. Add call-site wiring only
there; new logic belongs in focused modules.

## Boundary Inventory

| Name | Rust | PRL / wire / serde | TypeScript / Luau | FGD |
| --- | --- | --- | --- | --- |
| mover entity | `KinematicMoverComponent`, `ComponentKind::KinematicMover = 13` | PRL `KinematicMoverRecord`; net `KinematicMoverState` kind 13; serde `kind = "kinematic_mover"` | Not directly queryable in this plan; becomes a `world.query` component kind in E17-B | `kinematic_mover` |
| waypoint | `KinematicWaypointRecord` load data | PRL `KinematicWaypointRecord` | None | `kinematic_waypoint` |
| mover id | `KinematicMoverRecord.mover_id`, component compiled-mover-id | PRL `mover_id: u32`; wire `mover_id: u32` (stable cross-peer key) | Future `world.query` handle | n/a |
| mover name | `name: String` | `name` | Future `world.query` handle / command target | `name` |
| path (first waypoint ref) | `path: String` | `path` | None | `path` |
| next waypoint | `next: String` | `next` (empty means absent) | None | `next` |
| mode | `KinematicMoveMode` | PRL/FGD `move_mode`; wire `mode` (`once=0`, `ping_pong=1`) | Future command surface uses strings | `move_mode` |
| start flag | `start_on_spawn: bool` | `start_on_spawn` | None | `start_on_spawn` |
| speed | `KinematicMoverComponent.speed_mps: f32` | PRL `speed` finite positive (static; not replicated) | Future command surface may read only | `speed` |
| wait | `KinematicMoverComponent.wait_ms: f32` | PRL `wait_ms` finite non-negative (static); runtime countdown replicated as wire `wait_remaining_ms` | Future command surface may read only | `wait_ms` |
| tags | `Vec<String>` | `_tags` split on whitespace | Future `world.query`/commands | `_tags` |
| ground reference | `GroundRef` on `PlayerMovementComponent` (foundation): `Airborne` / `World` / `Mover(u32 mover_id)` — foundation-local, no `NetworkId` | `WirePlayerMovementState.is_grounded: bool` widened to `ground` (bitcode) | n/a | n/a |

## Wire Format

Two binary surfaces. Both mirror existing siblings — no new serialization
mechanism. PRL is hand-rolled little-endian (`to_le_bytes` / `from_le_bytes`, no
`bincode`/`byteorder`); the net wire is `bitcode`-derived. Do not mix the two.

### PRL `KinematicGeometrySection` (`SectionId::KinematicGeometry = 43`)

Mirror `GeometrySection` and `MapEntitySection` encoding:

- Little-endian throughout. Recorded in the PRL table-of-contents like any
  section (`SectionEntry`: `section_id: u32`, `offset: u64`, `size: u64`,
  `version: u16`). `version` starts at 1. Body is self-contained — no offsets
  into other sections.
- Every list is a `u32` count immediately before its entries (`movers`,
  `waypoints`, per-mover `vertices` / `indices` / `face_meta`, `tags`). Empty
  list = `u32(0)`, no trailing bytes.
- Every `String` (`name`, `path`, `next`, tag entries) is a `u32` byte-length
  prefix + raw UTF-8, no null terminator; decode validates UTF-8. Empty string =
  `u32(0)`. Absent `next` is the empty string, not a sentinel.
- `start_on_spawn` is a single byte `0`/`1` (mirror `AlphaLightsSection`).
  `move_mode` is a `u8` (`once=0`, `ping_pong=1`).
- `vertices` and `face_meta` reuse `geometry::Vertex` (36-byte stride) and
  `geometry::FaceMeta` unchanged, so `TextureNames` material lookup is identical
  to static geometry.

### Net `WireKinematicMoverState` (`COMPONENT_KIND_KINEMATIC_MOVER_STATE = 13`)

Bitcode owns the bit-level layout — specify the struct and its validation, not a
byte offset table:

- `WireKinematicMoverState` derives `bitcode::Encode` / `Decode`; fields are the
  block in **Networking**, in declaration order. Endianness is bitcode-internal.
- Framing follows `RawComponentPayload`: a `component_kind: u16` discriminant
  plus one `Option<T>` slot per component. Add a
  `kinematic_mover: Option<WireKinematicMoverState>` slot; exactly one slot is
  `Some` and must match `component_kind`, else `validate` rejects the payload
  (a validation error, not a decode error). Add the typed
  `ComponentPayload::KinematicMoverState` variant and its `.kind()` arm.
- Add an `all_finite` check (mirror `WireTransform::all_finite` /
  `WirePlayerMovementState::all_finite`): every `f32`
  (`segment_elapsed_ms`, `wait_remaining_ms`, `velocity`) must be finite.
  `direction ∈ {-1, 1}` and `mode ∈ {0, 1}`; out-of-range rejects before apply.
- Bump `SNAPSHOT_VERSION` 7 → 8. The engine/net discriminant-drift guard keeps
  `ComponentKind::KinematicMover` and `COMPONENT_KIND_KINEMATIC_MOVER_STATE`
  numeric-equal at 13 (verified free on the net side — existing kinds are
  `0` / `6` / `9`).

### Net `WirePlayerMovementState` ground reference

Widen the existing `is_grounded: bool` field to a `ground` reference encoding
`Airborne` / `World` / `Mover(mover_id)` — an enum tag plus a `u32` `mover_id`
for the `Mover` arm (the compile-time PRL key, stable across peers; no
`NetworkId` on the wire for this field). This mutates an existing `bitcode` wire
type's layout, so:

- bump `SNAPSHOT_VERSION` (7 → 8, shared with the mover payload) and the
  transport wire-version/`protocol_id` gate;
- update the `bitcode` encode/decode, `all_finite`/validation, raw↔typed
  conversion, baseline/delta, and drift/round-trip tests (the foundation
  `PlayerMovementComponent` mirrors the change on its `serde` derive);
- wire validation checks finiteness and a valid enum tag only; the unknown-
  `mover_id` rejection lives engine-side in `crate::netcode` client-apply, since
  the net crate is registry-blind;
- update every reader of the *player* grounded bool to the widened form
  (`grounded == ground != Airborne`); leave the unrelated AI
  `AgentComponent.is_grounded` untouched, and keep the movement-scope `grounded`
  scripting primitive projecting a `bool`.

## Future Scripting Seam

> **Non-normative.** Nothing in this section ships in this plan. It records the
> intended authoring arc so the foundation's choices do not foreclose it. The
> command API shape is settled in E17-B, not here.

Two authoring tiers, one substrate. **Basic movers are pure KVP authoring** —
place a `kinematic_mover` brush, wire a `kinematic_waypoint` chain, set
`speed`/`move_mode`, done. No script required; the FGD is the whole interface.
**Complex movers are scripted**, but only in the declare-not-drive sense the
engine already enforces (`scripting.md §1`): a script *selects* movers and binds
*closed-vocabulary commands* to level events. Rust still owns every tick — there
is no per-tick script control, by architecture.

The selection mechanism already exists and needs no new invention. `world.query`
selects entities by component kind plus author tag today (`world.query({
component: "light", tag: "t" })`, `postretro.fgd:132`). A future
`kinematicMover` component kind slots into that same path — this is the
"worldQuery to select the desired brushes" arc. The command vocabulary is E17-B:
closed verbs (`start`, `stop`, `reverse`, `goToPathNode`) crossing the FFI as
data, evaluated by Rust — the same shape as existing reaction primitives
(`setEmitterRate`, `setFogDensity`) and, for motion that depends on live state,
the Typed Command Buffer (`scripting.md §11`, whose first adopter is movement).

Illustrative only — the exact handle-vs-reaction binding is E17-B's call:

```ts
// FUTURE (E17-B). Not implemented in this slice. TypeScript shown; Luau is a
// behavioral twin. A level data script selects movers by component + tag and
// binds a closed-vocabulary command to a named event. Rust owns tick eval.
import { world, defineReaction } from "postretro";

const bridgeLifts = world.query({ component: "kinematicMover", tag: "bridge-lift" });

defineReaction("raiseBridge", () => {
  bridgeLifts.start();            // begin authored path motion
  bridgeLifts.goToPathNode("top");
});
```

What this foundation must keep open (each is already in scope above, called out
here so it is not optimized away):

- `kinematic_mover._tags` reaches the PRL record and the runtime component, so a
  future query can filter movers by author tag.
- `ComponentKind::KinematicMover` state is shaped to register as a
  `world.query` component kind later (see **Runtime Model**); no mover state
  hides where a `WorldQueryComponent` registration could not reach it.
- Tick evaluation stays deterministic and Rust-owned, so future commands remain
  declarative rather than becoming a live-VM escape hatch.

## Open Questions

None block this draft. Later E17 specs must resolve:

- trigger ownership and late-join semantics for co-op set pieces;
- angular carry/orientation policy for rotating platforms;
- whether doors ever participate in visibility/portal blocking;
- whether kinematic clusters justify a shared chunk primitive.
