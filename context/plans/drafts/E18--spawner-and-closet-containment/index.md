# E18-C — Spawner Entity + Closet Containment

## Goal

Give co-op set-pieces the monster-closet payload: an engine-owned `entity_spawner` that a
trigger reaction fires to spawn enemies mid-session, replicated to clients including late
joiners. Add a dormant-Brain gate so pre-placed reveal-closet enemies do not aggro through
walls or path through closed doors before the reveal. Spawn-flavor is the capstone's critical
path; reveal-flavor containment rides the same trigger surface a future door-open will share.

## Scope

### In scope

- `entity_spawner` point entity via the **generic classname-dispatch path** (like `prop_mesh` /
  `player_spawn`): a built-in classname handler, an engine-owned `SpawnerComponent`, an FGD
  entry. No new PRL section, no compiler branch, no loader change.
- `spawnFromSpawner` consequential reaction primitive (tag-targeted), executed VM-free in the
  fixed-tick seam. Spawns `count` enemies of a named archetype at the spawner origin; the
  existing entity-entity separation pass nudges them apart.
- Spawn-time replication registration: a new `DescriptorSpawnPath::RuntimeSpawn`, a broadened
  networked-AI predicate, and a host-gated post-tick registration sweep so spawned enemies enter
  the `ReplicableSet` and stamp a `NetworkId`. Clients materialize them from the snapshot's
  `entity_class`.
- Dormant-Brain containment: a `dormant` Brain flag seeded by a `start_dormant` enemy-placement
  KVP; a dormant brain does not acquire targets, does not steer, and holds position. A
  `wakeEnemies` consequential verb (tag-targeted) clears it. Both new verbs compose in one
  reaction (e.g. `[moverStart door, wakeEnemies closet]`).
- Pre-attack windup: a spawned enemy cannot attack for a minimum window ≥ the netcode
  interpolation clamp + the standard net profile's delivery budget, so it is visible on clients
  before it can hit. The windup doubles as jump-scare animation time.
- Full primitive-contract surface for both new verbs (SDK TS + Luau builders, typedef
  regeneration + drift snapshot + parity fixtures, validators).

### Out of scope

- **Area/encounter-scoped kill progress** ("clear the ambush" reactions that count spawned
  kills). Deferred to a sibling spec — see Open questions. C only ensures spawned kills do not
  corrupt existing install-scoped `ProgressTracker` totals.
- Trap pools / seeded arming and any gameplay RNG — E18-D. Spawn order here is deterministic.
- Agent-vs-mover awareness (steering ignoring movers) — a broader unowned gap; dormancy covers
  only the pre-reveal window. Noted, not owned.
- Multi-point spawn patterns, per-fire spawn arguments, spawning non-AI archetypes.
- A wire field for dormancy — a dormant enemy replicates as a stationary Idle enemy via its
  existing Transform; no new wire surface.
- Spawn-at-activator targeting (`@activators`) — depends on the still-in-`ready/`
  `E18--trigger-event-params`; not required here.

## Acceptance criteria

- [ ] An `entity_spawner` placed in a `.map` with an archetype name and count compiles and loads
      with no new PRL section; the built map's binary layout is unchanged from a map without it.
- [ ] Firing a reaction containing `spawnFromSpawner` targeting the spawner's tag creates `count`
      live enemies of the named archetype at the spawner, on the host, inside the fixed tick.
- [ ] A spawner referencing an unknown archetype name warns once at level install and spawns
      nothing at fire time (inert degradation); a spawner whose archetype is not an AI enemy
      warns and spawns nothing.
- [ ] In a two-peer co-op session, a host-spawned enemy appears on the connected client and on a
      client that joins after the spawn, driven by its replicated Transform.
- [ ] A spawned enemy cannot deal damage until at least the interpolation-clamp + delivery-budget
      window has elapsed since it spawned, asserted against the E15 reference `LinkConfig`.
- [ ] An enemy placed with `start_dormant` does not acquire or move toward a player standing
      adjacent (including through a thin wall) and does not leave its start position.
- [ ] A reaction containing `wakeEnemies` targeting the dormant enemy's tag makes it acquire and
      pursue a player in range on the next think tick; the same reaction can also drive a mover.
- [ ] A dormant enemy still takes damage and can be killed; killing it fires no target-selection
      or steering behavior.
- [ ] Killing a spawned enemy does not change any install-scoped kill-progress total (no
      over-count, no underflow).
