# E18 — Enemy group handle + aggro gate

## Goal

Establish the enemy-group authoring handle and deliver its first capability: an engine aggro
gate. A set-piece addresses a whole enemy group by tag and mutates it at fire time — the first
typed handle for the fire-time-tag resolution model that `damage(tag)` pioneered as a bare
function. The gate is the handle's first mutable field: `aggro` suppresses a Brain's target
acquisition until a set-piece opens it — the first of several aggro conditions (LOS, sound land in
later perception specs and compose additively). First consumer: E18-C (spawner + closet
containment), co-designed with this spec.

## Selector / descriptor model

Two selector/descriptor resolution models exist in the SDK. A selector fills a reaction
descriptor's binding-key slot; which key it fills implies when the engine resolves it.

| Model | Selector | Resolves | Descriptor key | Members |
|---|---|---|---|---|
| **setup-id** | `world.query({ component, tag? })` | entity ids at level install | `id` (per-entity `SequenceStep`) | movers, triggers, lights, fog, emitters |
| **fire-time-tag** | `enemies({ tag })` | the live tagged set at each fire | `tag` (`PrimitiveReactionDescriptor`) | enemies (this spec); `damage(tag)`; spawner (E18-C) |

Enemies are the fire-time-tag model's first typed handle. Two reasons put them there. **Group
semantics** (always true): a set-piece addresses a whole closet by one tag — one descriptor,
resolved against the live set, like `damage(tag)` — not an array of per-entity id-steps.
**Deferred existence** (forthcoming): a group spawned at reveal time has no id at install, so only
a tag can address it. E18-C's own reveal enemies are pre-placed and *do* exist at install; the
consumers that make fire-time resolution load-bearing are spawn-and-hold waves and the
area-progress reader, both of which tag their spawns. Either way, enemies are **not** a
`world.query` component: it returns per-entity id handles — the wrong shape for a group mutation —
and folding a fire-time return into it would fracture its setup-id contract.

**Handle is sugar.** `enemies({ tag }).update({ aggro: true })` emits the same
`PrimitiveReactionDescriptor` an author could hand-write
(`{ primitive: "updateEnemyState", tag, args: { aggro: true } }`). The handle adds naming, zero
binding semantics; the generic descriptor stays permanently valid as the escape hatch. **Both ends
are typed object partials.** The selector `{ tag }` says which enemies (grows: `faction`, `within`);
the mutation `{ aggro }` says what to change (grows: `hostility`, forced `target`). Select by a
filter object, mutate by an update object — each extensible by key, each invisible to existing
author code when a key is added. The direction is the value (`aggro: true` releases, `false`
holds), so there is no gate verb to name.

## Scope

### In scope

- **Aggro gate on `BrainComponent`** — a boolean gate (working name `aggro_armed`, default open).
  While closed: no target acquisition, no steering, holds current position and `LogicalState`;
  Transform untouched, so it replicates as a stationary Idle enemy. Damage, health, and death still
  process — a gated enemy is killable.
- **Gate seeding** from an `enabled_on_spawn = false` KVP on an AI-enemy map placement, read where
  `attach_descriptor_components` consumes placement KVPs. This is a **new bare-KVP read**: on a
  trigger volume `enabled_on_spawn` is a compiler-validated wire field, but on an enemy placement it
  is a plain KVP off `MapEntity.key_values` — parse with `parse_bool` (absent or malformed → warn,
  default the gate open). A deliberate exception to the convention that bare (non-`initial_`) KVPs
  are not consumed by components; call it out in a comment.
- **`updateEnemyState` consequential primitive** (tag-keyed) — carries a **typed partial** of
  enemy-state fields (today one key, `aggro?: bool`), bound at level install to a closed command,
  executed VM-free in the fixed tick, resolved at fire time against the live tagged Brain set (a
  group spawned after install is reachable once it carries the tag). Each present key mutates the
  matched brains; `aggro` sets `aggro_armed`. The partial is **typed per key** — not a
  `{ field, value }` bag, so fields with different value types stay distinct — and it is bounded to
  **consequential, authored enemy-state fields** (AI/Brain knobs). A distinct enemy verb — **not**
  an overload of `armTrigger`/`disarmTrigger` (that coupling is false-DRY: they split the moment the
  concerns diverge). Effect-based dispatch: consequential class (mutates replicated sim state).
