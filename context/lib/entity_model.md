# Entity Model

> **Read this when:** working on game logic, implementing entity types, loading entities from level data, or integrating entity state with renderer/audio.
> **Key invariant:** game logic owns all entities. Other subsystems borrow entity state read-only. Entities are component-tagged bags in a scripting registry; the engine ticks first-class components each frame.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Audio](./audio.md)

---

## 1. Design Philosophy

Entities are component-tagged bags in a central registry. Every entity carries a `Transform` at minimum; additional components attach capabilities. The engine walks component columns each tick — no runtime type checks, no downcasting.

This is not a full ECS. There is no archetype storage, no query planner, no system scheduler. Component iteration is straightforward: iterate all entities carrying a given component kind and act on them. Favor readability and simplicity over maximum flexibility.

**Registry versus pool.** The registry holds what something must address individually: a hit, a replication update, an interpolation target, a script handle. Anonymous, disposable presentation lives outside it, so it never adds to tick cost or competes for registry slots. Particles are the case (§8).

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
| Immersion | Output-only: the fraction of the pawn's capsule its winning fluid covered on the last authoritative movement tick. Written by host movement; read only by the `player.immersion` slot (§7b). Never replicated |
| Light | Dynamic point-light parameters |
| BillboardEmitter | Particle emitter configuration |
| SpriteVisual | Billboard visual parameters |
| FogVolume | Runtime fog-volume parameters |
| Weapon | Descriptor-authored actions and tuning plus one live resource, shared recovery, reload, charging/execution, and input-edge state |
| AmmoReserve | Pawn-owned ammunition balances pooled by authored ammo type; reloads transfer from this reserve into the active weapon magazine |
| MeshComponent | Model handle (`model: String`) for rigid/static or skinned models, plus optional declared animation states and per-entity animation state; spawned via `prop_mesh` or a descriptor carrying a mesh component. Optional descriptor-authored `attachments` mount prop model handles at the model's named sockets (`resource_management.md` §7), rendered at a skinned holder's posed joint or a rigid holder's resolved rest matrix/socket transform — presentation-only (no collision, hit-zone, or gameplay-query participation; no netcode wire change), one authoring shape for skinned and rigid holders. Descriptor `shadowOnly` applies to the owning local-player view; peer viewers render that player avatar forward. |
| Health | Hit points (`max`, `current`) plus optional direct-impact hitbox (one world-aligned AABB, fixed per archetype); declared via the `components.health` descriptor block. Health-bearing entities are damage targets. Hitscan rays and swept projectiles share target shapes from an authored AABB or zone-bearing skinned model (§7). |
| KinematicMover | Engine-owned deterministic translating/spinning mover driver seeded from PRL `KinematicGeometry`; addressed as `getMapEntities("mover")` members, whose SDK handle builds declarative mover-command reactions. Also carries the mover's authored transition event addresses and sound keys for the app drain; the sound keys are presentation-only. |
| TriggerVolume | Engine-owned, serializable host-authoritative touch/use state for an invisible level-authored AABB. Named `on_fire` / `on_exit` reactions fan out effects; an exit fires only after that player's activation fired. Tracks occupancy; arming reopens Touch activation for already-standing players, while Use remains press-driven. |
| Touchable | Host-authoritative touch mode and sphere radius for a world item. Presence identifies the entity as a touchable world item. |
| Brain | Engine-internal enemy behavior: the entity's authored behavior state graph, the state it currently occupies, and its per-instance timers. Engine-owned evaluation, author-owned states and transitions (§7c). |
| Agent | Engine-internal navmesh path-following state for a moving enemy: the collision capsule seeded at attach time from the baked agent parameters, live velocity and grounded flag, the followed path plus its cursor, and a destination. No script surface (the `PlayerMovement` precedent). |
| Spawner | Map-authored enemy spawn point: the archetype name it materializes, how many enemies each firing creates, and the `spawned_tags` each spawned enemy carries (its own `_tags` never pass to spawns). Addressed as `getMapEntities("spawner")` members; a member's `fire()` spawns one batch from that spawner only. Stateless across firings; the resolved descriptor stays in the session spawn context rather than the serializable component. |
| ProjectileTrap | Map-authored, bodiless (no model, collision or health) trap naming one weapon descriptor; it fires from its origin along the view-frame forward of its map angles (§4). `fire` runs one execution of the weapon's primary action; a `fire` that meets a running execution or recovery is dropped. `start` repeats on an anchored schedule at an authored interval after an authored phase; `stop` cancels repeats and lets an in-flight execution finish. Addressed as `getMapEntities("trap")` members and by trigger commands. Host-only; started state and schedules belong to one level install and never replicate. Its runtime (the stripped weapon, the schedule, its own activation clock) rides the component, outside the registry's `Weapon` column, so weapon sweeps never reach it; a fixed-tick trap stage after AI and before steering drives it (`crates/sim/src/traps`). |
| EntityState | Per-instance modder-owned numeric fields. Every entity receives an empty component at spawn; fields are emergent — a name is created by its first write and an unset name reads as zero. No descriptor surface and no schema, so an entity may gain state mid-play without a respawn. |
| DeferredEffect | Per-entity queue of impact effects awaiting a later fixed tick, plus the terminal inert flag marking an entity bound for the frame-end removal pass. Every entity receives one at spawn, so deferring an effect never depends on a brain being present. Bounded per entity; overflow drops the newest request. |
| DescriptorProvenance | Which descriptor spawned the entity, through which spawn path, which components that descriptor owns, and which map KVP overrides landed. Descriptor hot reload and replication plan against it rather than re-deriving ownership. |

Type-specific data lives in the component. An entity is "a player" by virtue of carrying `PlayerMovement`, not by belonging to a typed collection. Other entity types follow the same pattern: enemies use an **Agent** component for navmesh path-following and collide-and-slide movement, plus a **Brain** component carrying the behavior state graph the AI tick evaluates (§7c). Doors, projectiles, and pickups should attach their behavior through components instead of typed collections.

