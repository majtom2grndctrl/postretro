# Entity Model

> **Read this when:** working on game logic, implementing entity types, loading entities from level data, or integrating entity state with renderer/audio.
> **Key invariant:** game logic owns all entities. Other subsystems borrow entity state read-only. Entities are component-tagged bags in a scripting registry; the engine ticks first-class components each frame.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Audio](./audio.md)

---

## 1. Design Philosophy

Entities are component-tagged bags in a central registry. Every entity carries a `Transform` at minimum; additional components attach capabilities. The engine walks component columns each tick — no runtime type checks, no downcasting.

This is not a full ECS. There is no archetype storage, no query planner, no system scheduler. Component iteration is straightforward: iterate all entities carrying a given component kind and act on them. Favor readability and simplicity over maximum flexibility.

**Component ownership.** The component vocabulary is engine-closed, for two reasons: hardware- and loop-level concerns (storage layout, per-tick systems a script VM can't drive at scale) and the engine's opinionated genre vocabulary — a retro shooter owns health, shields, and ammo as first-class nouns. Modders extend through declared data (descriptors, store slots, reactions), never new component kinds. Whether a capability is a dedicated component kind or a generic parameterized one (e.g. a shared scalar-stat kind serving both health and shields) is an internal storage choice — invisible to the script surface, which composes and queries components by name.

---

## 2. Entity Representation

### Common Data

Every entity carries a `Transform` (position, rotation, scale in world space). `Transform` is the only component guaranteed present at spawn.

BSP leaf tracking is camera-only. The camera's current leaf is computed each frame for visibility. Entities do not track which leaf they occupy.

### Components

Capabilities attach via component columns in the registry. Current engine components:

| Component | Purpose |
|-----------|---------|
| Transform | World-space position, rotation, scale |
| PlayerMovement | Capsule physics state for the player pawn |
| Light | Dynamic point-light parameters |
| BillboardEmitter | Particle emitter configuration |
| ParticleState | Per-particle simulation state |
| SpriteVisual | Billboard visual parameters |
| FogVolume | Runtime fog-volume parameters |
| Weapon | Runtime weapon params and per-instance cooldown state |
| MeshComponent | Skinned model handle (`model: String`) plus optional declared animation states and per-entity animation state; spawned via `prop_mesh` or a descriptor carrying a mesh component |
| Health | Hit points (`max`, `current`) plus optional hitscan hitbox (one world-aligned AABB, fixed per archetype); declared via the `components.health` descriptor block. Hitscan-targetable iff health **and** (an authored AABB hitbox **or** a zone-bearing skinned model, §7) — a zone-bearing model is tested against bone-posed capsules rather than the AABB. |

Type-specific data lives in the component. An entity is "a player" by virtue of carrying `PlayerMovement`, not by belonging to a typed collection. Future entity types (enemies, doors, projectiles, pickups) follow the same pattern — illustrative, not current scope.

---

## 3. Entity Lifecycle

### Creation

Entities enter the world through two paths:

| Source | When | Examples |
|--------|------|----------|
| Level entity data | Level load | Player spawn, enemies, doors, pickups, triggers, lights |
| Runtime spawning | During gameplay | Projectiles, particles, explosion effects |

Level-load entities are created once when the level is parsed. Runtime entities are created by game logic in response to player actions or game events.

### Update

All entities update each fixed-timestep game logic tick. See section 5 for update order and model.

### Destruction

Entities are destroyed when:

- A scripted or engine bridge condition fires (expired particle, emitter despawn, level unload).
- Health reaches zero: a per-tick death sweep despawns non-player entities at zero HP (and reports kills to the progress tracker). The player pawn never despawns from damage — HP latches at zero and a one-shot death event fires.
- Level unloads (all entities destroyed).

Destruction is immediate: the entity's slot is cleared and its generation bumped (or the slot retired on generation overflow) in the same call that removes the entity. Callers must not hold entity IDs across points where destruction can occur.

---

## 4. Level Entity Data

Level files embed entity definitions: key-value pairs grouped per entity. Each group defines one entity with a `classname` key that identifies its type.

### Loading

The loader reads entity definitions and resolves each `classname` via a classname-dispatch table to an engine spawn handler. Recognized classnames produce an entity initialized from the key-value pairs (position, angle, flags, etc.).

Unknown classnames are logged as warnings and skipped. The engine does not crash on unrecognized entities — maps may contain editor-only or tool entities that have no runtime meaning.

### Key-Value Parsing

Entity properties arrive as string key-value pairs. The loader parses these into typed values (floats, vectors, integers, enums). Malformed values log a warning and fall back to defaults.

**Gameplay tuning params are not map-overridable.** Tuning params — weapon damage/range/fire-rate, movement physics, future wieldable/ability params — are descriptor-owned, never FGD KVPs. Maps cannot rebalance gameplay. Scripts may mutate them at runtime, including on events. This mirrors §7b: `PlayerMovement` physics pass verbatim from the descriptor with no FGD override. When adding a descriptor block, add no FGD KVPs for its tuning params. An archetype may still need FGD presence to be map-placeable (a pickup's position), but never its tuning surface.

---

## 5. Update Model

### Fixed Timestep

Game logic runs at a fixed tick rate, decoupled from render framerate. Renderer interpolates between the last two game states for smooth visuals.

### Update Order

| Order | Stage | Rationale |
|-------|-------|-----------|
| 0 | Transform snapshot | Copies current→previous transform for every already-live entity before any movement system runs. Entities spawned this tick skip the snapshot and initialize previous == current at construction (no pop on spawn). |
| 1 | Player movement tick | Input-driven; resolves capsule physics and position before anything reads player state |
| 2 | Camera follow | Camera position follows the resolved player pawn before aim-dependent systems run |
| 3 | Weapon fire tick | Reads input and active wieldable state after movement/camera settle; may spawn impact effects |
| 4 | Scripting bridges | Emitter, particle sim, light, and fog-volume bridges each walk their component columns and may spawn or despawn entities |

The camera follows the player pawn after movement resolves. When no player pawn exists (no `PlayerMovement` entity), a fly-camera moves directly from input.

### Per-Entity Transform Interpolation

The renderer interpolates each entity's visual transform between the previous- and current-tick positions for sub-tick smoothness. The render-stage accessor `interpolated_transform(id, alpha) -> Transform` takes the frame alpha (0..1, from `frame_timing`'s `current_alpha`) and returns a blended transform: position and scale component-lerped, rotation shortest-path slerped. The stage-0 snapshot (previous = current) ensures entities spawned on the current tick render without popping. The mesh render collector (`mesh_render.rs`) is the first consumer; the accessor is general for future per-entity visual passes.

### Events

Movement and weapon events are collected across all ticks in a frame and drained after the tick loop completes, so reactions observe the fully-settled post-tick world state. Audio is not yet implemented; event categories listed above are illustrative of the intended model, not current consumers.

---

## 6. Subsystem Interactions

### Ownership

Game logic owns entities exclusively. No other subsystem creates, modifies, or destroys entities directly.

| Subsystem | Interaction with entities |
|-----------|--------------------------|
| **Game logic / bridges** | Own, create, update, destroy via the registry |
| **Renderer** | Borrows transform and visual-component data read-only for drawing |
| **Audio** | Not yet implemented. Planned to consume movement events for spatial sound. |
| **Input** | No direct entity interaction; input state flows through game logic |

### BSP Leaf Linkage

The camera's current BSP leaf is computed each frame for portal-visibility culling. Entities do not track a leaf index; there is no per-entity leaf update.

---

## 7. Collision

### World Collision

Entities collide against static world geometry. At level load, PRL static geometry is built into a `parry3d` trimesh held by `CollisionWorld`. Queries use `parry3d::query::*` free functions against this collider — no `QueryPipeline`.

**Skeletal hit zones (M10).** An entity is hitscan-targetable iff it has health **and** (an authored AABB hitbox **or** a zone-bearing skinned model — a glTF whose joints carry hit-zone `extras`). A zone-bearing model is tested against bone-posed capsules, not the AABB. The standalone entity-raycast facility (`scripting_systems::hit_zones::nearest_entity_hit`) resolves the nearest targetable entity for any ray: broad phase is the authored AABB for AABB-only entities and a clip-swept derived bound for zone-bearing ones; narrow phase is the AABB slab test or, per tagged joint, a `parry3d` ray-vs-capsule test (segment from the joint's posed origin to its first child's, a sphere for a tagged leaf; radius = the joint's authored `hitZoneRadius` or the engine default 0.12 m). The model→world transform uses the entity's game-tick position + yaw only (no pitch/roll/scale). The weapon's hitscan delegates to this facility and keeps only world-vs-entity nearest-of resolution; the struck zone tag rides on `WeaponImpact.zone`. General division: **spatial structure rides on the asset** (per-joint hit-zone tags in the glTF), **balance rides in the script** (per-zone damage multipliers — forthcoming, M10 Task 5). See `plans/in-progress/M10--skeletal-hit-zones/`.

### Entity-Entity Collision

Entity-entity collision uses simple bounding volumes: axis-aligned bounding box (AABB) or bounding sphere per entity type. Overlap tests are direct geometric checks, not spatial partitioning.

| Volume type | Use case |
|-------------|----------|
| AABB | Entities with box-like extents (player, enemies, doors) |
| Sphere | Entities where orientation doesn't affect collision (projectiles, pickups) |

Entity type determines which volume shape to use. Volume size is fixed per entity type, not per instance.

### Collision Timing

World collision resolves inline during each entity's movement — the entity slides along or stops at world geometry within its update step. Entity-entity overlap tests run as a separate pass after all entity updates complete. This prevents update-order-dependent collision results: all entities move first, then overlaps are detected and resolved.

---

## 7b. Player Movement Component

The dominant engine entity today is the player pawn. It carries a `PlayerMovement` component alongside its `Transform`. The component holds the capsule geometry, per-axis physics parameters (ground, air, fall), and mutable tick state (velocity, grounded flag, air-jumps remaining, active movement-state variant, air-dashes remaining, dash cooldown timer).

Movement is purely engine-internal. Scripts cannot read or write `PlayerMovement` through `worldQuery`; the movement system owns it exclusively. The camera follows the pawn's position each tick (eye-height offset above capsule center); yaw and pitch remain mouse-driven.

Movement design intent — the custom-kinematic foundation, the declarative author surface, the state-machine seam, and the FPS-flexibility band — lives in `movement.md`. This section covers only the component's place in the entity model.

A player pawn is present only when a `player_spawn` entity in the level resolves to a movement descriptor. When no pawn exists, the engine falls back to a fly-camera so maps are navigable without a player descriptor.

---

## 8. Particles

Each live particle is a full ECS entity in the scripting entity registry, carrying `Transform`, `ParticleState`, and `SpriteVisual`. The emitter bridge spawns and despawns particles each tick via `EntityRegistry::spawn` / `despawn` — scripts never observe or manipulate individual particles.

The particle simulation runs in Rust every game-logic tick: velocity integration, buoyancy/drag, curve-evaluated size and opacity, spin rotation. Per-particle `on_tick` script callbacks are not supported. The particle render collector walks all `ParticleState` entities each render frame, buckets by sprite collection, and hands packed byte slices to the billboard pass.

The parent emitter entity carries `BillboardEmitterComponent`. Particles back-reference their parent via `ParticleState.emitter` (for spin-rate lookup); orphaned particles (emitter despawned) complete their lifetime at their last rotation angle.

---

## 9. Non-Goals

- Full ECS (archetype storage, query planner, system scheduler)
- Entity inheritance hierarchies
- Per-entity script lifecycle callbacks (entity types don't have script attachment points; scripts manipulate entities through registered primitives)
- Networked entity replication
- Entity serialization (save/load)
- Spatial partitioning for entity-entity queries (octree, grid)
- Physics engine integration (rigid body, joints, constraints)
