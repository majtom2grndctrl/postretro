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

Runtime cell lookup is transient. The camera is located each frame for portal traversal, and render collectors may locate an entity or particle position for visibility culling. Entities do not persistently track which cell they occupy.

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
| Weapon | Descriptor-authored weapon tuning plus live magazine, cooldown, reload, and fire/reload input-edge state |
| AmmoReserve | Pawn-owned ammunition balances pooled by authored ammo type; reloads transfer from this reserve into the active weapon magazine |
| MeshComponent | Model handle (`model: String`) for rigid/static or skinned models, plus optional declared animation states and per-entity animation state; spawned via `prop_mesh` or a descriptor carrying a mesh component. Optional descriptor-authored `attachments` mount prop model handles at the model's named sockets (`resource_management.md` §7), rendered at a skinned holder's posed joint or a rigid holder's resolved rest matrix/socket transform — presentation-only (no collision, hit-zone, or gameplay-query participation; no netcode wire change), one authoring shape for skinned and rigid holders. Descriptor `shadowOnly` applies to the owning local-player view; peer viewers render that player avatar forward. |
| Health | Hit points (`max`, `current`) plus optional hitscan hitbox (one world-aligned AABB, fixed per archetype); declared via the `components.health` descriptor block. Health-bearing entities are damage targets. They are hitscan-targetable when they have an authored AABB hitbox or a zone-bearing skinned model (§7). |
| KinematicMover | Engine-owned deterministic translating/spinning mover driver seeded from PRL `KinematicGeometry`; readable through `world.query({ component: "kinematic_mover" })`, whose SDK handle builds declarative mover-command reactions |
| TriggerVolume | Engine-owned, serializable host-authoritative touch/use state for an invisible level-authored AABB. Named `on_fire` / `on_exit` reactions fan out effects; an exit fires only after that player's activation fired. Tracks occupancy; arming reopens Touch activation for already-standing players, while Use remains press-driven. |
| Brain | Engine-internal enemy behavior: the entity's authored behavior state graph, the state it currently occupies, and its per-instance timers. Engine-owned evaluation, author-owned states and transitions (§7c). |

Type-specific data lives in the component. An entity is "a player" by virtue of carrying `PlayerMovement`, not by belonging to a typed collection. Other entity types follow the same pattern: enemies use an **Agent** component for navmesh path-following and collide-and-slide movement, plus a **Brain** component carrying the behavior state graph the AI tick evaluates (§7c). Doors, projectiles, and pickups should attach their behavior through components instead of typed collections.

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
- A scripted `despawn` effect reaches the end-of-frame removal pass. Health reaching zero only latches the entity and freezes non-player kill credit; it neither removes nor reports the entity. Successful scripted removal reports that frozen credit, while the player pawn instead remains present and emits one `playerDied` event at its first zero-HP latch.
- Level unloads (all entities destroyed).

Destruction is immediate: the entity's slot is cleared and its generation bumped (or the slot retired on generation overflow) in the same call that removes the entity. Callers must not hold entity IDs across points where destruction can occur.

Pawn-owned auxiliary entities share the pawn lifecycle. Remote slot cleanup
despawns the pawn and its sibling weapon together. Local level teardown clears
the whole registry and active-wieldable handle together. Player damage does not
despawn the local pawn. These rules keep live weapon timers attached to their
reserve-owning pawn; reload is never cancelled as orphan cleanup.

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
| 1 | Kinematic mover tick | Advances deterministic mover transforms and tick deltas before player movement consumes mover collision/carry |
| 2 | Player movement tick | Input-driven; resolves capsule physics and position before anything reads player state |
| 3 | Trigger tick | Host evaluates touch-entry and use-overlap triggers after player movement; commands mutate mover phase for the next mover tick |
| 4 | AI brain tick | Selects targets, evaluates each enemy's behavior graph, and applies the selected state's motion and action after player movement settles (§7c) |
| 5 | Host camera callback | Host-side camera/aim work runs after movement and AI, before aim-dependent steering and weapon systems |
| 6 | Agent steering tick | Applies navigation steering after AI decisions and host camera work |
| 7 | Weapon reload and fire tick | Advances reloads and transfers completed reloads from pawn reserves before consuming resolved fire and aim data; firing may spawn impact effects and apply damage |
| 8 | Death sweep | Processes entities whose health reached zero after same-tick damage |