**Weapon vocabulary.** One canonical weapon descriptor type — `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`; SDK `WeaponDescriptor`), authored as a `weapon` block on `defineEntity` and resolved by canonical name through the shared descriptor lookup (`find_descriptor`). No player-vs-enemy descriptor split and no branch on actor kind: a player-loadout weapon and an enemy attack resolve the same descriptor type. A restriction on who may wield a weapon, if ever added, is an attribute of the weapon — never a separate descriptor kind or an actor-type branch. Player pawns reference weapons through `components.inventory.loadout`; enemy attack entries may reference the same descriptor. Presentation follows the same rule: a weapon's sounds play whoever wields it. Projectiles retain their originating action, resolved shot identity, and effective fire/impact sounds. Delayed contacts never resolve through the shooter's current weapon (`audio.md` §4). Projectile traps are the one deliberate exception to the no-branch rule: a trap pays no ammo, heat or cell and never reloads, dry-fires or overheats. A trap takes projectile weapons only and refuses a charged primary action, having no held trigger to release; an unknown name, a hitscan weapon or a charged primary leaves it inert with one load warning. Its weapon resolves once per level install, so a hot-reloaded descriptor reaches it at the next install. Trap shots carry no attacker, direct or splash, and credit the weapon's own credit source, so a damaged enemy registers damage from an unknown source and never turns toward the trap.

**Weapon state.** Timed weapon states live in one component-level state machine: a new state is a variant plus its transition arms, never a new component field or boolean. Fire authorization is computed without consulting weapon state, so a preemption check and the real fire gate share one evaluation and cannot drift. State-dependent terms are applied by the caller around that shared verdict, not folded into it.

**Weapon activations.** Required `primary` and optional `secondary` actions share one execution, resource, and recovery gate. `press` starts once per fresh edge; `hold` requests another execution after completion and recovery. Secondary wins simultaneous fresh starts. An existing execution is never interrupted by another start, and blocked presses are consumed rather than queued. The one exception is a press refused only by the shooter's own pawn-state gate (a fire-blocking vault, `movement.md` §4): it is held and fires on the first open tick while the trigger stays held. Authors compose bounded shot/wait sequences; there is no burst opcode (`scripting.md` §11).

Charge belongs to an action, independent of resource kind. It requires `press`, freezes on explicit release, and never auto-fires at full charge. Release before the minimum cancels without debit; a normal sequence continues after button release. Switch, drop, death, input suspension, disconnect, level change, and descriptor replacement cancel future work, and so does entering a fire-blocking vault. `charge.interruptedBy` is a closed map from interrupt source to whether that source cancels the charge while it is still charging. Every key is optional and defaults to `true`; an unknown source, a non-boolean or a non-object rejects content, through one check both runtimes share. `vault` is the only source today: `false` keeps the charge charging through the climb and fires its release once the climb ends (`networking.md` §Combat authority). Future sources (taking damage, NPC interactions) join the same map rather than adding fields. Death, switching, dropping and the other cancels above are not interrupt sources and always cancel. A fresh reload press cancels before the ordinary reload gate, even when reload cannot start; holding reload does not cancel repeatedly.

Every due shot samples current aim and freezes resolved combat and presentation tuning. Shots spend separately; waits and cancelled charge spend nothing. Accepted shots set action recovery, while later shots in the same execution bypass the start gate. Insufficient ammo/cell ends execution with one dry-fire cue and action recovery; heat lockout ends it silently. A threshold-crossing heat shot fires before cancelling remaining steps. Completion and cancellation preserve paid resources and recovery already owed. Authored visual size is independent of collision and splash radius. Authorized shots retain their captured values through later switching or content replacement.

**Dynamic accuracy.** Per-shot spread is a dynamic axis, not a fixed cone. Descriptor-tuned bloom, movement, and optional vertical bias compose the effective hitscan half-angle through one shared host/client calculation. Authored tuning replicates; bloom accumulator and idle timer remain live per-instance state across hot reload. Projectiles converge on the crosshair and never sample the cone. Each machine exposes its predicted neutral-degree result through local-only `player.spread` HUD state; the reticle maps it to a spread ring in script.

**Weapon resources.** The descriptor's `resource` is a tagged union of `ammo`, `heat`, and `cell`, and a weapon carries at most one. Each variant's numbers are authored inside the `resource` block and projected through `effective()` beside the flat stats. Heat and cell differ from ammo in three ways:

- **They update every fixed tick, independent of fire.** Heat cools and cells refill for every weapon instance, wielded or holstered, including while charging. Recovery freezes on inactive instances. The passive update runs once per tick on the authoritative path, before the active weapon's shot gate. Connected clients never run it; they read the replicated values.
- **Overheat is a latch on the heat resource, not a weapon state.** Switching away lowers the weapon, which would overwrite a state variant; a latch survives the round trip. The shot that reaches the threshold still fires. The latch then refuses pulls silently until heat returns to zero, cooling without the usual delay.
- **Their gates join the state-blind fire verdict beside the magazine check.** Overheated is a silent refusal. A cell that cannot pay for a shot dry-fires, exactly as an empty magazine does.

Heat and cell have no reserve, ignore reload input, and do not carry across a level transition: the carried loadout keeps magazines only, so a cell arrives full and an overheat clears. HUD state follows the health pattern of a raw value plus a companion max (`scripting.md` §5), and a local slot names the active weapon's resource kind so a HUD can pick its readout.

**Splash / area effects.** Splash is a `splash` block on `WeaponDescriptor`, beside the projectile block rather than nested in its travel tuning. Current use is projectile impacts; descriptors reject splash on hitscan weapons. A blast damages each visible live damageable volume in radius once through the damage chokepoint, using authored linear distance falloff and optional owner exclusion. Static world geometry occludes; movers and entities do not. Host-authoritative results reach clients through replicated Health, and remote observers take the explosion burst from the reliable observer impact cue, which excludes the predicted owner. See [Networking](./networking.md) §Presentation events vs. replicated state and §Combat authority.

