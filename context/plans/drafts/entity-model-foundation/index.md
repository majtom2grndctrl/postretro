# Entity Model Foundation (Rust-only)

> **Status:** draft. Ready for refinement once Milestone 6 (sector graph) lands; conceptually independent of M5/M6 and can be drafted earlier.
> **Milestone:** 7 — see `context/plans/roadmap.md`.
> **Depends on:** Milestone 2 (fixed-timestep loop + input snapshot — done), Milestone 6 (sector graph, for entity ↔ sector linkage). Does **not** depend on Milestone 5 (lighting) or Milestone 8 (physics).
> **Unblocks:** Milestone 8 (chunk primitive + physics — needs entity-owned transforms), Milestone 9 (kinematic clusters — driven by `KinematicDriver` entities), Milestone 10 (destruction — triggered by `DamageSource` entities), Milestone 11 (scripting layer — binds this API).
> **Related:** `context/lib/entity_model.md` (architectural spec), `context/lib/index.md` §2 (subsystem boundaries), `context/plans/drafts/entity-types/index.md` (future consumer — post-M11 entity libraries).

---

## Context

The engine has no game logic layer yet. `main.rs` drives the camera directly from input each tick; there is no notion of an entity, no per-tick game state advancement beyond camera motion, no event stream. The lighting stack, sector graph, and renderer all exist, but nothing *lives* in the world.

Milestone 7 establishes that layer — in pure Rust, before any scripting runtime is introduced. The single most important constraint is **scripting-layer exposure**: every public API decided here becomes a scripting contract in Milestone 11. Once a script can call `entity.spawn(...)`, the shape of `spawn` is frozen. Decisions made now propagate to JS/Lua/Rhai bindings later. This plan's job is to make those decisions while the compiler can still catch inconsistencies.

Two reference drivers (`RotatorDriver`, `DamageSource`) validate the API shape and feed directly into M9 and M10 as stub behaviors.

**What this plan is not:** it is not a catalog of game-specific entity types (doors, enemies, pickups). Those are post-M11 scripts. See `drafts/entity-types/` for that future scope.

---

## Goal

Stand up the entity layer described in `context/lib/entity_model.md`:

- Typed entity collections keyed by stable numeric ID.
- Classname registry mapping `.map` entity lump `classname` strings to engine entity types.
- Fixed-timestep update hook in the Game logic stage.
- Parent/child transform hierarchy with inherited world-space transforms.
- Interpolation state surfaced to the renderer.
- Typed event emission/subscription with owned event payloads.
- A scripting-exposure audit that gates progression to Milestone 8.
- Two reference entity types (`RotatorDriver`, `DamageSource`) that prove the surface out.

---

## Design principles (durable)

These are the non-negotiable properties of the public API. The scripting-exposure audit (Task 5) enforces them.

| Property | Rule | Why |
|----------|------|-----|
| **Identity** | Public handles are plain numeric IDs (e.g. `EntityId(u32)` + generation counter), never Rust references or lifetimes. | Scripts cannot hold Rust borrows. IDs survive cross-boundary. |
| **Events** | Event payloads are owned, `Copy`- or `Clone`-friendly structs of primitives, IDs, and small fixed-size types (vectors, enums). No `&` fields, no trait objects, no associated types. | Scripts receive events across the FFI boundary. |
| **Generics** | Public API surface is monomorphic or uses only erasable generics (e.g. `TypeId`-keyed lookup). No `fn foo<T: Trait>(...)` on the public boundary. | Scripts cannot instantiate Rust generics. |
| **Lifetimes** | Zero explicit lifetimes on public signatures. Internal code may borrow freely; the boundary copies or returns IDs. | Scripts have no lifetime concept. |
| **Mutation** | Scripts mutate via commands (`move_to`, `destroy`, `emit`), not by holding `&mut` to entity data. | Enforces the ownership invariant from `entity_model.md` §6. |
| **Classname registry** | Classnames are registered once at startup; registration returns a type token used at spawn time. Unknown classnames log and skip, never panic. | Matches `entity_model.md` §4. Scripts will register their own classnames in M11. |

A public API that forces a scripting binding author to invent workarounds is a failure of this plan.

---

## Task 1 — Typed entity collections

