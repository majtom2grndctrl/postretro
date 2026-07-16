# E18-C — Spawner Entity + Closet Containment

## Goal

Give co-op set-pieces the monster-closet payload: an engine-owned `entity_spawner` that a
trigger reaction fires to spawn enemies mid-session, replicated to clients including late
joiners. Add an aggro gate so pre-placed reveal-closet enemies do not aggro through walls or
path through closed doors before the reveal. Spawn-flavor is the capstone's critical path;
reveal-flavor containment reuses the shipped arm/disarm surface a door-open already rides.

## Scope

### In scope

- `entity_spawner` point entity via the **generic classname-dispatch path** (like `prop_mesh` /
  `player_spawn`): a built-in classname handler, an engine-owned `SpawnerComponent`, an FGD
  entry. No new PRL section, no compiler branch, no loader change.
- `spawnFromSpawner` consequential reaction primitive (tag-targeted), executed VM-free in the
  fixed-tick seam. Each fire spawns `count` enemies of a named archetype at the spawner origin,
  facing the spawner's authored `angles`, offset by a fixed per-index delta so they do not overlap.
  The spawner is stateless — it never self-re-triggers; repeated spawns come only from repeated
  firing events, and one-shot behavior is owned by the firing trigger's policy.
- Spawn-time replication registration: a new `DescriptorSpawnPath::RuntimeSpawn`, a broadened
  networked-AI predicate, and a host-gated post-tick registration sweep so spawned enemies enter
  the `ReplicableSet` and stamp a `NetworkId`. Clients materialize them from the snapshot's
  `entity_class`.
- Spawner-archetype presentation: an archetype referenced only by a spawner (no pre-placed
  instance) has its mesh model uploaded and animation clips resolved on host and client, so a
  spawned enemy renders as its model rather than a debug capsule.
- Closet containment via an **aggro gate**: a Brain whose aggro gate is closed does not acquire
  targets, does not steer, and holds position. Authored closed with `enabled_on_spawn = false` on
  the enemy placement (the shipped arm/disarm input), reopened by `armTrigger` — the verb triggers
  already use, extended to enemy tags. The script-arm gate is one aggro condition among several;
  later perception/AI specs add more (LOS, sound) that compose with it, not replace it. A reveal
  reaction opens door and gate together: `[moverStart door, armTrigger closet]`.
- Pre-attack windup: a spawned enemy cannot attack until at least the netcode interpolation clamp
  (≤ 250 ms) after it spawns, so it is delivered and drawn on remote clients before it can hit. The
  windup doubles as jump-scare animation time.
- Full primitive-contract surface for the one new verb `spawnFromSpawner` (SDK TS + Luau builders,
  typedef regeneration + drift snapshot + parity fixtures, validators), plus extending
  `armTrigger` / `disarmTrigger` targeting and validation to enemy aggro gates.

### Out of scope

- **Area/encounter-scoped kill progress** ("clear the ambush" reactions that count spawned
  kills) — its own spec; it carries deep questions (regional cells, regional BVH) that do not
  belong here. C only ensures spawned kills do not corrupt install-scoped `ProgressTracker`
  totals.
- Trap pools / seeded arming and any gameplay RNG — E18-D. Spawn order here is deterministic.
- Agent-vs-mover awareness (steering ignoring movers) — a broader unowned gap; the aggro gate
  covers only the pre-reveal window. Noted, not owned.
- Multi-point spawn patterns, per-fire spawn arguments, spawning non-AI archetypes.
- A wire field for the aggro gate — a gated enemy replicates as a stationary Idle enemy via its
  existing Transform; no new wire surface.
- Spawn-at-activator targeting (`@activators`) — depends on the still-in-`ready/`
  `E18--trigger-event-params`; not required here.