- **SDK enemy-group handle** — `enemies(filter: { tag?: string })` → `EnemyGroup`, with one method,
  `update(fields: { aggro?: boolean })`, returning a single `PrimitiveReactionDescriptor`
  (`{ primitive: "updateEnemyState", tag, args: fields }`; the `damage(tag)` shape — tag-targeted
  primitives ride the `Primitive` reaction path, never a `sequence`). The `update`-args type **is**
  the contract surface; new fields are additive keys.
- **Full primitive-contract surface** for `updateEnemyState`: `CONSEQUENTIAL_PRIMITIVES`,
  `BoundTriggerCommand` (+ `Kind`), the `bind_command` arm, the `is_trigger_consequential_primitive`
  mirror (`crates/scripting-core/src/reaction_dispatch.rs`, the hardcoded debug-assert twin of the
  fixed-tick command set), a `register_*_reaction_primitives` call, SDK TS + Luau builders, the
  `update`-args schema in the typedef templates + drift snapshot + parity fixtures, validators.

### Out of scope

- **Future `update` fields and action verbs.** More state keys — `hostility` / faction (a
  Gravelord-style enemy→ally turn), forced `target` (the E10 `select_target` seam) — are additive
  keys on `update`, not built here. Lifecycle **actions** — `despawn` / `kill` (E18-R respawn +
  player-leave) — are one-shots in a different dispatch class and land as discrete methods, not
  `update` keys. Both out of scope; the boundary is the point: `update` mutates authored
  enemy-state fields, actions and combat effects do not ride it.
- **The read / query face** — group alive/kill count → shared slot for arena-clear gates. Belongs
  to the area-scoped-progress spec (deep questions: regional cells, regional BVH). This spec ships
  the command face only.
- **Additional aggro conditions** — LOS cone, sound gates. Later perception specs; they narrow
  acquisition (compose with the gate), never replace it. See `context/research/enemy-aggro-model.md`.
- **A wire field for the gate** — a gated enemy replicates as a stationary Idle enemy via its
  existing Transform; no new wire surface. `aggro_armed` is host-authoritative sim state; its
  behavioral effect replicates through the motion clients already observe.
- **Enemy id-addressed selection via `world.query`** — enemies are fire-time-tag by nature;
  `world.query` stays the setup-id device model (movers/triggers/lights/fog/emitters).
- **The spawner entity, `spawnFromSpawner`, and the closet set-piece** — E18-C, the consumer. This
  spec is the enemy handle and its aggro gate only.

## Acceptance criteria

- [ ] An enemy placed `enabled_on_spawn = false` does not acquire or move toward a player standing
      adjacent (including through a thin wall) and does not leave its start position.
- [ ] Firing a reaction whose body is `enemies({ tag }).update({ aggro: true })` (or the raw
      `{ primitive: "updateEnemyState", tag, args: { aggro: true } }`) makes a tagged enemy acquire
      and pursue an in-range player on the next think tick.
- [ ] `update({ aggro: false })` re-closes the gate and clears the enemy's steering destination, so
      a mid-chase enemy stops and holds; repeated same-value updates are idempotent (no oscillation,
      no panic).
- [ ] A gated (closed) enemy still takes damage and can be killed; killing it fires no
      target-selection or steering behavior.
- [ ] **Handle is sugar:** `enemies({ tag: "closet_a" }).update({ aggro: true })` deep-equals
      `{ primitive: "updateEnemyState", tag: "closet_a", args: { aggro: true } }`.
- [ ] An `updateEnemyState` whose tag resolves to no Brain is a `debug`-logged no-op, never a panic,
      and coexists with other reactions in the same trigger fan-out (e.g. a `moverStart` reaction).
      Empty match is legitimate for fire-time-tag (the group may be unspawned or fully killed) — the
      `applyDamage` empty-set precedent, not a warn.
- [ ] **Fire-time resolution:** an enemy created *after* install (spawned directly through the
      registry in a headless test) carrying the tag is affected by a later `updateEnemyState` fire —
      the property that distinguishes fire-time-tag from setup-id resolution.
- [ ] A gated enemy (destination cleared) is not nudged: with a second agent inside
      `SEPARATION_RADIUS_FACTOR`, the gated agent's position is unchanged across ticks — the
      idle-settle path exempts a destination-less agent from separation.
