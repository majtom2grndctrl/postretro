# E18 — Enemy group handle + aggro gate

## Goal

Establish the enemy-group authoring handle and deliver its first capability: an engine aggro
gate. Enemy set-pieces address **groups that may be spawned at reveal time** — they have no entity
id at level install — so the handle is tag-addressed and resolved at each fire, not id-resolved at
setup. This is the first typed handle for the fire-time-tag resolution model that `damage(tag)`
pioneered as a bare function. Its first verb, the aggro gate, suppresses a Brain's target
acquisition until a set-piece opens it — the first of several aggro conditions (LOS, sound land in
later perception specs and compose additively). First consumer: E18-C (spawner + closet
containment), co-designed with this spec.

## Selector / descriptor model

Two selector/descriptor resolution models exist in the SDK. A selector fills a reaction
descriptor's binding-key slot; which key it fills implies when the engine resolves it.

| Model | Selector | Resolves | Descriptor key | Members |
|---|---|---|---|---|
| **setup-id** | `world.query({ component, tag? })` | entity ids at level install | `id` (per-entity `SequenceStep`) | movers, triggers, lights, fog |
| **fire-time-tag** | `enemies({ tag })` | the live tagged set at each fire | `tag` (`PrimitiveReactionDescriptor`) | enemies (this spec); `damage(tag)`; spawner (E18-C) |

Enemies are the fire-time-tag model's first typed handle. The model is forced, not chosen: a
revealed/spawned enemy has no id at install, so its selector cannot resolve ids at setup — the tag
is the durable handle, resolution deferred to fire time. This is also why enemies are **not** a
`world.query` component: folding a fire-time-tag return into `world.query` would fracture its
setup-id contract.

**Handle is sugar.** `enemies({ tag }).releaseAggro()` emits the same `PrimitiveReactionDescriptor`
an author could hand-write (`{ primitive: "aggroGate", tag, args: { open: true } }`). The handle
adds naming and grouping, zero binding semantics; the generic descriptor stays permanently valid as
the escape hatch. The selector is an **object filter** (`{ tag }`), not a positional string, so it
grows in lockstep with the descriptor's binding key — future filter fields (faction, area) are
additive and invisible to existing author code.

## Scope

### In scope

- **Aggro gate on `BrainComponent`** — a boolean gate (working name `aggro_armed`, default open).
  While closed: no target acquisition, no steering, holds current position and `LogicalState`;
  Transform untouched (replicates as a stationary Idle enemy); skipped by the O(n²) steering
  separation pass so it is not nudged. Damage, health, and death still process — a gated enemy is
  killable.
- **Gate seeding** from an `enabled_on_spawn = false` KVP on an AI-enemy map placement, read where
  `attach_descriptor_components` consumes placement KVPs. This is a **new bare-KVP read**: on a
  trigger volume `enabled_on_spawn` is a compiler-validated wire field, but on an enemy placement it
  is a plain KVP off `MapEntity.key_values` — parse with `parse_bool` (absent or malformed → warn,
  default the gate open). A deliberate exception to the convention that bare (non-`initial_`) KVPs
  are not consumed by components; call it out in a comment.
- **`aggroGate` consequential primitive** (tag-keyed) — one verb carrying `{ open: bool }`, bound at
  level install to a closed command, executed VM-free in the fixed tick, resolved at fire time
  against the live tagged Brain set (spawned enemies included). Sets `aggro_armed` per `open`. An
  enemy-semantic verb — **not** an overload of `armTrigger`/`disarmTrigger` (that coupling is
  false-DRY: the two split the moment aggro grows conditions). Effect-based dispatch: consequential
  class (mutates replicated sim behavior).
- **SDK enemy-group handle** — `enemies(filter: { tag?: string })` → `EnemyGroup`, with
  `releaseAggro()` (open) and `holdAggro()` (close) each returning a single
  `PrimitiveReactionDescriptor` (the `damage(tag)` shape; tag-targeted primitives ride the
  `Primitive` reaction path, never a `sequence`).
- **Full primitive-contract surface** for the one new verb `aggroGate`: `CONSEQUENTIAL_PRIMITIVES`,
  `BoundTriggerCommand` (+ `Kind`), the bind/partition arms, a `register_*_reaction_primitives`
  call, SDK TS + Luau builders, typedef templates + drift snapshot + parity fixtures, validators.

### Out of scope

- **Endpoint-reserved handle verbs** — `forceTarget` (the E10 `select_target` seam), `setHostility`
  / faction (the "Richer enemy/character behavior descriptors" bullet; a Gravelord-style
  enemy→ally turn), `despawn` / `kill` (E18-R respawn + player-leave). The handle's verb space is
  designed to admit these as additive tag-targeted verbs; none is built here.
- **The read / query face** — group alive/kill count → shared slot for arena-clear gates. Belongs
  to the area-scoped-progress spec (deep questions: regional cells, regional BVH). This spec ships
  the command face only.
- **Additional aggro conditions** — LOS cone, sound gates. Later perception specs; they narrow
  acquisition (compose with the gate), never replace it. See `context/research/enemy-aggro-model.md`.