- A new `world.query` filter term for spawners or enemies — authoring does not need one in C.
- Runtime spawn-point validation (clear-of-geometry, on-navmesh) — spawn validity is an authoring
  responsibility, exactly as for map-placed enemies; the runtime trusts the authored placement. A
  spawner in a wall is an authoring bug for playtest and the dev overlay, not a runtime guard.

## Acceptance criteria

- [ ] An `entity_spawner` placed in a `.map` with an archetype name and count compiles and loads
      adding no new PRL `SectionId`; it rides the existing map-entities section (section inventory
      unchanged).
- [ ] Firing a reaction containing `spawnFromSpawner` targeting the spawner's tag creates `count`
      live enemies of the named archetype at the spawner, on the host, inside the fixed tick.
- [ ] A spawner referencing an unknown archetype name warns once at level install and spawns
      nothing at fire time (inert degradation); a spawner whose archetype is not an AI enemy
      warns and spawns nothing.
- [ ] Firing `spawnFromSpawner` against the same spawner twice spawns `2 × count` enemies — the
      spawner is stateless and never self-exhausts.
- [ ] Spawned enemies face the spawner's authored `angles` direction.
- [ ] A `count` of 0 (or a malformed value) spawns nothing and warns; registry exhaustion
      mid-batch spawns what fits and warns, without panicking.
- [ ] In a two-peer co-op session, a host-spawned enemy appears and renders its mesh on the
      connected client and on a client that joins after the spawn, driven by its replicated
      Transform.
- [ ] A spawner whose archetype has no pre-placed instance still renders its spawned enemies as
      their model (mesh uploaded, clips resolved) on host and client — not a debug capsule.
- [ ] A spawned enemy deals no damage for at least the interpolation clamp (≤ 250 ms) after spawn;
      at the E15 reference `LinkConfig` (`mandated_link`) it is present and drawable on the remote
      client before its first attack lands.
- [ ] An enemy placed `enabled_on_spawn = false` does not acquire or move toward a player standing
      adjacent (including through a thin wall) and does not leave its start position.
- [ ] A reaction containing `armTrigger` targeting that enemy's tag makes it acquire and pursue a
      player in range on the next think tick; the same reaction can also drive a mover.
- [ ] A gated (un-armed) enemy still takes damage and can be killed; killing it fires no
      target-selection or steering behavior.
- [ ] A spawned enemy carries no progress tags, so killing it changes no install-scoped
      kill-progress total (no over-count, no underflow).
- [ ] SDK TS and Luau emit byte-identical descriptors for `spawnFromSpawner`; the typedef drift
      check passes after regeneration.

## Tasks

### Task 1: `entity_spawner` entity class + `SpawnerComponent`

Add a built-in classname handler for `entity_spawner` mirroring `prop_mesh`
(`crates/postretro/src/scripting/builtins/prop_mesh.rs`): a `CLASSNAME` const, a handler
registered into `ClassnameDispatch` via `register_builtins`, riding the generic
`MapEntityRecord` point-entity path — **no** dedicated PRL section, compiler branch, or loader
change. The handler reads KVPs for the enemy archetype name and integer `count` (absent/empty
archetype → `[Loader]` warning, spawner spawns nothing) and attaches an engine-owned
`SpawnerComponent { archetype_name, count }`; it spawns via `try_spawn(transform, &entity.tags)`
so the spawner carries its `_tags` (the KVP bag is applied uniformly by `apply_classname_dispatch`,
but tags attach through `try_spawn`, exactly as `prop_mesh` does). `SpawnerComponent` is
engine-owned (the component vocabulary is engine-closed — this is fine). Resolving the archetype
name to an `EntityTypeDescriptor` cannot happen in the handler — `ClassnameHandler` is a bare
`fn(&MapEntity, &mut EntityRegistry) -> Option<EntityId>` with no descriptor list in scope. Do it
in a **separate post-dispatch install pass** where descriptors are in scope (alongside
`apply_data_archetype_dispatch`): for each `SpawnerComponent`, `find_descriptor` its
`archetype_name` (matches `canonical_name`), warn per AC 3 if missing or not an AI enemy
(`descriptor_materializes_ai_enemy`), and fill the component with the resolved descriptor so the
fixed-tick executor never touches the data registry or a `ScriptCtx`. `EntityTypeDescriptor`
(`crates/entities/src/data_descriptors/types/entity.rs`) is carryable on the component. Add the
FGD `@PointClass` entry (`entity_spawner`, archetype + count keys, plus the standard `angles` key
for facing). Enemy archetypes are any descriptor with an `ai` block; do not invent a modder-facing
enemy component.

