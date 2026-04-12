# Entity Model

> **Read this when:** working on game logic, implementing entity types, loading entities from level data, or integrating entity state with renderer/audio.
> **Key invariant:** game logic owns all entities. Other subsystems borrow entity state read-only. Entities are concrete typed objects, not component bags.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Audio](./audio.md)

---

## 1. Design Philosophy

Simple, direct entity model. Not an ECS. Not a component system.

Each entity is a concrete typed object. Player is a player, door is a door, projectile is a projectile. Type-specific data lives on the concrete type. No generic property bags, no trait-object soup, no inheritance hierarchies.

Typed collections over heterogeneous lists. The game stores players in one collection, enemies in another, projectiles in a third. Iteration is direct and predictable. No runtime type checks, no downcasting.

Favor readability and simplicity over maximum flexibility.

---

## 2. Entity Representation

### Common Data

All entities share a core set of spatial state.

| Data | Purpose |
|------|---------|
| Position | World-space location (3D vector) |
| Orientation | Facing direction (yaw at minimum; pitch where relevant) |
| Velocity | Movement vector, applied each tick |
| BSP leaf | Current leaf index for visibility culling, audio reverb zone lookup, and collision context |
| Bounding volume | AABB or sphere for entity-entity collision |

Common data is embedded directly in each entity type. No shared base struct inheritance — each type carries its own copy of these fields.

### Type-Specific Data

Type-specific state lives on the concrete type. Examples:

- Health and armor on entities that take damage.
- Weapon state (ammo, cooldown, selected weapon) on the player.
- AI state (patrol path, alert level, target) on enemies.
- Open/closed state and linked trigger on doors.
- Damage, speed, and owner on projectiles.

These are illustrative. Specific entity types are implementation scope, not spec scope.

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

- Health reaches zero (killed).
- A trigger condition fires (consumed pickup, expired projectile, door that auto-removes).
- Level unloads (all entities destroyed).

Destruction is deferred to the end of the tick. Entities marked for destruction are removed after all updates complete. This avoids invalidating references mid-tick.

---

## 4. Level Entity Data

Level files embed entity definitions: key-value pairs grouped per entity. Each group defines one entity with a `classname` key that identifies its type.

### Loading

The loader reads entity definitions and resolves each `classname` to an engine entity type. Recognized classnames produce the corresponding entity, initialized from the key-value pairs (position, angle, flags, etc.).

Unknown classnames are logged as warnings and skipped. The engine does not crash on unrecognized entities — maps may contain editor-only or tool entities that have no runtime meaning.

### Key-Value Parsing

Entity properties arrive as string key-value pairs. The loader parses these into typed values (floats, vectors, integers, enums). Malformed values log a warning and fall back to defaults.

---

## 5. Update Model

### Fixed Timestep

Game logic runs at a fixed tick rate, decoupled from render framerate. All entities update at the same rate. Renderer interpolates between the last two game states for smooth visuals.

### Update Order

| Order | Category | Rationale |
|-------|----------|-----------|
| 1 | Player | Input-driven; must resolve before anything reacts to player state |
| 2 | All other entities | Read world state including updated player position |

Within non-player entities, update order is stable but not individually specified. Entities read world state (BSP geometry, other entity positions) but do not modify other entities directly during their own update.

### Events

Entities emit game events during their update. Events are collected, not processed inline.

| Event category | Examples |
|----------------|----------|
| Audio triggers | Footstep, gunshot, explosion, door movement |
| Damage | Projectile hit, hazard contact |
| State changes | Pickup collected, enemy killed, door opened |
| Visual effects | Muzzle flash, impact spark, blood spray |

Events are consumed by audio and renderer after game logic completes, respecting frame order: Input -> Game logic -> Audio -> Render -> Present.

---

## 6. Subsystem Interactions

### Ownership

Game logic owns entities exclusively. No other subsystem creates, modifies, or destroys entities.

| Subsystem | Interaction with entities |
|-----------|--------------------------|
| **Game logic** | Owns, creates, updates, destroys |
| **Renderer** | Borrows position, orientation, and visual data (sprite index, animation frame) read-only for drawing |
| **Audio** | Consumes game events (sound triggers) emitted during update; reads entity positions for spatial audio |
| **Input** | No direct interaction with entities; input state flows through game logic |

### BSP Leaf Linkage

Each entity tracks which BSP leaf it occupies. This leaf index serves three consumers:

| Consumer | Use |
|----------|-----|
| Renderer | Visibility culling — skip entities in leaves outside the camera's current visibility set |
| Audio | Reverb zone lookup — determine acoustic environment for sounds emitted at entity position |
| Game logic | Spatial queries — which entities are near a point, which zone an entity occupies |

Leaf index updates each tick after position changes.

---

## 7. Collision

### World Collision

Entities collide against BSP world geometry. The BSP tree and brush data baked into PRL provide convex brush hulls — the original brush geometry used by the mapper. Collision tests against these hulls support arbitrary bounding volume sizes.

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

## 8. Non-Goals

- ECS or component system
- Entity inheritance hierarchies
- Scripting or modding API for entity behavior
- Networked entity replication
- Entity serialization (save/load)
- Spatial partitioning for entity-entity queries (octree, grid)
- Physics engine integration (rigid body, joints, constraints)
