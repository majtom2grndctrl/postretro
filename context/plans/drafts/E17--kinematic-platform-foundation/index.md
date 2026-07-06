# E17 - Kinematic Platform Foundation

> **Status:** draft.
>
> **Epic:** 17 - Kinematic Geometry and Moving Platforms.
>
> **Fits after:** E15 Phase 3/3.5 networking and movement prediction/reconciliation.

## Goal

Build the first deterministic moving-world slice: a map-authored brush platform
or elevator compiles into runtime-transformable geometry, moves along a linear
waypoint path, renders as dynamic world geometry, blocks and carries the player,
and replicates enough server-authored state for remote interpolation and local
replay.

This is the substrate plan. It proves one moving brush payload end-to-end before
the epic adds triggers, rotating carry, doors, dynamic portals, kinematic
clusters, or destruction.

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
  one-shot and ping-pong. Optional looping may ship if it falls out naturally.
- Existing fixed frame order only: input, game logic, audio, render, present.
- A renderer-owned draw path for kinematic brush payloads. All WGPU calls stay
  in `crates/postretro/src/render`.
- Collision queries that consider both static `CollisionWorld` geometry and
  active mover colliders.
- Player carry for linear movers: standing on a mover follows its linear delta;
  side/top collision works; leaving the mover preserves explicit movement intent
  plus the chosen carry-release velocity policy in this plan.
- Server-authoritative mover state in snapshots, with client apply feeding the
  existing interpolation/reconciliation machinery.
- A small dev map proving one platform or elevator in single-player and over the
  in-memory net harness.

### Out of scope

- Rotating platforms, angular carry, and player orientation changes.
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
- path waypoint indices;
- mode;
- segment index;
- direction sign;
- segment elapsed milliseconds;
- wait remaining milliseconds;
- current linear velocity;
- flags for started/completed/blocked.

The mover system runs in the fixed-tick game-logic phase after
`snapshot_transforms` and before player movement consumes collision for that
tick. It updates each mover's `Transform` and records previous/current deltas so
movement carry and renderer interpolation share the same tick state. Connected
clients do not author mover motion; they apply server state.

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

The first carry policy is linear only:

- a grounded player standing on a walkable mover surface inherits the mover's
  tick delta before or during collision integration so the capsule remains on
  the platform;
- horizontal player intent remains relative to world axes for this plan;
- when leaving a mover, preserve player-controlled velocity and add the mover
  velocity only when the previous tick had a grounded mover base. Do not add
  angular velocity because rotation is out of scope.

Keep the custom-kinematic movement invariant: no rigid-body player, no Rapier
world, no per-tick script.

### Networking

Server-authoritative from the Runtime Model up. The mover is deterministic and
server-owned, so its state rides the same E15 snapshot / interpolation /
reconciliation spine every replicated entity uses — replication is the design
spine here, not a step bolted onto local motion. A mover registers in
`ReplicableSet` when spawned on the authoritative side.

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

All floats must be finite at validation. Invalid direction/mode values reject
the payload before apply.

Snapshots also carry `Transform` for the mover. Remote clients render movers
through the existing remote interpolation path. Local prediction/replay for a
player standing on a mover must use the authoritative mover history for replay,
not a client-authored divergent path.

## Acceptance Criteria

- [ ] `sdk/TrenchBroom/postretro.fgd` includes `kinematic_mover` and
  `kinematic_waypoint` with the keys above.
- [ ] `prl-build` compiles a map with one `kinematic_mover` brush and two waypoints
  into a PRL with a kinematic geometry section. The mover brush is absent from
  static geometry, static BVH, static collision, portals, lightmap/SDF occluder
  bakes, and navmesh input.
- [ ] The runtime loads that PRL, spawns one mover entity with `Transform` and
  `KinematicMover`, and drives it deterministically along the waypoint segment.
- [ ] The mover renders with its authored material/texture and interpolates
  smoothly between fixed ticks.
- [ ] A player can stand on a moving linear platform/elevator for at least 10
  round trips in the dev map without visible jitter, falling through, accumulating
  vertical drift, or sliding off while providing no movement input.
- [ ] Player collision against the mover works from top and sides; a wall-like
  side contact slides or blocks according to the existing movement substrate.
- [ ] Leaving a moving platform applies the plan's release-velocity policy
  consistently in single-player and connected-client replay.