### Task 2: `spawnFromSpawner` verb + in-tick spawn executor

Add `spawnFromSpawner` to `CONSEQUENTIAL_PRIMITIVES` and a `Spawn { target: BoundTarget }`
variant to `BoundTriggerCommand` (and the `#[cfg(test)] BoundTriggerCommandKind`) in
`crates/postretro/src/trigger_bindings.rs`. The single primitive→command mapping point is
`bind_command`'s match (`partition_direct_reaction` and `bind_sequence_step` route into it); add
the `Spawn` arm there. The target is a `BoundTarget::Tag` (config lives on `SpawnerComponent`, no
per-fire args). Also add `spawnFromSpawner` to `is_trigger_consequential_primitive`
(`crates/scripting-core/src/reaction_dispatch.rs`), the hardcoded mirror of the fixed-tick command
set that backs the double-execution debug asserts. Put the spawn logic in a **new**
`crates/postretro/src/spawner.rs` module (mirror `kinematic_mover.rs`'s
`apply_mover_command_to_targets` + `register_mover_reaction_primitives` shape) so
`trigger_bindings.rs` gains only the variant and a delegating match arm in `execute_non_store`.
The executor resolves spawner entities by tag, and for each with a `SpawnerComponent` spawns
`count` enemies through `registry.spawn(transform)` + `attach_descriptor_components`
(module-private today — bump to `pub(crate)`; it takes an `&MapEntity`, so synthesize a minimal
one as the `defaultWeapon` literal path does), stamping `DescriptorProvenance.spawn_path =
DescriptorSpawnPath::RuntimeSpawn` (add this variant to `crates/entities/src/provenance.rs`).
Spawned enemies are **untagged** — this is deliberate (it keeps them out of tag-keyed kill-progress
counting, Task 6). Place the `count` enemies at the spawner origin plus a fixed per-index offset
so they do not perfectly overlap (do not rely on the steering separation pass, which may not run
for idle agents). Spawn is host-side, in-tick, in deterministic order, with no RNG. Register the
primitive through a `register_spawner_reaction_primitives` entry point (mirroring the mover
registration call site in `session/mod.rs`) so the descriptor/validation path accepts the verb.

The spawner is **stateless**: each fire spawns `count`, it holds no fired/exhausted flag, and it
never re-triggers itself — re-firing is an ordinary event outcome, and one-shot behavior (a closet
that springs once) is owned by the firing trigger's `once`/rearm policy, not the spawner. Spawned
enemies take the spawner entity's Transform rotation (its authored `angles`, via `rotation_quat()`)
so they face as placed. Degrade gracefully, matching `prop_mesh`: parse `count` with
warn-and-default-0 on a missing or malformed value; if `try_spawn` returns `None` mid-batch (the
registry at `u16::MAX`), spawn what fits, warn once, and stop — never panic. Do not validate spawn
positions against geometry or the navmesh — a spawner inside a wall is an authoring bug for
playtest/dev-overlay, consistent with map-placed enemies. Because the trigger system runs between
movement and AI in the tick, a spawned enemy exists before this tick's AI pass and may take its
first steering step the frame it appears; the windup gates only its first attack, not its first
step.

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
so level reload unregisters them. Run the sweep every tick — it is a full idempotent sweep with
stale-prune (re-registration keeps the `NetworkId`), so an unconditional per-tick call is correct;
a dirty-flag optimization is a later concern, not a correctness requirement, and no
spawn-happened signal is threaded out of the executor today. Gate the sweep on host/single-player
role exactly as `host_register_map_enemies_after_install` does. Single-player spawns work with no
registration (sweep is a no-op off-host).