Scripting bridges run later, outside the core simulation seam. Emitter, particle sim, light, and fog-volume bridges each walk their component columns and may spawn or despawn entities.

The host camera follows the player pawn after movement and AI resolve. When no player pawn exists (no `PlayerMovement` entity), a fly-camera moves directly from input.

### Per-Entity Transform Interpolation

The renderer interpolates each entity's visual transform between the previous- and current-tick positions for sub-tick smoothness. The render-stage accessor `interpolated_transform(id, alpha) -> Transform` takes the frame alpha (0..1, from `frame_timing`'s `current_alpha`) and returns a blended transform: position and scale component-lerped, rotation shortest-path slerped. The stage-0 snapshot (previous = current) ensures entities spawned on the current tick render without popping. The mesh render collector (`mesh_render.rs`) is the first consumer; the accessor is general for future per-entity visual passes.

### Events

Movement, AI, weapon, and death event names are collected across all catch-up ticks in stage buckets and drained after the tick loop completes. Death events use the sequence-aware drain path. Reactions observe the fully-settled post-tick world state.

---

## 6. Subsystem Interactions

### Ownership

Game logic owns entities exclusively. No other subsystem creates, modifies, or destroys entities directly. Networked replication (Epic 15) preserves this: a server snapshot is applied through a game-logic-owned apply step — the net subsystem produces typed snapshots and never mutates the registry directly.

| Subsystem | Interaction with entities |
|-----------|--------------------------|
| **Game logic / bridges** | Own, create, update, destroy via the registry |
| **Renderer** | Borrows transform and visual-component data read-only for drawing |
| **Audio** | Not yet implemented. Planned to consume movement events for spatial sound. |
| **Input** | No direct entity interaction; input state flows through game logic |

### Cell Linkage

Runtime cells are visibility IDs, not entity ownership. `LevelWorld::locate_cell` maps a point to a cell for camera visibility, render-side entity/particle culling, and diagnostics. The entity registry stores no cell index and runs no per-entity cell update. BSP is compile-only scaffolding and is not part of the entity runtime model.

---

## 7. Collision

### World Collision

Entities collide against static world geometry. At level load, PRL static geometry is built into a `parry3d` trimesh held by `CollisionWorld`. Queries use `parry3d::query::*` free functions against this collider — no `QueryPipeline`. Runtime cells and portals do not answer collision contacts.

**Skeletal hit zones.** The standalone entity-raycast facility resolves the nearest targetable entity for any ray. Targetable entities are Health-bearing damage targets with an authored AABB or zone-bearing skinned model, plus Mesh-only zone-bearing presentation targets used by connected clients for local hit detection. Mesh-only targets carry no `Health`, so they cannot take damage. Broad phase is the authored AABB for Health-only AABB targets and a derived model bound for zone-bearing ones. Narrow phase is the AABB slab test or, per tagged joint, a `parry3d` ray-vs-capsule test. Model→world uses full entity transform composition: scale, rotation, and translation plus mesh origin offset. Stateless meshes use authored rest pose for rendering, attachments, hit zones, and pose probes. Explicit animation components retain state-driven sampling. Gameplay capsules and probes intentionally omit presentation-only pose modifiers. A Health-bearing model with no usable zones falls back to its authored AABB; a Mesh-only target without usable zones is not targetable. The struck zone tag rides on `WeaponImpact.zone`.

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

## 7c. Enemy Brain Component

Enemy behavior is an **authored state graph**, not an engine-closed state enum. The descriptor declares named states; each state fixes a motion verb, an optional action verb, the mesh animation state it requests, and an ordered list of outgoing transitions. A separate ordered `interrupts` list holds any-state transitions. Transition guards are IR expressions (`scripting.md` §11) over a brain-local binding scope; they are bound once at spawn and evaluated by the engine every tick.