**Crate:** `postretro` · **New module:** `src/game/entity/` (parent module of this plan's code) · **Entry point:** `src/game/mod.rs` added to `main.rs`.

### Collection storage

1. **One concrete storage type per entity type.** `RotatorDriver` lives in `Vec<RotatorDriverSlot>`; `DamageSource` lives in `Vec<DamageSourceSlot>`. No `Vec<Box<dyn Entity>>`. Follows the "typed collections over heterogeneous lists" invariant in `entity_model.md` §1.

2. **Stable numeric IDs with generation counter.** `EntityId { index: u32, generation: u32 }`. Destroyed slots are retained in the vector with their generation bumped; reused on next spawn of that type. Dangling IDs fail lookup cleanly rather than aliasing a newer entity. The id is the *only* public handle to an entity.

3. **Per-type + global registry.** Each collection exposes `spawn`, `get`, `destroy`. A top-level `World` struct owns all collections and provides `lookup(EntityId) -> Option<EntityRef>` where `EntityRef` is a small enum dispatching to the right collection. The classname registry (Task 2) wires classname strings to the correct `spawn` path.

4. **Common transform block.** Every entity slot carries the common spatial state from `entity_model.md` §2: position, orientation (quaternion), scale, velocity, sector membership (forward-compatible with M6 — entity currently stores a `SectorId` but the sector graph wiring is a separate integration step), bounding volume. Embedded directly in each type's slot — no base struct.

### Query surface

5. **Typed iteration.** `world.rotator_drivers().iter()` returns a typed iterator over live slots. Dead slots are skipped.

6. **No heterogeneous query.** There is deliberately no `world.query::<Position>()` or component-style query. If game logic needs to touch "all damage-dealing things," it iterates the specific collections.

---

## Task 2 — Classname registry

**Input side:** `.map` entity lump → compiler. **Output side:** PRL entity section → engine.

### Compile-time

7. **Entity lump survives compilation.** `postretro-level-compiler` currently parses entities and discards all but light entities (see `parse.rs` ~lines 98–155). This task adds a new PRL section (`EntityLump` or similar — name to finalize in refinement) that stores the full key-value list for every entity the compiler doesn't strip. Each entry: `classname: String`, `origin: Vec3`, `angles: Vec3`, `properties: Vec<(String, String)>` (remaining keys). Keep the payload text-ish and simple — at M7 we prioritize iteration speed over wire-format compactness.

8. **Lights are still consumed by the lighting stage.** The new section is additive — the lighting pipeline continues to consume light entities at compile time. A future refactor may unify them, but not in this plan.

### Runtime

9. **Registry as a hashmap of factories.** `ClassnameRegistry: HashMap<&'static str, EntityFactory>` where `EntityFactory: fn(&EntitySpawnArgs, &mut World) -> EntityId`. Registration happens during engine startup, once, before any level loads. Each reference entity type (`RotatorDriver`, `DamageSource`) registers one classname.

10. **Spawn flow.** On level load: iterate the entity lump, look up each `classname` in the registry. Hit → call factory with parsed args. Miss → log warning, skip (per `entity_model.md` §4).

11. **Key-value parsing helpers.** A small parser crate-internally converts the string `properties` into typed values (floats, vectors, ints, enums) with default-on-malformed semantics (logged warning, default value). Factories use these helpers rather than re-implementing string parsing per type.

---

## Task 3 — Lifecycle + update hook

### Fixed-timestep integration

12. **`World::tick(dt, input_snapshot)` called from the game logic stage.** Currently `main.rs` has a single `for _ in 0..ticks { ... }` loop that advances the camera. Refactor: the body of that loop becomes `self.world.tick(tick_dt, &snapshot)` (camera motion temporarily stays inline; moving the player to a proper entity type is M12's job). Frame order (`Input → Game logic → Audio → Render → Present`) is preserved.

13. **Update order per `entity_model.md` §5.** Player-type entities tick before all others. Within non-player types, order is stable but unspecified. For M7 there is no player entity yet — the ordering machinery is scaffolded but only has one "bucket" of non-player entities.

14. **Deferred destruction.** `destroy(id)` sets a pending-destroy flag. After all updates complete, a sweep pass actually frees slots and bumps generations. This prevents iteration invalidation mid-tick, per `entity_model.md` §3.

### Parent/child transforms

15. **Optional `parent: Option<EntityId>` on every entity's transform block.** Children's world-space transform = parent's world transform composed with the child's local transform. Orphaned parents (destroyed before children) are handled by detaching children at destroy time — children inherit their last resolved world transform as a new local, then set `parent = None`. Cycles are rejected at `set_parent` time with a logged error and no-op.

16. **World-space resolution is a per-tick pass.** After all entity updates emit their local transform mutations, a resolve pass walks from roots down and fills in world transforms. Downstream consumers (renderer, event system position lookups) read world transforms only.

### Interpolation state

17. **Renderer-facing interpolation buffer.** For every entity with a visible representation (none yet in M7, but the slot exists), the world stores the previous tick's world transform alongside the current one. The renderer lerps between them using the frame timing interpolation factor. This extends the pattern already used for the camera (`frame_timing.push_state(...)` in `main.rs`).

18. **Non-visible entities skip interpolation.** `RotatorDriver` and `DamageSource` have no sprite or mesh in M7, so they skip the previous-transform push. The buffer is opt-in per entity type.

---

## Task 4 — Event system

### Shape

19. **Events are owned structs.** Each event type is a plain struct: `DamageEvent { source: EntityId, target: EntityId, amount: f32, kind: DamageKind }`. No `&` fields, no lifetimes, no trait objects.

20. **Event type registry.** Event types are registered once at startup (like classnames). Each gets a `TypeId`-based key internally; the scripting layer will map these to string names in M11 without changing the Rust API.

21. **Emission.** Entities call `world.emit(event)` during their tick. Events land in a per-tick queue.

22. **Subscriptions are classname- or ID-scoped.** Subscribers register: "I want all `DamageEvent`s where `target` has classname X," or "…where `target == specific id`." No broadcast to every entity — subscription scoping avoids O(N·M) dispatch at retro-scale entity counts.

23. **Dispatch after updates.** After the update pass and before the resolve pass (or after — to finalize in refinement), queued events are dispatched to matching subscribers. Subscribers may emit further events; bounded iteration (e.g. 4 passes max) prevents runaway cascades. The bound is logged on trip and treated as a data bug, not runtime recovery.

### Scripting compatibility

24. **Subscribers are closures *internally* only.** The public subscription API takes a `fn(&EventCtx) -> ()` or equivalent plain-function pointer — not a closure that captures Rust state. Script subscribers in M11 resolve to a script function identifier; Rust-side subscribers in M7 register plain functions. This constraint is the one most likely to surface in the scripting-exposure audit (Task 5) if implementation drifts; watch for it.

---

## Task 5 — Scripting-exposure audit

This is a gate, not an implementation task. Before declaring M7 complete:

25. **Enumerate the public API.** Walk every `pub fn`, `pub struct`, `pub enum` in `src/game/entity/` and its submodules. Produce a list — the PR description should include this list verbatim.

26. **Apply the rules from the Design principles table.** For each item, verify: no lifetimes on the signature, no non-erasable generics, no `&`-containing event payloads, IDs not references in argument positions, etc. Any violation is a fix, not a documented exception.

27. **Draft a scripting-binding sketch.** A one-page sketch showing what each public API looks like when called from a generic script-language perspective (pseudo-code). If any API cannot be expressed cleanly, rework the Rust surface now. This sketch goes in the PR description and then into `context/lib/entity_model.md` as a new section (`§ Scripting-facing surface`) so M11 has a reference.

28. **Sign off as a gate.** The PR does not merge until the audit is complete. This is the cheapest moment to catch API shape bugs.

---

## Task 6 — Reference entity types

Two concrete types exercise the whole stack.

### `RotatorDriver`

29. **Classname:** `game_rotator_driver`. FGD entry added in `assets/postretro.fgd`. Properties: `rate_yaw` (deg/s, default 30), `rate_pitch` (deg/s, default 0), `rate_roll` (deg/s, default 0), `targetname` (string, optional — for M9 integration: the cluster whose transform this entity drives).

30. **Behavior.** Each tick, advances its own orientation by `rates × tick_dt`. Emits a `TransformEvent { source: EntityId, transform: Transform }` that downstream consumers (kinematic clusters in M9) will subscribe to. In M7, nothing consumes the event — the test is that it fires at the fixed tick rate.

### `DamageSource`

31. **Classname:** `game_damage_source`. Properties: `amount` (f32, default 10.0), `kind` (enum: `bullet | explosion | melee`, default `bullet`), `targetname` (entity id reference or classname filter — finalize in refinement).

32. **Behavior.** Wired to a debug keybind (see §Debug hooks below). On keypress, emits a `DamageEvent` to the resolved target. In M7, nothing consumes the event — the test is that subscribed observers receive it with correct fields.

### Debug hooks

33. **A single debug keybind** (e.g. F7) triggers any `DamageSource` entities in the world to emit their event. Registered via the existing input action system. Gated behind a debug feature flag (`cargo run` exposes it; release builds strip it).

---

## Integration with existing systems

| System | Interaction |
|--------|-------------|
| **Input** (`src/input/`) | Read-only: `World::tick` receives the input snapshot. No entity owns input state directly; the future player entity (M12) will read from the snapshot passed in. |
| **Renderer** (`src/render/`) | No changes in M7. Interpolation state structure is added but unused (no visible entities). Renderer continues to borrow the camera transform only. |
| **Audio** (`src/`) | No audio subsystem exists yet. Event types are sized to accommodate audio triggers later; the emit/subscribe pattern is audio-ready. |
| **Frame timing** (`src/frame_timing.rs`) | `World::tick` plugs into the same fixed-timestep accumulator as the current camera advance. No changes to the timing module. |
| **Level compiler** (`postretro-level-compiler/`) | New PRL section for the entity lump. `parse.rs` stops discarding non-light entities. |
| **PRL format** (`postretro-level-format/`) | New section type. Follows the existing section table pattern. |

---

## Acceptance Criteria

1. `cargo test --workspace` passes. Unit tests cover: ID generation/generation-bump correctness; destroy-during-iteration deferral; parent/child transform composition; parent destruction detaches children cleanly; classname registry hit/miss/malformed-property paths; event subscription scoping (classname-scoped and ID-scoped); event dispatch bound enforcement.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. A test map with a `game_rotator_driver` entity loads. Logged tick output confirms the entity advances orientation at the fixed tick rate (not the frame rate). Unknown classnames log a warning and do not crash.
5. A test map with a `game_damage_source` entity plus a subscriber (test-only plain-function subscriber) logs the correct `DamageEvent` payload when the debug keybind fires.
6. Scripting-exposure audit (Task 5) complete. The API enumeration and scripting-binding sketch are included in the PR description. `context/lib/entity_model.md` updated with the `§ Scripting-facing surface` reference section.
7. `context/lib/entity_model.md` §4 updated: the "BSP entity lump" language is replaced with the PRL entity section. The key-value parsing contract documented here is summarized there.
8. `assets/postretro.fgd` extended with `game_rotator_driver` and `game_damage_source` entries.

---

## Out of scope

- Any concrete game entity type beyond the two reference drivers (player, enemy, door, pickup, trigger, projectile). Those are post-M11 scripts — see `drafts/entity-types/`.
- Entity-entity collision. Bounding volumes are stored but no collision pass runs in M7. (M8 adds the chunk collider broadphase.)
- Physics, rigidbodies, velocity integration beyond simple `position += velocity × dt` (if used at all — the reference drivers don't need it).
- Scripting runtime, bindings, hot reload. All M11.
- Spatial queries beyond sector membership lookup. No octree, no grid broadphase — `entity_model.md` §8 Non-Goals.
- Save/load serialization of entity state. Out of scope project-wide.
- Networked entity replication. Out of scope project-wide.
- Renderer-visible entity types (sprites, billboards, meshes). M8 chunk primitive is the right seam for visible entities.
- Player entity implementation. Deferred to M12 (player movement).
- Audio event consumers. Deferred until the audio subsystem exists.

---

## Key decisions to make during refinement

- Exact shape of `EntityId` (u32+u32 vs. packed u64; generation width).
- Whether `World` is a single struct with all collections as fields, or a `HashMap<TypeId, Box<dyn AnyCollection>>`. The former is simpler and more Rust-idiomatic; the latter is more modder-friendly for M11. Leaning toward the former with a small dispatch enum — revisit in refinement.
- Event dispatch timing: after updates + before transform resolve, vs. after resolve. Affects whether subscribers see pre- or post-hierarchy world transforms.
- Event subscription API shape: builder (`.on::<DamageEvent>().with_classname(...).call(fn)`) vs. flat registration function. Builder reads better; flat is simpler to bind.
- PRL entity section wire format details (string table vs. inline strings; endianness consistent with existing sections).
- Whether `game_damage_source.targetname` resolves at level load (eager) or each emit (lazy). Lazy is more flexible for entities that spawn at runtime; eager is faster and catches typos at load.
- How `SectorId` on entities is populated and kept fresh. Depends on M6 landing details; a stub that always returns `SectorId(0)` is acceptable for M7 and revisited when sector graph runtime queries exist.