- [ ] SDK TS and Luau emit byte-identical descriptors for `spawnFromSpawner` and `wakeEnemies`;
      the typedef drift check passes after regeneration.

## Tasks

### Task 1: `entity_spawner` entity class + `SpawnerComponent`

Add a built-in classname handler for `entity_spawner` mirroring `prop_mesh`
(`crates/postretro/src/scripting/builtins/prop_mesh.rs`): a `CLASSNAME` const, a handler
registered into `ClassnameDispatch` via `register_builtins`, riding the generic
`MapEntityRecord` point-entity path — **no** dedicated PRL section, compiler branch, or loader
change. The handler reads KVPs for the enemy archetype `canonical_name` and integer `count`
(absent/empty archetype → `[Loader]` warning, spawner spawns nothing; tags/KVPs are applied
uniformly by `apply_classname_dispatch`, so the handler need not set them). Introduce an
engine-owned `SpawnerComponent` (the component vocabulary is engine-closed — this is fine)
carrying the resolved enemy archetype and count. Resolve the archetype name to an
`EntityTypeDescriptor` at install via `find_descriptor` (matches `canonical_name`) and store what
the in-tick spawn needs so the fixed-tick executor never touches the data registry or a
`ScriptCtx` — carry the resolved descriptor (or an equivalent registry-reachable handle) on the
component. Add the FGD `@PointClass` entry (`entity_spawner`, archetype + count keys). Enemy
archetypes are any descriptor with an `ai` block; do not invent a modder-facing enemy component.

### Task 2: `spawnFromSpawner` verb + in-tick spawn executor

Add `spawnFromSpawner` to `CONSEQUENTIAL_PRIMITIVES` and a `Spawn { target: BoundTarget }`
variant to `BoundTriggerCommand` (and the `#[cfg(test)] BoundTriggerCommandKind`) in
`crates/postretro/src/trigger_bindings.rs`. Map the primitive in `partition_direct_reaction` and
`bind_sequence_step`; the target is a `BoundTarget::Tag` (config lives on `SpawnerComponent`, no
per-fire args). Put the spawn logic in a **new** `crates/postretro/src/spawner.rs` module (mirror
`kinematic_mover.rs`'s `apply_mover_command_to_targets` + `register_mover_reaction_primitives`
shape) so `trigger_bindings.rs` gains only the variant and a delegating match arm in
`execute_non_store`. The executor resolves spawner entities by tag, and for each with a
`SpawnerComponent` spawns `count` enemies through `registry.spawn(transform)` +
`attach_descriptor_components`, stamping `DescriptorProvenance.spawn_path =
DescriptorSpawnPath::RuntimeSpawn` (add this variant to
`crates/entities/src/provenance.rs`). Enemies spawn at the spawner origin; the existing
entity-entity separation pass resolves interpenetration. Spawn is host-side, in-tick, in
deterministic order, with no RNG. Register the primitive through a
`register_spawner_reaction_primitives` entry point (mirroring the mover registration call site in
`session/mod.rs`) so the descriptor/validation path accepts the verb.

### Task 3: Spawn-time replication registration + client materialization

In `crates/postretro/src/netcode/descriptor_class.rs`, broaden `is_networked_ai_map_enemy` to
accept `spawn_path ∈ {MapPlacement, RuntimeSpawn}` (Brain+Agent still required) and rename it to
`is_networked_ai_enemy`, updating both call sites (`descriptor_entity_class` and the host
registration sweep) and the scripting-side classifier agreement test. This makes
`descriptor_entity_class` stamp a spawned enemy's `canonical_name` so the client materializes its
presentation from a Transform-only record (else it is an invisible/untyped ghost). Add a
host-gated post-tick registration pass mirroring `host_register_map_enemies`
(`crates/postretro/src/netcode/replication.rs:239`, idempotent — re-registration keeps the
`NetworkId`): after the fixed tick, sweep for networked-AI enemies not yet in the `ReplicableSet`,
`allocator.stamp` + `ReplicableSet::register` each, and track them in the same `map_enemies` set
so level reload unregisters them. Gate the sweep on host/single-player role exactly as
`host_register_map_enemies_after_install` does, and skip it when no spawn occurred that tick.
Single-player spawns work with no registration (sweep is a no-op off-host).

### Task 4: Dormant-Brain gate + `wakeEnemies` verb