**Knockback.** Weapon hits carry an independent velocity-change payload. Direct hits use travel direction; splash uses the vector from blast center to damageable-volume center. Both support an authored upward blend. Splash force has its own distance falloff and owner multiplier: disabling self-damage still permits rocket jumping, and zero damage still permits a concussive blast. Splash replaces direct health damage; explicitly authored direct and radial pushes compose. Only live damageable players and navigation agents receive movement impulses. Health-only props remain stationary. Hit-zone multipliers affect damage only. Direct projectile tuning is captured at fire time; impact never re-reads the weapon. Response belongs to `movement.knockback` for players and `behavior.knockback` for enemies. See [Movement](./movement.md) §6.

---

## 3. Entity Lifecycle

### Creation

Entities enter the world through two paths:

| Source | When | Examples |
|--------|------|----------|
| Level entity data | Level load | Player spawn, enemies, doors, pickups, triggers, lights |
| Runtime spawning | During gameplay | Projectiles, scripted actors |

Level-load entities are created once when the level is parsed. Runtime entities are created by game logic in response to player actions or game events. Anonymous particles and impact bursts use the separate presentation pool (§8).

### Update

All entities participate in applicable fixed-timestep game logic stages each tick. Once damage settles, a zero-HP enemy is excluded from AI and steering but may still participate in other applicable stages. See section 5 for update order and model.

### Destruction

Entities are destroyed when:

- A scripted or engine bridge condition fires (emitter despawn, level unload).
- A scripted `despawn` effect reaches the end-of-frame removal pass. Health reaching zero only latches the entity and freezes non-player kill credit; it neither removes nor reports the entity. Successful scripted removal reports that frozen credit, while the player pawn instead remains present and emits one `playerDied` event at its first zero-HP latch.
- Level unloads (all entities destroyed).

Destruction is immediate: the entity's slot is cleared and its generation bumped (or the slot retired on generation overflow) in the same call that removes the entity. Callers must not hold entity IDs across points where destruction can occur.

**Held ownership.** Inventory membership defines whether an item is held, independently of its components and equip state. One canonical relation and a derived registry lookup serve replication, spatial targeting, AI actor/target selection, steering and mover blocking. All live slots count. Mutations and lifecycle cleanup keep the relation consistent; conflicting live ownership fails atomically. Holding preserves retained Health and behavior state and existing weapon execution/resource rules. Successful drop restores world participation. Releasing a held wieldable never moves the active weapon except to replace the one released: releasing the active weapon mid-switch moves to a live switch target, a dead target ends the switch on the weapon in hand, and a released weapon leaves idle. Single-player, host and a client applying a host-side drop share this rule. Explicit scripted Health mutation and lifecycle effects retain their semantics. Descriptor refresh never infers ownership from component presence.

Pawn-owned auxiliary entities share the pawn lifecycle. Retirement follows the
pawn's current inventory, including later pickups; items that were dropped or
acquired elsewhere survive. Destruction clears
ownership before entity-slot reuse. Local level teardown clears
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

**Map declarations seed level state; descriptors tune archetypes.** Maps may declare initial level gameplay values at load, including world gravity, so basic content does not require a script. Maps may also place static regional gravity, push and fluid volumes, baked at compile time (§7, Environment Volumes); any further regional field needs its own amendment, and gas and haze stay with fog volumes. `force` on `push_volume` is one such amendment: a modifier that rides its volume's push and wins with it, scaling what bodies feel while particles and fluid surfaces show the nominal push (`movement.md` §6). Teleporter regions are another: baked like the rest, but armed and disarmed by script, so not static (§7). Ghost links are a third: baked regions scripts arm, presentation only (§7d). A map declaration cannot mutate gameplay after load or override per-archetype descriptor tuning. Weapon damage/range/action timing, player movement tuning, and future wieldable/ability tuning remain descriptor-owned. Scripts may mutate runtime values through their supported primitives, including on events. An archetype may need FGD presence to be map-placeable (a pickup's position), but never its tuning surface.

**Facing convention (engine-wide).** A map entity's yaw is the arrow TrenchBroom draws: Quake yaw 0 faces +X and turns counter-clockwise. The compiler keeps its sign (`build_pipeline.md` §Supported Map Formats), because the axis map (x, y, z) → (−y, z, −x) is a proper rotation. Camera and player forward is −Z; a model's front (glTF) is +Z. The half turn between them is applied at exactly one seam, `MapEntity::model_rotation_quat()`, which every model-bearing placement spawns from: props, spawners, descriptor-placed enemies, dummies and pickups, and loadout weapons. View-frame and frontless placements — player spawn and network slots, billboard emitters — spawn from `rotation_quat()`, and teleport destinations use the arrow's yaw directly. Copies a spawner releases fan out to the left of its arrow. A new spawn path picks its frame by the axis its entity treats as forward, never by negating yaw.

---

## 5. Update Model

### Fixed Timestep

Game logic runs at a fixed tick rate, decoupled from render framerate. Renderer interpolates between the last two game states for smooth visuals.

### Update Order

| Order | Stage | Rationale |
|-------|-------|-----------|
| 0 | Transform snapshot | Copies current→previous transform for every already-live entity before any movement system runs. Entities spawned this tick skip the snapshot and initialize previous == current at construction (no pop on spawn). |
| 1 | Kinematic mover tick | Advances deterministic mover transforms and tick deltas before player movement consumes mover collision/carry |
| 2 | Player movement tick | Input-driven; resolves capsule physics and position before anything reads player state. Teleporter arm/disarm queued since the previous tick has applied right after the transform snapshot, so every pawn reads one armed state. A teleport resolves after each pawn's movement step, so the trigger tick sees the arrival pose; authoritative `teleport` events dispatch once every pawn has moved |
| 3 | Trigger tick | Host evaluates touch-entry and use-overlap triggers after player movement; commands mutate mover phase for the next mover tick |
| 4 | Touchable tick | Host evaluates player-item touch overlap after triggers and before AI |
| 5 | Authoritative projectile flight | Advances existing gameplay projectiles after movement settles. Impacts route through the Health chokepoint before AI reads damage and attacker facts. |
| 6 | AI brain tick | Excludes zero-HP brains, then selects targets, evaluates each admitted enemy's behavior graph, and applies the selected state's motion and action after player movement and projectile impacts settle (§7c) |
| 7 | Host camera callback | Host-side camera/aim work runs after movement and AI, before aim-dependent steering and weapon systems |
| 8 | Agent steering tick | Applies navigation steering after AI decisions and host camera work |
| 9 | Weapon reload and activation tick | Advances passive resources and reloads, transfers completed reloads from pawn reserves, then consumes actions and per-shot aim; accepted shots may spawn impact effects and apply damage |
| 10 | Projectile launch finalization | Closes spawn grace for projectiles launched after the flight stage without moving them. They begin flight on the next fixed tick. |
| 11 | Death sweep | Processes entities whose health reached zero after same-tick damage |