### Task 4: Enemy aggro gate + arm/disarm targeting

Add an aggro gate to `BrainComponent` (`crates/entities/src/components/brain.rs`) — a boolean gate
(working name `aggro_armed`, default open) that, while closed, blocks target acquisition. Seed it
closed from an `enabled_on_spawn = false` KVP on an AI-enemy map placement, read where
`attach_descriptor_components` consumes placement KVPs. Note this is a **new** bare-KVP read: on a
trigger volume `enabled_on_spawn` is a compiler-validated wire field, but on an enemy placement it
is a plain KVP off `MapEntity.key_values` — parse it with `parse_bool` (absent or malformed → warn
and default the gate open). This is a deliberate exception to the convention that bare (non-`initial_`)
KVPs are not consumed by components; call it out in a comment. In `run_ai_tick_with_navigation`
(`crates/postretro/src/scripting/systems/ai.rs`), a gated brain does not run `evaluate_transition`,
does not steer, and holds its current position and state — its Transform is untouched, so it
replicates as a stationary Idle enemy; ensure the O(n²) steering separation pass also skips a gated
agent so it is not nudged. Damage, health, and death still process — a gated enemy can be killed.
The gate seed is not a `DescriptorMapOverride` (that enum is closed to light/emitter), so ensure a
descriptor hot-reload re-applies it, or document reload-reopens-the-gate as a dev-only limitation.
Extend the shipped `armTrigger` /
`disarmTrigger` verbs so that when their tag resolves to AI-enemy entities they open / close the
aggro gate (alongside any same-tagged triggers): `arm_trigger_targets` / `disarm_trigger_targets`
(`crates/postretro/src/trigger_system.rs`) gain the Brain-gate toggle. No new consequential verb —
this reuses their shipped dispatch, so a reveal reaction composes `[moverStart door, armTrigger
closet]` to open the door and the gate in one fire. The aggro gate is one condition among several:
later perception/AI specs add LOS and sound gates that compose additively (acquisition requires the
gate open; new conditions narrow, never replace, it). Spawn-flavor enemies spawn with the gate open
— they appear after the reveal.

### Task 5: SDK builder, typedefs, validators

Add TS and Luau builders for `spawnFromSpawner(tag)` under `sdk/lib/`, emitting byte-identical wire
descriptors (follow the shipped consequential-verb builders — `armTrigger`/`disarmTrigger`/mover
commands). Extend the typedef templates (`crates/scripting-core/src/typedef/templates/`), regenerate
`postretro.d.ts` / `.d.luau`, update the drift snapshot, and add TS/Luau parity fixtures. Update the
reaction-argument validators to accept `spawnFromSpawner` and reject malformed targets, matching the
shipped per-primitive warn-skip. `armTrigger`/`disarmTrigger` already carry builders and typedefs —
extend only their target-validation to admit enemy-tag targets. No `world.query` filter change (see
out of scope).

### Task 6: Pre-attack windup + kill-progress guard

Enforce a minimum pre-attack windup on a spawned enemy by seeding its existing
`attack_cooldown_remaining_ms` (`crates/entities/src/components/brain.rs`, counted down in `ai.rs`)
at spawn time in `spawner.rs` to at least the adaptive interpolation clamp (≤ 250 ms), so a spawned
enemy cannot land its first attack before it is delivered and drawn on remote clients. Harness-assert
the property at the E15 reference `LinkConfig` (`mandated_link`): a freshly spawned enemy is present
and drawable on the client before it deals damage — assert at that profile, not universally. There is
no separate "delivery budget" constant; the interpolation clamp is the pinned floor and the harness
observes the delivered-before-hit property. Kill-progress needs no counting change here: spawned
enemies are untagged (Task 2), and `ProgressTracker::on_entity_killed` matches only by tag, so a
spawned kill can never touch an install-scoped total — add a test asserting this and do not build
area-scoped progress (deferred sibling spec).