Add a `dormant: bool` field to `BrainComponent` (`crates/entities/src/components/brain.rs`),
default `false`. Seed it from a `start_dormant` boolean KVP on an AI-enemy map placement, read
where `attach_descriptor_components` applies placement overrides. In
`run_ai_tick_with_navigation` (`crates/postretro/src/scripting/systems/ai.rs`), skip acquisition
and steering for a dormant brain: it does not run `evaluate_transition`, does not steer, holds its
spawn position and state, and its Transform is untouched (so it replicates as a stationary Idle
enemy). Damage, health, and death still process — a dormant enemy can be killed. Add `wakeEnemies`
as a second consequential verb: `CONSEQUENTIAL_PRIMITIVES` entry, `BoundTriggerCommand::Wake {
target: BoundTarget }` (+ `Kind`), partition/bind mapping, an `execute_non_store` arm that clears
`dormant` on every tagged Brain, and a `register_*_reaction_primitives` registration. It is
tag-targeted and composes in a multi-step reaction with `moverStart` (one reveal reaction opens
the door and wakes the closet). Spawn-flavor enemies are never dormant (they spawn after the
reveal); dormancy is reveal-flavor only.

### Task 5: SDK builders, typedefs, validators for both verbs

Add TS and Luau builders for `spawnFromSpawner(tag)` and `wakeEnemies(tag)` under `sdk/lib/`,
emitting byte-identical wire descriptors (follow the shipped consequential-verb builders —
`armTrigger`/`disarmTrigger`/mover commands). Extend the typedef templates
(`crates/scripting-core/src/typedef/templates/`), regenerate `postretro.d.ts` / `.d.luau`, update
the drift snapshot, and add TS/Luau parity fixtures. Update the reaction-argument validators to
accept both verbs and reject malformed targets, matching the shipped per-primitive warn-skip.
Extend the `world.query` filter vocabulary if a spawner/enemy filter term is needed for authoring.

### Task 6: Pre-attack windup AC + kill-progress guard

Enforce a minimum pre-attack windup on a spawned enemy: it cannot deal damage until at least the
adaptive interpolation clamp (≤ 250 ms) plus the standard net profile's delivery budget has
elapsed since spawn, so remote clients see it before it can hit. Harness-assert this at the E15
reference `LinkConfig` (assert at the standard profile, not universally). Separately, guard
kill-progress: a `RuntimeSpawn` enemy's death must not mutate any install-scoped `ProgressTracker`
total (`crates/scripting-core/src/reaction_dispatch.rs` — totals are counted once at install).
Exclude `RuntimeSpawn` enemies from that counting so spawned kills neither over-count a fixed
total nor underflow. Do not build area-scoped progress here (deferred sibling spec).

## Sequencing

**Phase 1 (sequential):** Task 1 — the spawner entity + component; everything spawns through it.
**Phase 2 (sequential):** Task 2 — the verb, executor, and `RuntimeSpawn` provenance variant; consumes Task 1's component and edits `trigger_bindings.rs`.
**Phase 3 (concurrent):** Task 3 (replication), Task 4 (dormancy + wake), Task 6 (windup + progress) — disjoint files; 3 and 6 consume Task 2's spawn path, 4 is independent. Task 4 edits `trigger_bindings.rs`, which Task 2 already finished, so no in-phase conflict.
**Phase 4 (sequential):** Task 5 — SDK/typedef contract; consumes the final verb set from Tasks 2 and 4.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Spawner class | `entity_spawner` handler | n/a (no PRL section) | n/a | n/a | `entity_spawner` (`@PointClass`) |
| Spawn verb | `BoundTriggerCommand::Spawn` | reaction descriptor `"spawnFromSpawner"` | `"spawnFromSpawner"` | `"spawnFromSpawner"` | n/a |
| Wake verb | `BoundTriggerCommand::Wake` | reaction descriptor `"wakeEnemies"` | `"wakeEnemies"` | `"wakeEnemies"` | n/a |
| Spawner component | `SpawnerComponent` | not replicated (host-local) | n/a | n/a | n/a |
| Runtime spawn path | `DescriptorSpawnPath::RuntimeSpawn` | `"runtime_spawn"` (serde, local only) | n/a | n/a | n/a |
| Dormancy | `BrainComponent.dormant` | not on the wire | n/a | n/a | `start_dormant` (bool, enemy placement) |
| Archetype ref | `SpawnerComponent` archetype | n/a | n/a | n/a | archetype `canonical_name` key |
| Spawn count | `SpawnerComponent` count | n/a | n/a | n/a | integer `count` key |

## Rough sketch