Scripting bridges run later, outside the core simulation seam. Emitter, light and fog-volume bridges walk their configuration components. The emitter bridge writes particles into the presentation pool (§8); particle simulation updates that pool without spawning or rewriting registry entities.

The host camera follows the player pawn after movement and AI resolve. When no player pawn exists (no `PlayerMovement` entity), a fly-camera moves directly from input.

### Per-Entity Transform Interpolation

The renderer interpolates each entity's visual transform between the previous- and current-tick positions for sub-tick smoothness. The render-stage accessor `interpolated_transform(id, alpha) -> Transform` takes the frame alpha (0..1, from `frame_timing`'s `current_alpha`) and returns a blended transform: position and scale component-lerped, rotation shortest-path slerped. The stage-0 snapshot (previous = current) ensures entities spawned on the current tick render without popping. The mesh render collector (`mesh_render.rs`) is the first consumer; the accessor is general for future per-entity visual passes.

### Events

Movement, AI, weapon, and death events are collected across all catch-up ticks in stage buckets and drained after the tick loop completes. Movement, AI and weapon events travel as **emissions**: the event address paired with its emitter — an entity plus its origin at the tick, or an impact's contact set — and retained presentation provenance. Weapon shots carry their originating action and effective sound keys; enemy events carry their behavior graph and attack or entered activity. An entered activity with no `onEnter` still emits, with no address. Emissions are stamped where the event happens, so an emitter despawned before the drain still has a position. The drain fires each built-in and authored alias with the same emitter and plays descriptor sounds (`audio.md` §4, `scripting.md` §12). Death events use the sequence-aware drain path. Reactions observe the fully-settled post-tick world state.

---

## 6. Subsystem Interactions

### Ownership

Game logic owns entities exclusively. No other subsystem creates, modifies, or destroys entities directly. Server-authoritative replication preserves this: game logic applies server snapshots; netcode produces typed snapshots and never mutates the registry directly.

| Subsystem | Interaction with entities |
|-----------|--------------------------|
| **Game logic / bridges** | Own, create, update, destroy via the registry |
| **Renderer** | Borrows transform and visual-component data read-only for drawing |
| **Audio** | No direct entity access. The app drain turns emissions into anchored sound requests and answers audio's per-frame position queries from render-interpolated transforms, read-only; audio sees only opaque entity keys (`audio.md` §1). |
| **Input** | No direct entity interaction; input state flows through game logic |

### Cell Linkage

Runtime cells are visibility IDs, not entity ownership. `LevelWorld::locate_cell` maps a point to a cell for camera visibility, render-side entity/particle culling, and diagnostics. The entity registry stores no cell index and runs no per-entity cell update. BSP is compile-only scaffolding and is not part of the entity runtime model.

---

## 7. Collision

### World Collision

Entities collide against static world geometry. At level load, PRL static geometry is built into a `parry3d` trimesh held by `CollisionWorld`. Queries use `parry3d::query::*` free functions against this collider — no `QueryPipeline`. The one exception is the skin-distance capsule sweep movement and movers use: it runs through a small engine query dispatcher that keeps parry's BVH traversal for choosing candidate triangles but answers each capsule–triangle pair itself, by conservative advancement from the cast's start on exact closest points. A hit lands where the capsule first reaches the skin band and is dropped unless the rest of the motion would carry the capsule more than 0.5 mm into the band. Contract: a cast that starts outside the band, or at most 0.5 mm inside it, never ends more than 0.5 mm inside it. A tick-level contract sits beside it: a body whose own movement starts a tick clear of static geometry never ends that tick overlapping a surface by more than 0.5 mm. The cast contract bounds depth into the band per cast; the tick contract bounds surface overlap per tick (`movement.md` §1). Pairs the dispatcher can't answer exactly — not capsule–triangle, a degenerate triangle, or a capsule core piercing the triangle — use parry's cast. parry's own leaf cast is unreliable at the band boundary — tilted normals that bleed speed on flat floors, missed in-band starts that walk a capsule into a wall it rests against (dimforge/parry#452). Known limitation: a slide across a coplanar triangle seam that starts deeper than 0.5 mm inside the band hits the neighbour triangle's shared edge at a slightly tilted normal (about 4° from 1 mm deep); internal-edge handling is the follow-up fix. Static and mover casts treat `max_toi` as inclusive; parry's trimesh traversal drops a hit exactly at the limit. Combined static-plus-mover queries skip a mover before its narrow phase when its mesh-centred bounding sphere, built at load, cannot reach the query's swept sphere. The test carries rounding slack and passes non-finite input through, so the cull never changes a result. Static collision is built before any level-install mutation, so a mesh parry rejects fails the load and leaves the empty, already-unloaded world untouched. That route is defensive: parry 0.31 rejects a trimesh only for empty indices, which `CollisionWorld::from_level` pre-filters, and PRL decode validates the rest. Likewise a mover whose collision mesh parry rejects loads with no collider and a `[Loader]` warning, but kinematic-geometry decode already rejects empty mover geometry. Runtime cells and portals do not answer collision contacts.

**Skeletal hit zones.** The standalone entity-raycast facility resolves the nearest targetable entity for any ray. Targetable entities are Health-bearing damage targets with an authored AABB or zone-bearing skinned model, plus Mesh-only zone-bearing presentation targets used by connected clients for local hit detection. Mesh-only targets carry no `Health`, so they cannot take damage. Broad phase is the authored AABB for Health-only AABB targets and a derived model bound for zone-bearing ones. Narrow phase is the AABB slab test or, per tagged joint, a `parry3d` ray-vs-capsule test. Model→world uses full entity transform composition: scale, rotation, and translation plus mesh origin offset. Stateless meshes use authored rest pose for rendering, attachments, hit zones, and pose probes. Explicit animation components retain state-driven sampling. Gameplay capsules and probes intentionally omit presentation-only pose modifiers. A Health-bearing model with no usable zones falls back to its authored AABB; a Mesh-only target without usable zones is not targetable. The struck zone tag rides on `WeaponImpact.zone`.

