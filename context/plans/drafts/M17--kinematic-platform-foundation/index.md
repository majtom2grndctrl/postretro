# M17 - Kinematic Platform Foundation

> **Status:** draft.
>
> **Milestone:** 17 - Kinematic Geometry and Moving Platforms.
>
> **Fits after:** M15 Phase 3/3.5 networking and movement prediction/reconciliation.

## Goal

Build the first deterministic moving-world slice: a map-authored brush platform
or elevator compiles into runtime-transformable geometry, moves along a linear
waypoint path, renders as dynamic world geometry, blocks and carries the player,
and replicates enough server-authored state for remote interpolation and local
replay.

This is the substrate plan. It proves one moving brush payload end-to-end before
the milestone adds triggers, rotating carry, doors, dynamic portals, kinematic
clusters, or destruction.

## Scope

### In scope

- A brush entity classname `func_mover` in the FGD and compiler.
- Point waypoint entities for a simple path: `kinematic_waypoint`.
- Compile-time extraction of `func_mover` brushes into a new PRL kinematic
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

Authors create a `func_mover` brush entity and at least two
`kinematic_waypoint` point entities. The mover's `path_start` points at the
first waypoint's `targetname`; each waypoint may point at the next waypoint via
`next`.

First-slice FGD keys:

- `func_mover.targetname`: stable mover name.
- `func_mover.path_start`: first waypoint targetname.
- `func_mover.speed`: meters per second, finite and positive.
- `func_mover.wait_ms`: optional endpoint wait in milliseconds, finite and
  non-negative.
- `func_mover.move_mode`: `once` or `ping_pong`.
- `func_mover.start_on_spawn`: boolean, defaults true.
- `func_mover._tags`: space-delimited script/query tags, matching existing FGD
  convention.
- `kinematic_waypoint.targetname`: stable waypoint name.
- `kinematic_waypoint.next`: optional next waypoint targetname.

No trigger key ships in this plan. That keeps the first platform independent of
M18 trigger ownership and lets the platform substrate land before level-event
semantics.

### Runtime Model

Add an engine-owned mover component after `ComponentKind::Brain`:
`ComponentKind::KinematicMover = 13`. The component is not directly queryable
by scripts in this plan.

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
precedent. This plan adds that dedicated subsystem for `func_mover`.

`func_mover` brushes must be removed from the static world path:

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

The server owns mover state. A mover must be registered in `ReplicableSet` when
spawned on the authoritative side.

Add a wire payload for mover state in `postretro-net`:

- `COMPONENT_KIND_KINEMATIC_MOVER_STATE = 13`, numeric-equal to
  `ComponentKind::KinematicMover`.
- `RawComponentPayload` gains a `kinematic_mover` option slot.
- `ComponentPayload` gains `KinematicMoverState`.
- Bump `SNAPSHOT_VERSION` and the protocol gate.

Wire mover fields:

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

- [ ] `sdk/TrenchBroom/postretro.fgd` includes `func_mover` and
  `kinematic_waypoint` with the keys above.
- [ ] `prl-build` compiles a map with one `func_mover` brush and two waypoints
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
- [ ] In the deterministic net harness at the M15 profile (150 ms RTT, jitter,
  5% loss), a remote client sees the mover smoothly interpolated and a local
  player riding it reconciles without persistent correction drift.
- [ ] No new `unsafe` is introduced.
- [ ] No non-renderer module imports `wgpu` or creates GPU resources.
- [ ] Existing static maps with no movers load and render unchanged.

## Tasks

### Task 1: PRL format, FGD, and compiler extraction

Add `kinematic_geometry` to `postretro-level-format` with section id 37, the
next free id after `NavMesh = 36`. Add serialization tests.

Section shape:

```text
KinematicGeometrySection {
  version: u16 = 1,
  movers: Vec<KinematicMoverRecord>,
  waypoints: Vec<KinematicWaypointRecord>,
}

KinematicMoverRecord {
  mover_id: u32,
  targetname: String,
  tags: Vec<String>,
  origin: [f32; 3],
  pivot: [f32; 3],
  path_start: String,
  speed: f32,
  wait_ms: f32,
  move_mode: u8,          // once=0, ping_pong=1
  start_on_spawn: bool,
  vertices: Vec<geometry::Vertex>,
  indices: Vec<u32>,
  face_meta: Vec<geometry::FaceMeta>,
}

KinematicWaypointRecord {
  targetname: String,
  next: String,           // empty string means no next waypoint
  origin: [f32; 3],
}
```

Mirror the existing `GeometrySection` vertex and face-meta encoding so material
lookup keeps using `TextureNames`. Use existing string encoding patterns from
`MapEntitySection`.

Compiler work:

- add FGD definitions;
- collect `func_mover` brush entities in `parse.rs` before the brush-entity
  skip path;
- collect `kinematic_waypoint` point entities from the generic entity stream or
  a dedicated route, but do not spawn them as runtime generic entities;
- validate finite positive speed, finite non-negative waits, known mode, and
  resolvable `path_start`;
- emit warnings for orphan waypoints;
- pack the new section in `pack.rs`;
- add regression tests proving mover brushes do not enter static geometry.

### Task 2: Runtime loading, component, and deterministic driver

Load section 37 in `prl.rs` into `LevelWorld`. Add
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
scenario at the M15 latency/loss profile.

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
- `crates/level-format/src/lib.rs`: `SectionId::KinematicGeometry = 37`.
- `sdk/TrenchBroom/postretro.fgd`: `func_mover`, `kinematic_waypoint`.
- `crates/level-compiler/src/parse.rs`: collect `func_mover` brush entities and
  `kinematic_waypoint` points instead of skipping them.
- `crates/level-compiler/src/map_data.rs`: store kinematic mover source data.
- `crates/level-compiler/src/pack.rs`: emit the new section.
- `crates/postretro/src/prl.rs`: load section 37 into `LevelWorld`.
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
| mover entity | `KinematicMoverComponent`, `ComponentKind::KinematicMover = 13` | PRL `KinematicMoverRecord`; net `KinematicMoverState` kind 13; serde `kind = "kinematic_mover"` | Not directly queryable in this plan | `func_mover` |
| waypoint | `KinematicWaypointRecord` load data | PRL `KinematicWaypointRecord` | None | `kinematic_waypoint` |
| mover name | `targetname: String` | `targetname` | Future command target string | `targetname` |
| path start | `path_start: String` | `path_start` | None | `path_start` |
| next waypoint | `next: String` | `next` (empty means absent) | None | `next` |
| mode | `KinematicMoveMode` | `move_mode` / wire `mode` (`once=0`, `ping_pong=1`) | Future command surface uses strings | `move_mode` |
| start flag | `start_on_spawn: bool` | `start_on_spawn` | None | `start_on_spawn` |
| speed | `speed_mps: f32` | `speed` finite positive | Future command surface may read only | `speed` |
| wait | `wait_ms: f32` | `wait_ms` finite non-negative | Future command surface may read only | `wait_ms` |
| tags | `Vec<String>` | `_tags` split on whitespace | Future `world.query`/commands | `_tags` |

## Open Questions

None block this draft. Later M17 specs must resolve:

- trigger ownership and late-join semantics for co-op set pieces;
- angular carry/orientation policy for rotating platforms;
- whether doors ever participate in visibility/portal blocking;
- whether kinematic clusters justify a shared chunk primitive.