- [ ] SDK TS and Luau emit byte-identical `updateEnemyState` descriptors; the typedef drift check
      passes after regeneration.
- [ ] No new wire surface: a gated enemy's snapshot carries no gate field; it replicates via its
      existing Transform as a stationary Idle enemy (snapshot shape unchanged from a non-gated
      stationary enemy).

## Tasks

### Task 1: Aggro gate on `BrainComponent` + FSM / steering gating + seeding

Add the gate field to `BrainComponent` (`crates/entities/src/components/brain.rs`) — a boolean
(working name `aggro_armed`, default open). In `run_ai_tick_with_navigation`
(`crates/postretro/src/scripting/systems/ai.rs`), a gated brain does not run `evaluate_transition`,
does not steer, and holds its current position and `LogicalState` (Idle/Alert/Attack/Death,
`brain.rs`) — its Transform is untouched, so it replicates as a stationary Idle enemy. Gate only
the not-dead FSM/steering block: the tick's every-tick zero-HP → Death transition, death countdown,
and despawn stay live, or a killed gated enemy never despawns. Closing the gate clears the agent's
steering destination (the `SteeringIntent::Clear` path): a destination-less agent takes the
idle-settle early-continue in `agent_steering.rs` and receives no separation push, so no separate
separation-pass guard is needed — and a re-closed mid-chase enemy stops rather than coasting to a
stale destination. Damage, health, and death paths are untouched — a gated enemy can be killed.
Seed the gate closed from an `enabled_on_spawn = false` KVP on an AI-enemy placement, read where
`attach_descriptor_components` (`data_archetype.rs`) consumes placement KVPs; `parse_bool`, bare-KVP
exception comment, absent/malformed → warn and default open. The seed is not a
`DescriptorMapOverride` (that enum is closed to light/emitter), so ensure a descriptor hot-reload
re-applies it, or document reload-reopens-the-gate as a dev-only limitation.

### Task 2: `updateEnemyState` consequential primitive + Brain-tag executor

Add one consequential verb `updateEnemyState` carrying a typed partial `{ aggro?: bool }` (one key
today; the args type grows by key). Register it in `CONSEQUENTIAL_PRIMITIVES`, add a `bind_command`
arm (`crates/postretro/src/trigger_bindings.rs`) → `BoundTriggerCommand::UpdateEnemyState { aggro:
Option<bool> }` (`crates/postretro/src/trigger_commands.rs`), and add it to the
`is_trigger_consequential_primitive` mirror (`crates/scripting-core/src/reaction_dispatch.rs`)
backing the double-execution debug asserts. The fixed-tick executor (`execute_non_store`) resolves
the tag against the live **Brain** set — `query_by_component_and_tag(Brain, tag)`, not the
Transform-keyed `BoundTarget::resolve` — and applies each present field to the matched brains
(`aggro` → `aggro_armed`). The app-drain arm (`register_*_reaction_primitives`) receives targets
pre-resolved by Transform (`reaction_dispatch.rs`), so its handler filters to Brain-bearing
entities. An empty tag resolution is a `debug`-logged no-op, not a warn — for fire-time-tag an empty
match is legitimate (the group may be unspawned or fully killed); matches the shipped `applyDamage`
empty-set behavior. A distinct Brain-targeted command, **not** an extension of the trigger-mutation
chokepoint (`apply_trigger_mutation_to_targets`) — enemy state and trigger arming are separate
concerns. No wire surface: the fields are host-side sim state; their effect reaches clients through
the enemy's existing Transform replication.

### Task 3: SDK `enemies({ tag }).update({ ... })` handle + typedefs + validators + parity

Add the enemy-group selector and handle under `sdk/lib/` — a new `sdk/lib/entities/enemies.ts`
exporting `enemies(filter: { tag?: string }): EnemyGroup`, wrapped into the SDK surface
(`sdk/lib/index.ts` / `prelude.ts` alongside `damage`). `EnemyGroup` carries one method,
`update(fields: { aggro?: boolean })`, returning a single `PrimitiveReactionDescriptor`
(`{ primitive: "updateEnemyState", tag, args: fields }`) — the `damage(tag)` builder is the template
(`sdk/lib/data_script.ts:204`). Mirror in Luau. Extend the typedef templates
(`crates/scripting-core/src/typedef/templates/`) with the `update`-args type, regenerate
`postretro.d.ts` / `.d.luau`, update the drift snapshot, add TS/Luau parity fixtures. Validators
accept `updateEnemyState`, reject an unknown key or a mistyped field, and treat an empty partial as
a no-op. No `world.query` filter change — the enemy selector is its own fire-time-tag path.