**Radial entity overlap.** A one-to-many spatial query, sibling to nearest-entity raycast: given a world center and radius it returns every live damageable entity whose broad-phase volume intersects the sphere, each with its nearest point and distance. Caller-agnostic — no weapon, projectile, or splash knowledge. It reuses the raycast broad phase and shared liveness gate. Optional static-world occlusion drops wall-hidden targets; a caller-supplied predicate handles exclusions. This is the first non-ray query-family member; future melee sweeps and area damage-over-time can reuse its candidate boundary.

### Entity-Entity Collision

Entity-entity collision uses simple bounding volumes: axis-aligned bounding box (AABB) or bounding sphere. Overlap tests are direct geometric checks, not spatial partitioning.

| Volume type | Use case |
|-------------|----------|
| AABB | Entities with box-like extents (player, enemies, doors) |
| Sphere | Entities where orientation doesn't affect collision (projectiles, pickups) |

Entity descriptors select the volume shape and fixed dimensions. Touchable items carry their descriptor-seeded sphere radius in `Touchable`; player-item overlap tests that sphere against the player capsule.

### Collision Timing

World collision resolves inline during each entity's movement — the entity slides along or stops at world geometry within its update step. Trigger-volume overlap runs after player movement, and touchable-item sphere/capsule overlap runs immediately after that trigger stage and before AI: only player pawns and stationary world items participate, so this narrower placement observes every relevant movement without delaying the touch to the next tick. Other entity-entity overlap tests run as a separate pass after all relevant entity updates complete. This prevents update-order-dependent collision results: every participating entity moves first, then overlaps are detected and resolved.

### Environment Volumes

Maps place static regional gravity, push and fluid volumes (`build_pipeline.md` §Custom FGD, id 52). They are non-solid: no collision, cell, portal, lightmap, SDF or navmesh role. `postretro-collision` answers every query over them: pure geometry over level data, no movement or entities. Each authored brush is a convex leaf — a plane set with bounds and a contents mask naming the fields its volume sets. Query indexes are built at level install, not baked, after static collision and before the level content digest publishes or the first tick runs. Installing another level replaces them whole; unload clears them. Queries take a contents mask and never see a volume whose contents miss it. Acceleration-only point queries may use bounded specialized indexes; candidate narrowing must preserve exact containment and independent field winners. Fluid and mixed queries retain the complete geometry index.

**One overlap rule per field.** A field's value comes from the winning volume among those that set it and are in reach; with none, the level default applies (level gravity, zero push, dry). The winner is the highest priority, then the smallest summed brush volume — so a pool inside a room wins without priorities — compared at 8 significant bits so near-equal sizes usually tie, then the earliest map entity. Fields resolve independently: a pool with no gravity inside a low-gravity room takes the pool's fluid and the room's gravity.

- **Gravity and push** reach is the body's position. Push yields two outputs: the winner's nominal push, and the felt push, nominal times the winner's force (default 1). Force never ranks a winner, so a force-0 push still carves its region.
- **Fluid** reach is the capsule's vertical segment, feet to head. A fluid volume is in reach only when it covers a positive length of the segment, so feet resting on the surface stay dry. Immersion is the segment's coverage by the union of every in-reach volume carrying the winner's fluid, so a pool split into several entities of one fluid reads whole, a different fluid never adds, and leaving through any face, bottom included, drops it.
- A non-finite probe resolves the defaults, dry. Resolution is deterministic and allocation-free; its result never depends on tree shape or visit order.

**Who resolves what.** Each body resolves at its own tick-start position, every tick.

| Body | Resolves |
|---|---|
| Player pawn | The full environment, felt push (`movement.md` §4, §6) |
| AI agent | Gravity only, clamped to its downward part. Agent collide-and-slide adds gravity every tick, grounded included, so upward gravity would lift it off the navmesh; in an upward-gravity volume it behaves as in zero gravity. Agents ignore push, never swim, and walk fluid floors |
| Projectile | Nothing; projectiles apply no gravity |
| Particle | Gravity and the nominal push at its post-step position, once per rendered frame; never fluid (§8). A force-0 river still carries foam |
| Fluid surface face | The nominal push 2 mm inside the face, at install and on every fluid-table or tuning refresh, for its drift (`rendering_pipeline.md` §7.3a) |

**Teleporter regions.** A `trigger_teleport` is a regional volume like those above — convex leaves, one entity's brushes read as their union (`build_pipeline.md` id 55), indexed at install — but not static. Each installs one entity carrying the `Teleporter` component (map-order index, source entity, armed state, `arrival_sound`, `on_teleport`), placed and tagged from its id-29 map-entity companion so scripts can find it. Armed state starts from `enabled_on_spawn`; scripts arm and disarm it on the host, applied at the start of the next movement tick (`scripting.md` §12), and it replicates (`networking.md` §Current contract). Mode picks the arrival: classic lands the body point on its `target` `teleport_destination` facing that destination's yaw; seamless carries the body's offset from a source-side `landmark` destination onto the target, turned by the yaw difference. Destinations are compiler-only and leave no runtime entity. Only player pawns resolve teleporters; agents, projectiles and particles never teleport. A pawn tests its body point, where gravity and push resolve, against armed regions it is not latched in, after its movement step, not at tick start. Among several, the smallest summed brush volume wins, overlapping brushes counted twice; there is no priority and no size quantization, so map order breaks only an exact tie of the computed sums, and near-identical regions rank by float rounding. The teleport ends the pawn's movement tick at the arrival pose. Relocation, arrival state and latches: `movement.md` §6 (Teleport).

**Hitscan splash facts.** A hitscan pellet reports each fluid-surface crossing strictly before its blocking hit as a fact: point, outward normal of the crossed face, fluid. A ray starting on a face never splashes there, a face shared by two brushes of one volume is no crossing, and two adjacent fluid entities give two. The fact is presentation: never replicated or hashed. Nothing consumes it yet.