### Task 7: Spawner-archetype model upload + clip resolve (host + client)

Runtime-spawned enemies must render, not fall back to a debug capsule. Today the renderer model
cache and animation-clip indices are built by **level-load-only** passes scoped to the archetypes
the map references *by placement*: the host model sweep (`distinct_mesh_models`,
`crates/postretro/src/main.rs`) and clip resolve (`resolve_mesh_entity_clips`, same file), and on
the connected client the suppressed-AI upload (`suppressed_ai_enemy_mesh_models`,
`crates/postretro/src/scripting/builtins/data_archetype.rs`, scoped to classes the map references).
An archetype referenced only by an `entity_spawner` KVP matches none of them, so its mesh never
uploads and its clips never resolve. At install, collect every spawner-referenced archetype (from
the resolved `SpawnerComponent`s of Task 1) and union its `mesh.model` handle into both upload
sweeps (host and client) and resolve its clips, so a spawner-only archetype is drawable before its
first spawn. The client already has a post-materialization resolve precedent for net-remote enemies
(`materialize_net_remote_enemy_presentation` + clip resolve, `main.rs`); reuse its shape rather than
inventing a per-spawn upload path.

## Sequencing

**Phase 1 (sequential):** Task 1 — the spawner entity + component + install-time archetype resolution; everything spawns through it.
**Phase 2 (sequential):** Task 2 — the verb, executor, and `RuntimeSpawn` provenance variant; consumes Task 1's resolved component and edits `trigger_bindings.rs`.
**Phase 3 (concurrent):** Task 3 (replication), Task 4 (aggro gate + arm/disarm), Task 6 (windup + progress), Task 7 (presentation upload) — files are disjoint: 3 = netcode (`descriptor_class.rs`/`replication.rs`), 4 = `ai.rs`/`brain.rs`/`trigger_system.rs`, 6 = `spawner.rs` + harness, 7 = `main.rs` presentation sweeps + the client upload pass in `data_archetype.rs`. 3/6/7 consume Task 2's spawn path; 4 is independent.
**Phase 4 (sequential):** Task 5 — SDK/typedef contract; consumes the final verb set from Tasks 2 and 4.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Spawner class | `entity_spawner` handler | n/a (no PRL section) | n/a | n/a | `entity_spawner` (`@PointClass`) |
| Spawn verb | `BoundTriggerCommand::Spawn` | reaction descriptor `"spawnFromSpawner"` | `"spawnFromSpawner"` | `"spawnFromSpawner"` | n/a |
| Spawner component | `SpawnerComponent` | not replicated (host-local) | n/a | n/a | n/a |
| Runtime spawn path | `DescriptorSpawnPath::RuntimeSpawn` | `"runtime_spawn"` (serde, local only) | n/a | n/a | n/a |
| Aggro gate | `BrainComponent` aggro gate | not on the wire | n/a | n/a | `enabled_on_spawn` (bool, enemy placement) |
| Aggro arm/disarm | `armTrigger` / `disarmTrigger` (extended to Brain tags) | shipped descriptors | shipped | shipped | n/a |
| Archetype ref | `SpawnerComponent` archetype | n/a | n/a | n/a | archetype `canonical_name` key |
| Spawn count | `SpawnerComponent` count | n/a | n/a | n/a | integer `count` key |

## Rough sketch