## Sequencing

**Task 1 → Task 2 → Task 3**, sequential. Task 2 mutates the `aggro_armed` field Task 1 defines;
Task 3 regenerates typedefs from the verb Task 2 registers. Task 1 and Task 2 are engine
(`brain.rs`/`ai.rs`/`agent_steering.rs`/`data_archetype.rs`, then
`trigger_bindings.rs`/`trigger_commands.rs`); Task 3 is `sdk/` + typedef templates — disjoint from
1–2, but ordered after 2 for the verb contract.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Aggro gate | `BrainComponent` `aggro_armed` | not on the wire | n/a | n/a | `enabled_on_spawn` (bool, enemy placement) |
| Enemy update verb | `BoundTriggerCommand::UpdateEnemyState { aggro: Option<bool> }` | reaction descriptor `"updateEnemyState"` | `"updateEnemyState"` | same | n/a |
| Enemy selector | `query_by_component_and_tag(Brain, tag)` | n/a | `enemies({ tag })` | same | n/a |
| Enemy handle | n/a | n/a | `EnemyGroup.update({ aggro })` | same | n/a |

## Rough sketch

- **Gate + FSM (Task 1).** Gate in `run_ai_tick_with_navigation` before `evaluate_transition`
  (Idle→Alert is XZ-distance, no LOS — the reason a sealed enemy would otherwise aggro through
  walls). Closing the gate clears the steering destination; a destination-less agent takes the
  idle-settle path and gets no separation push, so `agent_steering.rs` needs no gate-aware edit.
  Seed where `attach_descriptor_components` reads placement KVPs.
- **Verb (Task 2).** `bind_command`'s match is the single primitive→`BoundTriggerCommand` mapping
  point (`partition_direct_reaction` / `bind_sequence_step` route into it unchanged);
  `execute_non_store`'s existing arms resolve `BoundTarget::Tag` via `resolve` (Transform-keyed).
  The `UpdateEnemyState` arm bypasses `resolve` and queries Brains by tag, applying each present
  field. One new verb touches `CONSEQUENTIAL_PRIMITIVES`, `BoundTriggerCommand` (+`Kind`),
  `bind_command`'s match, the `is_trigger_consequential_primitive` mirror, a
  `register_*_reaction_primitives` call, then the Task 3 SDK/typedef surface.
- **Handle (Task 3).** `damage()` (`data_script.ts:204`) is the template: a pure builder returning a
  tag-keyed `PrimitiveReactionDescriptor`. `enemies({ tag }).update(fields)` bakes the filter's tag
  and the partial into that descriptor. Object-filter selector and typed-partial update both grow by
  key, with no signature break.
- **Oversized-file note (soft):** `ai.rs` exceeds ~800 lines; additions here are one Brain field +
  one tick guard. Verb logic is localized to the `trigger_bindings.rs` / `trigger_commands.rs` arms.

## Script syntax examples

```ts
// A sealed reveal-closet: enemies placed gate-closed in TrenchBroom, released on a plate.
// A reaction body is one primitive, and a tag-targeted primitive can't ride a sequence,
// so the door and the release are two reactions the trigger fan-out fires together.
export const openClosetDoor = defineReaction("openClosetDoor",
  { primitive: "moverStart", tag: "closet_door" });

export const releaseCloset = defineReaction("releaseCloset",
  enemies({ tag: "closet_a" }).update({ aggro: true }));

// The plate fires both reveal reactions together.
onTriggerEvent({ tag: "reveal_plate" }, "enter", [openClosetDoor, releaseCloset]);

// The handle is sugar — this hand-written body is byte-identical:
defineReaction("releaseCloset",
  { primitive: "updateEnemyState", tag: "closet_a", args: { aggro: true } });
```

```
// TrenchBroom: a pre-placed enemy that starts gate-closed (contained until the reveal).
{ "classname" "grunt" "enabled_on_spawn" "0" "_tags" "closet_a" }
```