- **A wire field for the gate** — a gated enemy replicates as a stationary Idle enemy via its
  existing Transform; no new wire surface. `aggro_armed` is host-authoritative sim state; its
  behavioral effect replicates through the motion clients already observe.
- **Enemy id-addressed selection via `world.query`** — enemies are fire-time-tag by nature;
  `world.query` stays the setup-id device model (movers/triggers/lights/fog/spawner).
- **The spawner entity, `spawnFromSpawner`, and the closet set-piece** — E18-C, the consumer. This
  spec is the enemy handle and its aggro gate only.

## Naming (open — owner's call)

The handle-verb pair is drafted as `releaseAggro()` / `holdAggro()` (theatrical set-piece voice —
"release the closet"). Alternatives: `allowAggro` / `suppressAggro` (neutral), `armAggro` /
`disarmAggro` (matches the internal `aggro_armed` field). Placeholder pending the owner's call; it
is a mechanical rename across this spec, the primitive stays `aggroGate` regardless.

## Acceptance criteria

- [ ] An enemy placed `enabled_on_spawn = false` does not acquire or move toward a player standing
      adjacent (including through a thin wall) and does not leave its start position.
- [ ] Firing a reaction whose body is `enemies({ tag }).releaseAggro()` (or the raw
      `{ primitive: "aggroGate", tag, args: { open: true } }`) against that enemy's tag makes it
      acquire and pursue an in-range player on the next think tick.
- [ ] `holdAggro()` re-closes the gate; a re-closed enemy stops acquiring. Repeated fires of the
      same open/close are idempotent (no oscillation, no panic).
- [ ] A gated (closed) enemy still takes damage and can be killed; killing it fires no
      target-selection or steering behavior.
- [ ] **Handle is sugar:** `enemies({ tag: "closet_a" }).releaseAggro()` deep-equals
      `{ primitive: "aggroGate", tag: "closet_a", args: { open: true } }`; `holdAggro()` deep-equals
      the same with `open: false`.
- [ ] An `aggroGate` descriptor whose tag resolves to no Brain warn-skips once (asserted through its
      warn counter — no static validator), and coexists with other reactions fired by the same
      trigger fan-out (e.g. a `moverStart` reaction) without interference.
- [ ] **Fire-time resolution:** an enemy created *after* install (via the `#[cfg(test)]` registry
      seam) carrying the tag is affected by a later `aggroGate` fire — the property that
      distinguishes fire-time-tag from setup-id resolution.
- [ ] A gated enemy is skipped by the separation pass — with a second agent placed inside
      `SEPARATION_RADIUS_FACTOR`, the gated agent's position is unchanged across ticks.
- [ ] SDK TS and Luau emit byte-identical `aggroGate` descriptors; the typedef drift check passes
      after regeneration.
- [ ] No new wire surface: a gated enemy's snapshot carries no gate field; it replicates via its
      existing Transform as a stationary Idle enemy (snapshot shape unchanged from a non-gated
      stationary enemy).

## Tasks

### Task 1: Aggro gate on `BrainComponent` + FSM / steering gating + seeding

Add the gate field to `BrainComponent` (`crates/entities/src/components/brain.rs`) — a boolean
(working name `aggro_armed`, default open). In `run_ai_tick_with_navigation`
(`crates/postretro/src/scripting/systems/ai.rs`), a gated brain does not run `evaluate_transition`,
does not steer, and holds its current position and `LogicalState` (Idle/Alert/Attack/Death,
`brain.rs`) — its Transform is untouched, so it replicates as a stationary Idle enemy. Ensure the
O(n²) steering separation pass (`crates/postretro/src/agent_steering.rs`, `SEPARATION_RADIUS_FACTOR`)
also skips a gated agent so it is not nudged. Damage, health, and death paths are untouched — a
gated enemy can be killed. Seed the gate closed from an `enabled_on_spawn = false` KVP on an
AI-enemy placement, read where `attach_descriptor_components` (`data_archetype.rs`) consumes
placement KVPs; `parse_bool`, bare-KVP exception comment, absent/malformed → warn and default open.
The seed is not a `DescriptorMapOverride` (that enum is closed to light/emitter), so ensure a
descriptor hot-reload re-applies it, or document reload-reopens-the-gate as a dev-only limitation.

### Task 2: `aggroGate` consequential primitive + Brain-tag executor

Add one consequential verb `aggroGate` carrying `{ open: bool }`. Register it in
`CONSEQUENTIAL_PRIMITIVES` and add a `bind_command` arm (`crates/postretro/src/trigger_bindings.rs`)
→ a new `BoundTriggerCommand::AggroGate { open }` (`crates/postretro/src/trigger_commands.rs`). Its
executor (`execute_non_store`) resolves `BoundTarget::Tag` against the live **Brain** set —
`query_by_component_and_tag(Brain, tag)`, not `Transform` — and sets each matched brain's
`aggro_armed` per `open`. A tag resolving to no Brain warn-skips once (reuse the shipped
per-primitive warn-skip counter). This is a distinct Brain-targeted command, **not** an extension of
the trigger-mutation chokepoint (`apply_trigger_mutation_to_targets`) — enemy aggro and trigger
arming are separate concerns that only look alike as booleans. No wire surface: the gate is
host-side sim state; its effect reaches clients through the enemy's existing Transform replication.