---

## 7b. Player Movement Component

The dominant engine entity today is the player pawn. It carries a `PlayerMovement` component alongside its `Transform`. The component holds the capsule geometry, per-axis physics parameters (ground, air, fall), and mutable tick state (velocity, grounded flag, air-jumps remaining, active movement-state variant, air-dashes remaining, dash cooldown timer).

Movement is engine-internal. No script surface writes `PlayerMovement` — `getMapEntities` exposes map-placed kinds only; the movement system owns it exclusively. Scripts read one readonly projection per player: `player.swimming` from this component's movement state, and `player.immersion` from the separate `Immersion` component (`scripting.md` §5). Immersion is its own component because `PlayerMovement` is cloned into reconcile replay and merged from the wire; host movement writes it after each authoritative tick, and no state intent, replay or wire merge reads it. The camera follows the pawn's position each tick (eye-height offset above capsule center); yaw and pitch remain mouse-driven.

Movement design intent — the custom-kinematic foundation, the declarative author surface, the state-machine seam, and the FPS-flexibility band — lives in `movement.md`. This section covers only the component's place in the entity model.

**Lower-body facing and locomotion states.** A player's third-person body splits at the waist: the upper body faces aim, and the lower body faces travel, folded so it never points behind aim. Travel more than 105° from aim is a backpedal — the legs face against travel, toward aim, and backward states play. Travel within 75° of aim is forward. The band between holds the previous classification, so strafing while the aim sweeps does not flip the legs 180°; a pure strafe keeps whichever way the body was already moving, with about a 90° twist, since strafe clips and blend spaces are deferred. Stopping holds the classification too: a stopped body faces aim regardless, and a delayed presentation still drawing the last of the motion keeps its fold. The upper-body twist therefore stays within 105°. The engine selects player locomotion states by name, and only when the descriptor declares `walk_forward` or `walk`: the descriptor's `defaultState` when still; `walk_forward` (or `walk`) and `walk_backward` at walking speed; `run_forward` (or `run`) and `run_backward` when speed is nearer run pace. The run tier engages above the midpoint of `ground.speed.walk` and `ground.speed.run` plus 15% of their gap, and releases below the midpoint minus 15%; the active state is the memory. The midpoint, not walk speed, is the switch because walking while turning carries ground speed past the walk cap. A backward state that is undeclared, or whose clip did not resolve, plays its own tier's forward state — the visible signal that a backward clip is still owed. There is no cross-tier fallback, except that a run forward state whose clip did not resolve leaves the walk tier covering every speed; `run_backward` needs a run forward state beside it. Each state rate-scales against its own `travelSpeed`. The movement state, not speed, picks the crouch tier, since a crouch-walk and a slow stand-walk travel alike: `Crouching` plays `crouch_idle` when still and `crouch_forward` or `crouch_backward` when moving, never a run state, rate-scaled with `ground.speed.crouch` as the fallback reference; `Sliding` holds `crouch_idle` at rate 1 throughout. A missing or unresolved `crouch_backward` plays `crouch_forward`, and `crouch_idle` falls back to `defaultState`. The tier exists only beside a usable `crouch_forward`; without one, crouching and sliding select as standing does. Crouch states commit direction as walk and run states do; `crouch_idle` commits none. The jump sequence overrides every tier while it runs, and only in `Normal` or `Crouch` movement — swim, vault, dash and slide keep their own selection. A takeoff with the jump spent and vertical velocity above zero plays `jump_up` until the apex or the clip's end, then `jump_loop`; a fall without a jump keeps the ground stride until the pawn has been airborne 0.15 s (9 `air_ticks` at 60 Hz), so step-downs and ground-stick gaps never show a fall pose. An air jump after takeoff keeps `jump_loop`, and a launch without a jump (knockback, a push) falls into `jump_loop` after the delay. A takeoff needs a fresh airborne stretch: past the delay a spent jump still rising — an air dash or vault handing back mid-rise, a teleport arrival into the air, which spends the jump — falls rather than replaying `jump_up`; only a jump out of the water takes off however long the swimmer was off the ground. Airborne overrides posture, but a crouched landing returns to the crouch tier. A standing landing at or below `ground.speed.walk` plays `jump_down` until its clip ends, the pawn speeds past walk, or it jumps again; a faster landing goes straight to the stride. Jump states play at rate 1 and commit no direction. Without a usable `jump_loop` the sequence is absent; a missing `jump_up` takes off into `jump_loop`, and a missing `jump_down` lands straight into locomotion. A one-shot ends against its model clip's length, timed on the animation clock from when the selecting peer saw its motion begin; a clip whose length is unknown counts as played through. The classification and the jump phase are per-peer presentation memory on the mesh animation and never replicate (`networking.md` §Host-side remote-pawn presentation). Each peer's state selector is their only writer; where a peer selects no state for a pawn — a connected client's own pawn — the pose pass keeps the classification instead.

A player pawn is present only when a `player_spawn` entity in the level resolves to a movement descriptor. When no pawn exists, the engine falls back to a fly-camera so maps are navigable without a player descriptor.

**Player spawn points.** A `player_spawn` is a place, never a pawn by itself. Single player and a co-op host materialize exactly one pawn at install, at the first spawn in map order without `coop_only 1` whose `entity_class` resolves (co-op-only spawns are a warned fallback); the host assigns each joiner a free spawn in map order through the seat table (`networking.md` §Slot lifecycle). The selection and the origin shift live in `crates/sim/src/scripting/builtins/player_spawn.rs`. A spawn's canonical origin is the **feet**: the `.map` front end moves `player_spawn` 24 units down to the bottom of its FGD box (Quake's player hull, `size(-16 -16 -24, 16 16 32)`), and spawn raises a movement pawn's transform by `halfHeight + radius` to the capsule centre, as AI placements already rise from feet. Nothing snaps or depenetrates a pawn at spawn, so a box sunk into the floor starts embedded. A `.prl` baked before this convention must be rebaked: un-rebaked spawns sit 24 units deeper and start in the floor.