- [ ] In the deterministic net harness at the E15 profile (150 ms RTT, jitter,
  5% loss), a remote client sees the mover smoothly interpolated and a local
  player riding it reconciles without persistent correction drift.
- [ ] No new `unsafe` is introduced.
- [ ] No non-renderer module imports `wgpu` or creates GPU resources.
- [ ] Existing static maps with no movers load and render unchanged.

## Tasks

### Task 1: PRL format, FGD, and compiler extraction

Add `kinematic_geometry` to `postretro-level-format` with
`SectionId::KinematicGeometry = 43` — the next free id after
`ShadowmaskAtlas = 42` (`SectionId` is `#[repr(u32)]`). Add serialization tests.

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
  pivot: [f32; 3],
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
`MapEntitySection`.

Compiler work:

- add FGD definitions;
- collect `kinematic_mover` brush entities in `parse.rs` before the brush-entity
  skip path;
- collect `kinematic_waypoint` point entities from the generic entity stream or
  a dedicated route, but do not spawn them as runtime generic entities;
- validate finite positive speed, finite non-negative waits, known mode, and
  resolvable `path`;
- emit warnings for orphan waypoints;
- pack the new section in `pack.rs`;
- add regression tests proving mover brushes do not enter static geometry.

### Task 2: Runtime loading, component, and deterministic driver

Load section 43 (`KinematicGeometry`) in `prl.rs` into `LevelWorld`. Add
`KinematicMoverComponent` and `ComponentKind::KinematicMover = 13`, update
`ComponentKind::COUNT`, `ComponentValue`, registry storage, serde, and the
netcode discriminant drift tests.

At level load, spawn one entity per mover record with:

- `Transform` at the record origin/pivot;
- `KinematicMoverComponent` seeded from the path;
- authoritative registration in `ReplicableSet` on the server/host side.

Add a fixed-tick mover system that evaluates deterministic linear motion using
tick `dt`, path segment length, speed, waits, and mode. The system must run
before player movement collision in the game-logic stage.

### Task 3: Renderer-owned kinematic brush draw path

Add renderer-owned GPU resources and a draw path for kinematic brush payloads.
The game/runtime side passes plain CPU records and per-frame draw instances
only; it never touches WGPU.

Requirements:

- upload kinematic mover local vertices/indices/material ranges at level load;
- per frame, collect visible mover instances using current/interpolated
  transforms;
- draw with the authored material textures;
- keep movers out of the static world BVH/indirect buffer;
- avoid new required adapter features and preserve the current bind-group limit.

First-slice culling may be conservative: visible if the mover origin or AABB is
inside the camera-visible leaf or a nearby visible leaf. It may draw a few
extra movers; it must not disappear while the player can see or stand on it.

### Task 4: Moving-collider query layer and player carry

Build local-space parry trimeshes for mover colliders at load and query them at
runtime with the mover transform. Add a query layer that returns the nearest hit
across static world and active movers, then adapt `movement/substrate.rs` to use
that layer without losing existing static-world behavior.

Implement linear carry:

- detect grounded contact on a mover surface;
- record the mover base on `PlayerMovementComponent` or a small companion
  engine-owned field;
- apply the mover tick delta to the player while grounded on that base;
- apply the release-velocity policy when leaving the base;
- handle platform reversal and endpoint waits without jitter.

Add unit tests for static-only behavior, moving top contact, moving side
contact, endpoint wait, reversal, and release velocity.

### Task 5: Network payload, client apply, and replay harness

Extend `postretro-net` and `postretro` replication for
`KinematicMoverState`. Bump `SNAPSHOT_VERSION`, update raw payload validation,
finite checks, raw-from-typed conversion, baseline/delta tests, and engine/net
discriminant guards.

Host snapshot production collects `Transform` then `KinematicMoverState` for
registered mover entities. Client apply updates the mover component and
presentation transform. Local replay for a player standing on a mover uses the
authoritative mover samples for the replay ticks so corrections do not compound
from a mismatched platform pose.

Extend the in-memory prediction/reconciliation harness with a moving-platform
scenario at the E15 latency/loss profile.

### Task 6: Demo map, diagnostics, and documentation

Add a small dev map or extend an existing dev map with one simple elevator or
linear platform. Add concise diagnostics:

- optional debug-line AABB/path overlay for movers;
- log one summary at level load: mover count, waypoint count, vertex/index
  totals.

Update context docs only where implementation changed the durable contract:

- `context/lib/build_pipeline.md` for the PRL/FGD/compiler path;
- `context/lib/entity_model.md` for `KinematicMover`;
- `context/lib/movement.md` for moving-base carry;
- `context/lib/rendering_pipeline.md` for the dynamic kinematic draw path;
- `context/lib/networking.md` for the mover payload.

## Sequencing

Phase 1 is sequential: Task 1. It establishes the wire/storage format the rest
of the plan consumes.

Phase 2 can run as a small parallel pair after Task 1:

- Task 2 runtime/component/driver;
- Task 3 renderer draw path.

Phase 3 is sequential: Task 4. It consumes the runtime driver and collider
payloads, and it is the highest-risk movement integration step.

Phase 4 is sequential: Task 5. Networking should follow the movement semantics
so it replicates the final state needed for replay rather than a guessed shape.

Phase 5 is final integration: Task 6, plus any manual QA.

Do not split this plan into a wave with the trigger/event spec. The first
platform touches too many substrate boundaries; land it alone, then draft the
trigger/event plan against the actual mover API.

## Rough Sketch

- `crates/level-format/src/kinematic_geometry.rs`: section structs, encoding,
  validation helpers.
- `crates/level-format/src/lib.rs`: `SectionId::KinematicGeometry = 43`.
- `sdk/TrenchBroom/postretro.fgd`: `kinematic_mover`, `kinematic_waypoint`.
- `crates/level-compiler/src/parse.rs`: collect `kinematic_mover` brush entities and
  `kinematic_waypoint` points instead of skipping them.
- `crates/level-compiler/src/map_data.rs`: store kinematic mover source data.
- `crates/level-compiler/src/pack.rs`: emit the new section.
- `crates/postretro/src/prl.rs`: load section 43 (`KinematicGeometry`) into `LevelWorld`.
- `crates/postretro/src/scripting/components/`: new kinematic mover component.
- `crates/postretro/src/scripting/registry.rs`: component enum/value wiring.
- `crates/postretro/src/sim/` or a new game-logic system: fixed-tick mover
  evaluation before player movement.
- `crates/postretro/src/collision/`: moving-collider query layer beside
  `CollisionWorld`.
- `crates/postretro/src/movement/substrate.rs`: consume the combined query and
  moving-base carry.
- `crates/postretro/src/render/`: renderer-owned mover buffers/draws.
- `crates/net/src/wire.rs`, `crates/net/src/replication.rs`,
  `crates/postretro/src/netcode/`: mover payload, validation, apply, drift
  guards, harness.

Oversized-file warning: `main.rs` is already large. Add call-site wiring only
there; new logic belongs in focused modules.

## Boundary Inventory

| Name | Rust | PRL / wire / serde | TypeScript / Luau | FGD |
| --- | --- | --- | --- | --- |
| mover entity | `KinematicMoverComponent`, `ComponentKind::KinematicMover = 13` | PRL `KinematicMoverRecord`; net `KinematicMoverState` kind 13; serde `kind = "kinematic_mover"` | Not directly queryable in this plan; becomes a `world.query` component kind in E17-B | `kinematic_mover` |
| waypoint | `KinematicWaypointRecord` load data | PRL `KinematicWaypointRecord` | None | `kinematic_waypoint` |
| mover name | `name: String` | `name` | Future `world.query` handle / command target | `name` |
| path (first waypoint ref) | `path: String` | `path` | None | `path` |
| next waypoint | `next: String` | `next` (empty means absent) | None | `next` |
| mode | `KinematicMoveMode` | `move_mode` / wire `mode` (`once=0`, `ping_pong=1`) | Future command surface uses strings | `move_mode` |
| start flag | `start_on_spawn: bool` | `start_on_spawn` | None | `start_on_spawn` |
| speed | `speed_mps: f32` | `speed` finite positive | Future command surface may read only | `speed` |
| wait | `wait_ms: f32` | `wait_ms` finite non-negative | Future command surface may read only | `wait_ms` |
| tags | `Vec<String>` | `_tags` split on whitespace | Future `world.query`/commands | `_tags` |

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
  numeric-equal at 13; confirm 13 is free on the net side when implementing.

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