- **Spawner entity (Task 1).** `prop_mesh.rs` is the exact template — `CLASSNAME`, a handler
  returning a spawned id with a component attached, registered in `ClassnameDispatch::register_builtins`,
  tags/KVPs applied by `apply_classname_dispatch`. `find_descriptor(descriptors, name)` resolves
  the archetype at install; `descriptor_materializes_ai_enemy(d) == d.ai.is_some()` validates it
  is an enemy.
- **In-tick spawn (Task 2).** `execute_non_store` (`trigger_bindings.rs:409`) matches
  `BoundTarget::Tag` via `resolve` (`:469`, `query_by_component_and_tag(Transform, tag)`); the
  `Spawn` arm delegates to `spawner::spawn_from_spawner_targets`. Spawn = `registry.spawn(transform)`
  (`registry.rs:744`) + `attach_descriptor_components` (`data_archetype.rs:389`) with `spawn_path =
  RuntimeSpawn`. Keep the descriptor reachable without `ScriptCtx` (resolved onto `SpawnerComponent`
  at install).
- **Replication (Task 3).** `is_networked_ai_map_enemy` (`descriptor_class.rs:31`) currently gates
  on `MapPlacement`; broaden to `{MapPlacement, RuntimeSpawn}` + rename → `descriptor_entity_class`
  (`:61`) then stamps the class for spawned enemies. Post-tick sweep mirrors
  `host_register_map_enemies` (`replication.rs:239`), which is idempotent
  (`replication.rs:487-493`); register via `ReplicableSet::register` (`:47`) + `allocator.stamp`.
  Pawn-accept template: `on_slot_accepted` (`netcode/lifecycle.rs:132`).
- **Dormancy (Task 4).** Gate in `run_ai_tick_with_navigation` (`ai.rs:504`) before
  `evaluate_transition` (`ai.rs:248`, Idle→Alert at `:257-259` is XZ-distance, no LOS — the reason
  a sealed enemy aggros through walls). `LogicalState` is Idle/Alert/Attack/Death (`brain.rs:29`).
- **Contract tax.** Two verbs each touch `CONSEQUENTIAL_PRIMITIVES`, `BoundTriggerCommand` (+Kind),
  `partition_direct_reaction`/`bind_sequence_step`, a `register_*_reaction_primitives` call, SDK
  TS+Luau, typedef templates + drift + parity, validators.
- **Oversized-file note (soft):** `trigger_bindings.rs` (1377), `ai.rs` (1001), `registry.rs`
  (1611), `reaction_dispatch.rs` (1361) exceed ~800 lines. Additions here are localized (enum
  variant + list entry + match arm; one Brain field + one tick guard), and `trigger_bindings.rs`
  is concurrently reshaped by `ready/E18--trigger-event-params`, so a split-first task is **not**
  recommended — it would collide. Spawn logic lands in a new `spawner.rs`, not by growing
  `trigger_bindings.rs`.

## Script syntax examples

```ts
// Spawn-flavor closet: a plate fires a reaction that spawns the ambush.
defineReaction("springAmbush", [
  spawnFromSpawner("closet_a"),   // tag on the entity_spawner(s)
]);

// Reveal-flavor closet: one reaction opens the door AND wakes the pre-placed enemies.
defineReaction("revealCloset", [
  moverStart("closet_door"),
  wakeEnemies("closet_a"),        // clears `dormant` on tagged brains
]);
```

```
// TrenchBroom: the spawner point entity and a pre-placed dormant enemy.
{ "classname" "entity_spawner" "archetype" "grunt" "count" "3" "_tags" "closet_a" }
{ "classname" "grunt" "start_dormant" "1" "_tags" "closet_a" }
```

## Open questions

- **Area/encounter-scoped kill progress → own spec.** "Clear the ambush" needs progress counting
  that survives runtime spawns (install-time `ProgressTracker` totals are fixed). Owner's call:
  spin a dedicated sibling spec (working name **E18-C-progress**, or fold into E18-D/E). C only
  guards against corruption. Confirm the sibling's home before promotion.
- **Wake verb vs. arm/disarm reuse.** Research §4.4 frames dormancy as "driven by the same
  arm/disarm verbs." This spec adds a dedicated `wakeEnemies` instead, because `armTrigger`/
  `disarmTrigger` are trigger-scoped and overloading them onto Brain tags muddies both. Flag if
  the owner prefers overloading arm/disarm.
- **`start_dormant` KVP vs. descriptor default.** Seeding dormancy per-placement (KVP) keeps the
  archetype reusable as both dormant and active. Confirm KVP over a descriptor-level flag.