### Task 3: SDK `enemies({ tag })` handle + typedefs + validators + parity

Add the enemy-group selector and handle under `sdk/lib/` — a new
`sdk/lib/entities/enemies.ts` exporting `enemies(filter: { tag?: string }): EnemyGroup`, wrapped
into the SDK surface (`sdk/lib/index.ts` / `prelude.ts` alongside `damage`). `EnemyGroup` carries
`releaseAggro()` and `holdAggro()`, each returning a single `PrimitiveReactionDescriptor`
(`{ primitive: "aggroGate", tag, args: { open } }`) — the `damage(tag)` builder is the exact
template (`sdk/lib/data_script.ts:204`). Mirror in Luau. Extend the typedef templates
(`crates/scripting-core/src/typedef/templates/`), regenerate `postretro.d.ts` / `.d.luau`, update
the drift snapshot, add TS/Luau parity fixtures. Update the reaction-argument validators to accept
`aggroGate` and reject a malformed `open`, matching the shipped per-primitive warn-skip. No
`world.query` filter change — the enemy selector is its own fire-time-tag path.

## Sequencing

**Task 1 → Task 2 → Task 3**, sequential. Task 2 sets the `aggro_armed` field Task 1 defines; Task 3
regenerates typedefs from the verb Task 2 registers. Task 1 and Task 2 are engine
(`brain.rs`/`ai.rs`/`agent_steering.rs`/`data_archetype.rs`, then
`trigger_bindings.rs`/`trigger_commands.rs`); Task 3 is `sdk/` + typedef templates — disjoint from
1–2, but ordered after 2 for the verb contract.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Aggro gate | `BrainComponent` `aggro_armed` | not on the wire | n/a | n/a | `enabled_on_spawn` (bool, enemy placement) |
| Aggro verb | `BoundTriggerCommand::AggroGate { open }` | reaction descriptor `"aggroGate"` | `"aggroGate"` | `"aggroGate"` | n/a |
| Enemy selector | `query_by_component_and_tag(Brain, tag)` | n/a | `enemies({ tag })` | `enemies({ tag })` | n/a |
| Enemy handle verbs | n/a | n/a | `EnemyGroup.releaseAggro` / `holdAggro` | same | n/a |

## Rough sketch

- **Gate + FSM (Task 1).** Gate in `run_ai_tick_with_navigation` before `evaluate_transition`
  (Idle→Alert is XZ-distance, no LOS — the reason a sealed enemy would otherwise aggro through
  walls). Seed where `attach_descriptor_components` reads placement KVPs.
- **Verb (Task 2).** `bind_command`'s match is the single primitive→`BoundTriggerCommand` mapping
  point; `execute_non_store` matches `BoundTarget::Tag` via `resolve`. The `AggroGate` arm resolves
  Brains (not Transforms) by tag and flips `aggro_armed`. One new verb touches
  `CONSEQUENTIAL_PRIMITIVES`, `BoundTriggerCommand` (+`Kind`), `partition_direct_reaction` /
  `bind_sequence_step`, a `register_*_reaction_primitives` call, then the Task 3 SDK/typedef surface.
- **Handle (Task 3).** `damage()` (`data_script.ts:204`) is the template: a pure builder returning a
  tag-keyed `PrimitiveReactionDescriptor`. `enemies({ tag })` returns an object whose verbs bake the
  filter's tag into that descriptor. Object-filter selector, not a positional string, so the
  descriptor's binding key can gain fields later without a signature break.
- **Oversized-file note (soft):** `ai.rs` exceeds ~800 lines; additions here are one Brain field +
  one tick guard. Verb logic is localized to the `trigger_bindings.rs` / `trigger_commands.rs` arms.

## Script syntax examples

```ts
// A sealed reveal-closet: enemies placed gate-closed in TrenchBroom, released on a plate.
// Two single-primitive reactions; the trigger fan-out fires both (a reaction body is ONE
// primitive, and tag-targeted primitives ride the Primitive path, never a sequence).
export const openClosetDoor = defineReaction("openClosetDoor",
  { primitive: "moverStart", tag: "closet_door" });

export const releaseCloset = defineReaction("releaseCloset",
  enemies({ tag: "closet_a" }).releaseAggro());   // → { primitive:"aggroGate", tag:"closet_a", args:{ open:true } }

// The plate fires both reveal reactions together.
onTriggerEvent({ tag: "reveal_plate" }, "enter", [openClosetDoor, releaseCloset]);

// The handle is sugar — this hand-written body is byte-identical:
defineReaction("releaseCloset",
  { primitive: "aggroGate", tag: "closet_a", args: { open: true } });
```

```
// TrenchBroom: a pre-placed enemy that starts gate-closed (contained until the reveal).
{ "classname" "grunt" "enabled_on_spawn" "0" "_tags" "closet_a" }
```