---

## 7c. Enemy Brain Component

Enemy behavior is an **authored hierarchical statechart**, not an engine-closed state enum. A graph declares named activities and ordered guarded transitions. Activities may be leaves or compose nested graph layers and stateless selector layers. A leaf selects motion, an optional action, and its animation. Transition guards are IR expressions (`scripting.md` §11) over a brain-local binding scope; the engine binds them when the graph is installed and evaluates them every tick.

**Ownership split.** The author owns which activities and layers exist, what each one does, the ordered guards between them, and — through an optional per-graph candidacy predicate — which of the candidates the engine offers are worth engaging. The engine owns everything else: which entities are offered as candidates at all, ranking and retention among the eligible ones, think-stride time-slicing of acquisition, target-switch hysteresis, combat-slot resolution, steering, facing, damage application through the chokepoint, and the aggro gate. Motion and action verbs are closed vocabularies — an activity selects an engine behavior, it does not describe one.

The line between the two halves of targeting is **perceivable vs. worth engaging**. The floor decides what an enemy could possibly perceive — a question with one right answer, so it is correctness. Whether a perceivable pawn is worth attacking is taste: a blind grunt, a psychic boss, and an enemy that ignores the wounded are all valid designs. Candidacy can only narrow the offer set, never widen it.

**One brain authoring surface.** `components.behavior` is the sole enemy-brain authoring surface. The descriptor materializes one retained graph component.

Invariants the evaluator upholds:

- **Guards evaluate every tick.** No activity, animation, or cooldown latches evaluation off. A commitment window is an authored guard over time-in-activity, never an engine mechanism.
- **Transitions evaluate outer-to-inner.** At each level, wildcard (`"*"`) rows precede the active activity's rows, each in declaration order; the first true row wins. A winning edge enters its target and descends through initial activities immediately. Newly entered activities do not evaluate transitions until the next tick.
- **Guards are read-only.** Per-entity state fields are written by impact policies and reactions; guards only read them. That is how a hit-driven reaction and an authored wildcard transition compose without either knowing about the other.
- **Candidacy is per-graph eligibility; disengagement is per-activity policy.** The candidacy predicate is read-only and evaluated once per offered candidate during a ranking scan, never against the target already retained — dropping a retained target is what guards are for. It answers eligibility only, never rank: it produces a boolean, and ranking stays the engine's.
- **Peers are candidates; the engine may rank retaliation over distance.** The offered set includes brain-bearing enemies, not only player pawns, so an enemy can perceive a peer that damaged it. Ranking stays the engine's — candidacy is still boolean and narrowing — but the engine adds a **retaliation** term: a candidate whose recent accumulated damage exceeds an authored per-pair or per-archetype **tolerance** is preferred over the nearest hostile, tuned by authored scalars, never an authored ranking expression. The engine admits such an over-tolerance attacker to candidacy even when sentiment leaves the pair non-hostile — its own widening, not authored candidacy's — but never prices the think stride off it; the stride stays the nearest sentiment-hostile distance. A retaliation-acquired target is engine-held against distance hysteresis until it is lost or a stronger over-tolerance provoker unseats it; standing down otherwise stays an authored guard.
- **Hostility is mutable per-entity state, not a fixed enemy/friend archetype.** The engine floor narrows fresh acquisition by it — a candidate is offered only when the evaluating enemy is hostile toward it — while dropping a target that turns friendly stays authored, per the split above. Hostility resolves from authored content: factions are named manifest data, and a directional per-pair **sentiment** scalar decides it — negative hostile, zero neutral, positive allied — seeded so unauthored content reproduces the prior faction-inequality rule (different faction hostile, same faction neutral) with zero authoring. The durable authored contract is the target-hostility fact a guard reads, not the numeric faction leaf beneath it: that leaf is an index into the sentiment table. The authored scalar is immutable baseline content. The AI reads a live sparse value over that baseline; authored reactions and impact effects move it, and author-tunable decay eases it back. The live value is host-authoritative, persisted, and replicated as state rather than content (`networking.md`).
- **Target selection holds no aliveness policy.** Aliveness gates the attack, not the choice of whom to attack. An enemy may select a corpse and be unable to hit it; disengaging from a downed target is an authored wildcard transition over target-side facts, not an engine rule. The authored death signal is the death sweep's latch rather than a health comparison — a health comparison cannot distinguish a corpse from no target at all, since target-side facts read zero untargeted.
- **Acquisition and stand-down are graph policy.** The engine holds no acquisition leash. A candidacy predicate, including a distance predicate, bounds fresh acquisition only; an ordered transition stands down a retained target. A graph without either policy pursues without limit by design.
- **Motion includes position goals, not only pursuit.** A motion verb may steer toward a fixed point — the enemy's home anchor, or the next point on an authored route — as much as toward the target. The home anchor is the enemy's spawn position, fixed for the brain's life and host-only; placing the entity authors its home, and runtime re-homing is a separate additive path. An activity that can select a position goal cannot declare an action or contain an action-producing layer. A resolved position goal is non-engaged: it holds no target and takes no combat slot, so movement leash and retreat compose as ordinary authored guards over the home-distance fact, not as engine policy.
- **Damage recency does not imply a spatial source.** Every landed hit resets recency, including contextless script damage. A separate latest-hit provenance fact says whether that hit supplied an attacker world position. Authored behavior must conjoin both before using damage recency to interpret shared last-known-position memory or directional bearing. The player's damage bearing takes the same shape — a bearing paired with a source-known flag (decided, not yet built; `networking.md` §Presentation events vs. replicated state).
- **The think stride is cost machinery and shares no data path with authored relevance rules.** Its distance is the raw retained target distance without a new scan, or the raw nearest hostile candidate the engine offers when none is retained. Engine-owned hostility defines that offered set; authored candidacy and guards never filter or clamp its distance. Deriving it from the authored-filtered value inverts the stride — an absent distance reads as due every tick, so the far-band enemy the stride exists to make cheap becomes the one that scans most.
- **Bound guard programs are derived data.** They live in the evaluator, never on the component, so they are never serialized and never affect component equality. They rebuild from the retained graph whenever the entity is seen.
- **Animation is subordinate to graph state.** An unknown animation name warns once at spawn and keeps the prior animation at tick time; it never aborts the tick. A state that pursues without acting is a locomotion state: its animation is a travel cycle, so it yields to the graph's initial-state animation at a standstill. That makes the initial state's animation the graph's rest pose, and authors should pick it accordingly.
- **Death is not a graph transition.** AI and steering exclude a zero-HP enemy as soon as damage settles. The later death sweep owns the death latch and frozen credit; an authored impact policy owns the death animation and despawn delay.