- **Spawner entity (Task 1).** `prop_mesh.rs` is the exact template — `CLASSNAME`, a handler
  returning a spawned id with a component attached, registered in `ClassnameDispatch::register_builtins`;
  KVPs applied by `apply_classname_dispatch`, tags via `try_spawn(transform, &entity.tags)`
  (`prop_mesh.rs:65`). Archetype resolution is a post-dispatch install pass, not the handler:
  `find_descriptor(descriptors, name)` (`data_archetype.rs:249`); `descriptor_materializes_ai_enemy(d)
  == d.ai.is_some()` (`:293`) validates it is an enemy.
- **In-tick spawn (Task 2).** `bind_command`'s match (`trigger_bindings.rs:697`) is the single
  primitive→`BoundTriggerCommand` mapping point. `execute_non_store` (`:409`) matches `BoundTarget::Tag`
  via `resolve` (`:469`, `query_by_component_and_tag(Transform, tag)`); the `Spawn` arm delegates to
  `spawner::spawn_from_spawner_targets`. Spawn = `registry.spawn(transform)` (`registry.rs:744`) +
  `attach_descriptor_components` (`data_archetype.rs:389`, bump to `pub(crate)`; synthesize its
  `&MapEntity` as the `defaultWeapon` literal does, `:752`) with `spawn_path = RuntimeSpawn`, untagged.
  Keep the descriptor reachable without `ScriptCtx` (resolved onto `SpawnerComponent` at install).
- **Presentation (Task 7).** Model upload + clip resolve are level-load-only and placement-scoped
  (`distinct_mesh_models` / `resolve_mesh_entity_clips` in `main.rs`; client
  `suppressed_ai_enemy_mesh_models`, `data_archetype.rs:354`). Union spawner-referenced archetypes
  into both, else the spawned enemy draws as a debug capsule (the E10 AC#3 regression documented at
  `data_archetype.rs:343-349`).
- **Replication (Task 3).** `is_networked_ai_map_enemy` (`descriptor_class.rs:31`) currently gates
  on `MapPlacement`; broaden to `{MapPlacement, RuntimeSpawn}` + rename → `descriptor_entity_class`
  (`:61`) then stamps the class for spawned enemies. Post-tick sweep mirrors
  `host_register_map_enemies` (`replication.rs:239`), which is idempotent
  (`replication.rs:487-493`); register via `ReplicableSet::register` (`:47`) + `allocator.stamp`.
  Pawn-accept template: `on_slot_accepted` (`netcode/lifecycle.rs:132`).
- **Aggro gate (Task 4).** Gate in `run_ai_tick_with_navigation` (`ai.rs:504`) before
  `evaluate_transition` (`ai.rs:248`, Idle→Alert at `:257-259` is XZ-distance, no LOS — the reason
  a sealed enemy aggros through walls). `armTrigger`/`disarmTrigger` extend
  `arm_trigger_targets`/`disarm_trigger_targets` (`trigger_system.rs`) with a Brain-gate branch.
  `LogicalState` is Idle/Alert/Attack/Death (`brain.rs:29`).
- **Contract tax.** One new verb, `spawnFromSpawner`, touches `CONSEQUENTIAL_PRIMITIVES`,
  `BoundTriggerCommand` (+Kind), `partition_direct_reaction`/`bind_sequence_step`, a
  `register_*_reaction_primitives` call, SDK TS+Luau, typedef templates + drift + parity,
  validators. `armTrigger`/`disarmTrigger` are already all of that — Task 4 only adds a Brain-gate
  branch to their runtime target resolution.
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

// Reveal-flavor closet: one reaction opens the door AND arms the pre-placed enemies.
defineReaction("revealCloset", [
  moverStart("closet_door"),
  armTrigger("closet_a"),         // opens the aggro gate on tagged brains
]);
```

```
// TrenchBroom: the spawner point entity and a pre-placed, gate-closed enemy.
{ "classname" "entity_spawner" "archetype" "grunt" "count" "3" "_tags" "closet_a" }
{ "classname" "grunt" "enabled_on_spawn" "0" "_tags" "closet_a" }
```