**Ownership split.** The author owns which states exist, what each one does, the ordered guards between them, and — through an optional per-graph candidacy predicate — which of the candidates the engine offers are worth engaging. The engine owns everything else: which entities are offered as candidates at all, ranking and retention among the eligible ones, think-stride time-slicing of acquisition, target-switch hysteresis, combat-slot resolution, steering, facing, damage application through the chokepoint, and the aggro gate. Motion and action verbs are closed vocabularies — a state selects an engine behavior, it does not describe one.

The line between the two halves of targeting is **perceivable vs. worth engaging**. The floor decides what an enemy could possibly perceive — a question with one right answer, so it is correctness. Whether a perceivable pawn is worth attacking is taste: a blind grunt, a psychic boss, and an enemy that ignores the wounded are all valid designs. Candidacy can only narrow the offer set, never widen it.

**One brain representation.** Both authoring spellings produce the same component. The legacy four-state block lowers to an equivalent generated graph at spawn, so there is exactly one evaluator code path and no fork to keep in sync.

Invariants the evaluator upholds:

- **Guards evaluate every tick.** No state, animation, or cooldown latches evaluation off. A commitment window is an authored guard over time-in-state, never an engine mechanism.
- **Interrupts first, then the current state's own transitions**, each in declaration order, first true wins. An interrupt naming the current state is skipped — interrupts fire as transitions, never as self re-entry.
- **Guards are read-only.** Per-entity state fields are written by impact policies and reactions; guards only read them. That is how a hit-driven reaction and an authored interrupt compose without either knowing about the other.
- **Candidacy is per-graph eligibility; disengagement is per-state policy.** The candidacy predicate is read-only and evaluated once per offered candidate during a ranking scan, never against the target already retained — dropping a retained target is what guards are for. It answers eligibility only, never rank: it produces a boolean, and ranking stays the engine's.
- **Target selection holds no aliveness policy.** Aliveness gates the attack, not the choice of whom to attack. An enemy may select a corpse and be unable to hit it; disengaging from a downed target is an authored interrupt over target-side facts, not an engine rule. The authored death signal is the death sweep's latch rather than a health comparison — a health comparison cannot distinguish a corpse from no target at all, since target-side facts read zero untargeted.
- **The acquisition leash is symmetric and legacy-only.** Where a leash exists it bounds acquisition and retention on the same terms; a target beyond it is neither acquired nor kept. It stays engine-side because retention needs a clear on every tick, which a per-scan predicate structurally cannot reproduce. No descriptor field spells a disengagement range for an authored graph — a distance clause in the candidacy predicate is how a graph bounds its own acquisition, and a graph with no such clause pursues without limit by design.
- **The think stride is cost machinery and shares no data path with relevance rules.** The distance that prices the stride comes from an unleashed, unfiltered read. Deriving it from a leashed or filtered value inverts the stride — an absent distance reads as due every tick, so the far-band enemy the stride exists to make cheap becomes the one that scans most.
- **Bound guard programs are derived data.** They live in the evaluator, never on the component, so they are never serialized and never affect component equality. They rebuild from the retained graph whenever the entity is seen.
- **Animation is subordinate to graph state.** An unknown animation name warns once at spawn and keeps the prior animation at tick time; it never aborts the tick. A state that pursues without acting is a locomotion state: its animation is a travel cycle, so it yields to the graph's initial-state animation at a standstill. That makes the initial state's animation the graph's rest pose, and authors should pick it accordingly.
- **Death is not a graph transition.** The death sweep latches a zero-HP enemy and the AI tick skips it from then on; an authored impact policy owns the death animation and the despawn delay.

Graph evaluation is host-only. Clients consume replicated animation state and never evaluate guards.

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
- Client-authoritative entity state; deterministic-lockstep replication. (Server-authoritative replication — server owns entities, clients receive replicated `ComponentValue` deltas — is the Epic 15 direction; see `context/research/netcode/`.)
- Entity serialization (save/load)
- Spatial partitioning for entity-entity queries (octree, grid)
- Physics engine integration (rigid body, joints, constraints)