Graph evaluation is host-only. Clients consume replicated animation state and never evaluate guards.

---

## 7d. Ghost Links (decided, not yet built)

Timelines are separate physical room copies joined by seamless teleporters. A **ghost link** lets a copy perceive the other: while armed, its members draw as translucent ghosts at the mapped pose in the other copy, and their sounds can play there. Room copies keep collision, hits, splash, AI perception and triggers isolated without any interaction filter. Ghosts are presentation only: they never collide, block, take hits or touch triggers.

- **Region.** A `ghost_link` brush volume over the source copy names a landmark inside it and a target in the other copy, both teleport destinations. The mapping is the seamless teleport's landmark → target transform, one function teleports and ghosts share, so a ghost and a seamless arrival from the same pose agree. A link is independent of teleporters. A two-way link is two volumes.
- **Eligibility is declared per layer, and links only narrow.**
  - A descriptor's `ghost` attribute marks an archetype that can ghost. It defaults to false for every descriptor, players included: the engine never branches on actor kind. A pawn ghosts with its third-person weapon.
  - A weapon's projectile block carries its own `ghost`, resolved from the firing weapon on every peer, a monster's projectiles included.
  - Movers have no descriptor: a link's `ghost_tags` names them by tag, per link.
  - Each link shows four member categories: players, non-player descriptor entities, projectiles and movers. A category never ghosts what the descriptor or tag did not opt in. The map sets a link's starting categories; scripts change them.
  - An impact belongs to a link when its shot's owner is a member.
  - The viewing player's own pawn, projectiles and sounds never ghost on that viewer's machine.
- **Resolved locally.** Each peer reads `ghost` from its own descriptors by the class each entity was materialized from, and resolves `ghost_tags` against its own movers. No membership fact rides the wire, and peers whose content differs see different ghosts. Per-instance tags on descriptor placements are unusable here: a connected client's copies of them carry no tags or map identity.
- **Per frame, from presented state.** An eligible entity is a member while its link is armed, its category is shown, and its presented point is inside the volume. The ghost draws at the mapped presented pose, so aim, pose modifiers and teleport cuts carry over. A ghost whose membership ends, despawn included, fades out at its last in-volume mapped pose, so a swap never draws a ghost between copies. An entity inside two armed links ghosts through each.
- **Not built in this design:** flipping `ghost` on one entity at runtime, and widening a link past its opt-ins.

Draw, limiter and audio contracts: `rendering_pipeline.md` §7.9, §7.8, `audio.md` §1. Replication: `networking.md` §Current contract. Script surface: `scripting.md` §2, §12.

---

## 8. Particles

Particles are anonymous presentation in a pool outside the registry. A particle has no entity id, no components and no script visibility, and is never replicated. Particle count never changes gameplay tick cost or gameplay spawn capacity.

**Producers.** The emitter bridge, a frame-time owner, writes the pool directly. Producers in the fixed tick or network ingest (impact bursts) queue plain-data requests on registry intake, drained into the pool at frame time. They never touch the pool, and the queue never reaches transport: each peer spawns its own bursts (`networking.md` §Presentation events vs. replicated state). Requests drain in raise order before emitter spawning; both producers feed the same frame's simulation and collection. Level unload and reload clear the pool and pending intake together.

**Simulation.** Rust, once per rendered frame on frame time, after the emitter bridge and before the light bridge: velocity, buoyancy and drag, curve-evaluated size and opacity, spin. Particles stay off the fixed tick; no per-particle script callbacks. Each step integrates position first, then takes gravity and push at the post-step position under the bodies' overlap rule (§7), never fluid. Buoyancy scales gravity; push is the nominal push, unscaled by buoyancy or force. A particle leaves on the step its lifetime ends, and every survivor steps exactly once. Pool order carries no meaning: the billboard pass is order-independent (`rendering_pipeline.md` §7.4).

**Emitter parameters.** A particle takes its emitter's curves, drag, buoyancy, sprite collection and tint at spawn and keeps them for life; a later emitter change reaches only later spawns. Parameters live in shared immutable blocks a particle references by index, never copied per particle. Blocks survive until their last particle expires; a later rewrite mints a new block only when a spawn succeeds. Spin is the one live value: a live emitter's spin rate refreshes its particles, so spin reactions turn existing smoke, and an orphan (emitter despawned) keeps rotating at the last rate. Reusing an entity slot cannot reconnect that orphan to a new emitter.

**Capacity.** A per-emitter cap of 4,096 and a global live budget of 65,536, shared by emitter and impact particles. Past either, new spawns drop with a rate-limited warning; live particles are never evicted. Gameplay registry capacity is independent. Steady-state spawning, stepping and expiry perform no per-particle heap allocation; level clear retains reusable capacities.

**Render collect.** Each rendered frame the collector culls each particle by its own position against the visible cells, buckets by its resolved sprite collection, and hands packed instances to the billboard pass. A conservative frustum pretest may skip a particle only when its whole rotated quad is outside; an offscreen centre alone never proves rejection. Gameplay projectiles remain individually addressed registry entities and share only the billboard collection path.

---

## 9. Non-Goals

- Full ECS (archetype storage, query planner, system scheduler)
- Entity inheritance hierarchies
- Per-entity script lifecycle callbacks (entity types don't have script attachment points; scripts manipulate entities through registered primitives)
- Client-authoritative entity state; deterministic-lockstep replication. Server-authoritative replication owns gameplay entities on the server; clients receive replicated component state.
- Entity serialization (save/load)
- Spatial partitioning for entity-entity queries (octree, grid)
- Physics engine integration (rigid body, joints, constraints)
