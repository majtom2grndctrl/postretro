# E18 — Contact Dispatch Parameters (`ContactScope`)

## Goal

Let a contact-fired reaction act on the ephemeral facts of the fire: *who* touched the trigger (`on.activators`), *which* volume fired (`on.trigger`), and how many bodies are on it (`on.occupancy`) — per §12 (`scripting.md`). Unblocks "damage whoever pressed the plate" and "disarm the plate that fired," authored once for any number of plates. Consumes the dispatch-param mechanism from `E18--dispatch-scope-params`; the key architectural move is that entity inputs never enter the IR substrate — `activators`/`trigger` ride the existing command-*target* channel (`BoundTarget`), keeping the IR purely numeric.

## Scope

### In scope

- **Activator identity at the fire site.** The trigger fire event already carries the crossing player (`TriggerEventFire.player: PlayerId`); resolve it to a pawn `EntityId` at dispatch (Local wraps the pawn; Remote resolves via `AuthoritativePlayer.pawn`) and thread a fire context — fired-trigger id, activator pawn set, effective occupancy — into the binding executor, which today keys only on `(trigger, edge)` and discards the player.
- **Sentinel command targets.** Two dispatch-time targets alongside tag and entity-id targeting: `"@activators"` (the fire's activating pawns — broadcast: the engine iterates the set) and `"@trigger"` (the fired volume). Legal in a `SequenceStep` id position and as a `PrimitiveReactionDescriptor` target; resolved per fire by the executor.
- **`on.occupancy` numeric leaf.** `@occupancy` — the fired volume's *effective* occupant count (alive-filtered, the `E18--coop-activation-policy` predicate) at fire time: on an enter fire it includes the activator; on an exit fire it excludes them. Bound via a contact-vocabulary `DispatchScope`.
- **SDK surface.** `ContactParams` (`activators`, `trigger` opaque target tokens; `occupancy` a `RuntimeValue`); free builders `damage(on.activators, amount)`, `armTrigger(on.trigger)` / `disarmTrigger(on.trigger)`; both runtimes, typedefs, drift check.
- **Script-side contact observer.** Free function `onContact({ tag }, "enter" | "exit", fire[])` returning a descriptor; carried on `setupLevel`'s manifest and `ModManifest.contacts` with the same `levels` selector as crossings. Installs into the same binding table the brush `on_fire`/`on_exit` KVP path uses; both paths publish `ContactScope`.
- **`"occupied"` is not an event.** The §12 table's third contact event is dropped: occupancy-conditioned firing is already covered by crossings over the `occupiedCount`/`occupants` ambient refs — one mechanism, not two. §12 updated at promotion.
- **Split-before-extend.** Relocate the command/target/execute half out of `trigger_bindings.rs` (986 lines) before extending it.
- Tests: headless damage-the-presser, disarm-own-plate, occupancy param, install-time cross-source rejection; two-endpoint remote-activator damage.

### Out of scope

- **Non-player activators.** The trigger substrate tracks `PlayerId` only; AI/monster activators wait for the substrate to track them. `activators` is a pawn set, v1.
- **Per-activator sequence fan-out** (`on.activators.each((a) => seq([...]))` — §12 fork 3b). Broadcast targeting covers the driving cases; a fan-out opcode adds control flow to the loop-free IR and waits for a concrete case needing distinct per-activator *sequences*.
- **An `EntityRef` script value type.** `on.activators`/`on.trigger` are opaque author-time target tokens; no entity id is ever readable in script from dispatch, and they are never IR operands (`runtime.add(on.trigger, 1)` is ill-typed and has no wire form).
- **Entity inputs in the IR substrate** — no new `IrValue` kind, no entity-typed `BindingScope` channel. Targets are commands' business.
- **`levelLoad`/crossing sources publishing contact inputs** — a `ContactParams` reaction on those sources rejects at install (mechanism owned by the sibling spec).
- Dynamic (script-spawned) trigger volumes as `onContact` subjects — map-authored volumes only, matching the trigger substrate.

## Acceptance criteria

- [ ] Damage-the-presser: with a reaction `damage(on.activators, 25)` bound to a plate's enter event, the entering pawn loses exactly 25 HP and a second pawn standing elsewhere is untouched (headless).
- [ ] Broadcast-per-edge: two pawns entering the same plate on the same tick are each damaged exactly once (enter edges are per-player; each fire's activator set is that edge's pawn).
- [ ] Disarm-own-plate: two plates sharing one binding/address — stepping on plate A disarms plate A only; plate B still fires later (`disarmTrigger(on.trigger)`).
- [ ] `on.occupancy`: a reaction writing `on.occupancy` to a slot records 2 when two live pawns occupy the fired volume at fire time; a corpse on the volume is not counted; an exit fire excludes the leaver.
- [ ] Cross-source rejection: a `ContactParams` reaction fired from `levelLoad` or a crossing is rejected at level install with a warning and never fires; other reactions on that source are unaffected.
- [ ] `onContact({ tag }, "enter", [r])` fires identically to a brush `on_fire` KVP naming the same reaction — same fire context, same commands (headless equivalence test).
- [ ] Two-endpoint: a remote player's pawn pressing the plate takes the damage on the host, and the client observes it via the shipped health replication; the local player is untouched.
- [ ] TS: passing a `ContactParams`-typed reaction to `onStateCrossing`'s fire list fails `tsc --noEmit` (compile-must-fail fixture); `Reaction<{}>` is still accepted by contact sources.
- [ ] Typedef drift check passes with `ContactParams`, `onContact`, `damage`, `armTrigger`/`disarmTrigger` in `postretro.d.ts` / `.d.luau`; a TS and a Luau fixture author an identical presser-trap producing identical wire data.
- [ ] The binding executor split lands behavior-preserving: existing trigger-binding tests pass unchanged before the feature commits.
- [ ] Determinism: two identical headless runs produce identical damage ledgers and fire sequences for a multi-pawn presser scenario.

## Tasks

### Task 1: Split `trigger_bindings.rs`

Behavior-preserving relocation before the extend. Move the command/target/execute half — `BoundTriggerCommand` (`crates/postretro/src/trigger_bindings.rs:96`), `BoundTarget` (`:122`), `BoundTriggerCommand::execute` (`:277`), `BoundTarget::resolve` (`:332`) — into a new sibling module (e.g. `trigger_commands.rs`), leaving the binding table, install/partition logic (`TriggerBindingTable`, `partition_direct_reaction` at `:355`, `CONSEQUENTIAL_PRIMITIVES` at `:25`), and `bind_sequence_step` (`:524`) in place. No behavior change; existing trigger tests pass unchanged (re-path as needed). Isolates the surface Tasks 2–3 extend.

### Task 2: Fire context → executor; sentinel targets; `@occupancy`

The engine core. **Fire context:** build a per-fire context at the dispatch closure in `run_authoritative_tick_with_dispatch` (`crates/postretro/src/trigger_system.rs:140`; closure invoked at `:262`/`:306` with the full `TriggerEvent`) carrying the fired trigger `EntityId`, the activator pawn set (resolve `TriggerEventFire.player: PlayerId` — `Local(EntityId)` is the pawn; `Remote(u64)` resolves through the tick's `AuthoritativePlayer { id, pawn }` list built at `sim/mod.rs:156-169` — one pawn per enter/exit edge today, carried as a set for future multi-activator fires), and the fired volume's effective occupant count via `effective_occupants(volume, &alive_set)` from `E18--coop-activation-policy` (enter fires dispatch after the occupant-map insert, so the count includes the entrant; assert the exit-side complement). Thread it through `TriggerBindingTable::execute` (`trigger_bindings.rs:231`) into command execution — the signature currently takes only `(trigger, edge, registry, slot_table)`. **Sentinel targets:** add `Activators` and `FiredTrigger` variants to `BoundTarget` (Task 1's new module); `resolve` maps them from the fire context to entity lists (broadcast: `Damage` on `Activators` applies the shipped per-entity damage dispatch to each pawn; `Arm`/`Disarm` on `FiredTrigger` targets the fired volume). At bind time (`partition_direct_reaction` / `bind_sequence_step`), a `"@activators"`/`"@trigger"` spelling in a step id or primitive target position produces the variant; the same spelling reaching a *non-trigger* install path (named/`levelLoad`/crossing dispatch) is rejected at install with a warning, and unknown `@`-spellings reject rather than parse as tags. **`@occupancy`:** register a contact vocabulary `[("@occupancy", Number)]` `DispatchScope` (sibling spec's Task 1 type); trigger-installed IR-valued `setState` values (`BoundTriggerCommand::StoreSlot`, extended by `ready/E18--ir-valued-reactions` to carry a bound program) bind against it at install and eval in-tick with the fire context's count seeded. Plumbing: the fire context is built where `TriggerTickContext` (`sim/mod.rs:66`) members are already in scope; `AuthoritativePlayer` list and alive set are tick-local per the coop spec's Task 2.

### Task 3: `onContact` observer — wire, manifest, install

Script-side contact binding. Descriptor: `{ tag, event: "enter" | "exit", fire: [names/handles], levels? }`. SDK free function `onContact(filter, event, fire)` (filter is `{ tag: string }`, matching the trigger-volume query filter vocabulary) returning the descriptor; carried on `setupLevel`'s returned manifest under a new `contacts` key and on `ModManifest.contacts`, composed at level install by the same `levels` tag-selector rules as reactions/crossings (additive, same-name warn). Install: resolve `tag` to the level's trigger-volume entities (the entity `_tags` grouping, as trigger diagnostics reads it) and register each fired reaction into `TriggerBindingTable` under `(volume, edge)` — the exact table the brush `on_fire`/`on_exit` KVP path populates, so execution, partition (consequential vs presentation), residual drain, and the Task 2 fire context are shared with zero new dispatch machinery. A tag matching zero volumes warns and is inert (the shipped inert-degradation pattern). `"enter"`/`"exit"` are the only events — no `"occupied"`; the parser rejects other event strings with a warning.

### Task 4: SDK surface + typedefs

Author-facing. Extend the sibling spec's params-object layout with `ContactParams`: `activators` and `trigger` as frozen opaque target tokens (distinct TS brands so `damage` accepts only `activators`-or-tag and `armTrigger`/`disarmTrigger` accept only `trigger`), `occupancy` as the `{op:"input",name:"@occupancy"}` node. Free builders exported from `"postretro"`: `damage(target, amount)` → `{ primitive: "applyDamage", target: "@activators", args: { amount } }` (with a tag string it emits the shipped tag-targeted form); `armTrigger(target)` / `disarmTrigger(target)` → one-element step arrays `[{ id: "@trigger", primitive: "armTrigger" | "disarmTrigger", args: {} }]` mirroring the handle builders (`sdk/lib/entities/triggers.ts:26`). `onContact` per Task 3. Luau twins with byte-identical wire output. Typedef surface (`crates/postretro/src/scripting/typedef/`) gains `ContactParams`, the builders, `onContact`, and the widened step-id type; regen `postretro.d.ts`/`.d.luau`, update the drift snapshot. Compile-must-fail fixtures: `ContactParams` reaction in a crossing fire list; `damage(on.trigger, 5)`; `on.activators` used as a `runtime.*` operand.

### Task 5: Tests and fixtures

Prove the pattern end to end. Headless: presser trap (enter → 25 damage to the entrant only, via the health ledger); same-tick double-entry broadcast (each pawn damaged once); disarm-own-plate with two plates sharing a binding; `on.occupancy` slot write (2 live pawns → 2; corpse excluded; exit excludes leaver); install-time rejection of a contact reaction on `levelLoad` and on a crossing; `onContact`-vs-brush-KVP equivalence. Parity: TS and Luau fixtures author an identical presser trap with identical wire data. Two-endpoint (loopback harness): remote pawn presses, host applies damage, client converges via health replication. Determinism: extend the headless gate with a multi-pawn presser sequence. Dev fixture (level + script): a spike-trap room demonstrating `damage(on.activators, …)` + `disarmTrigger(on.trigger)` as the documented one-shot-trap pattern.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; blocks the executor extend.
**Phase 2 (concurrent):** Task 2 (engine fire context + sentinels), Task 4 (SDK + typedefs) — disjoint files; Task 4 authors against the pinned wire.
**Phase 3 (sequential):** Task 3 — `onContact` install consumes Task 2's executor/table shape.
**Phase 4 (sequential):** Task 5 — tests/fixtures over the finished surfaces.

Cross-spec dependencies: requires `E18--dispatch-scope-params` (the `DispatchScope` substrate and params-object/typing layout), `ready/E18--ir-valued-reactions` (bound `StoreSlot` programs on the trigger path), and `ready/E18--coop-activation-policy` (`effective_occupants` helper + alive set). All three land first.

## Rough sketch

- **Two worlds, resolved.** Numeric dispatch inputs (`@occupancy`) are IR leaves through `DispatchScope` — same substrate as `@rising`/`@dt`. Entity dispatch inputs are **command targets**: the descriptor stores a sentinel spelling, bind produces a `BoundTarget` variant, and the executor resolves it from the fire context. The IR substrate stays two-typed (`Number`/`Bool`); no entity channel, no new leaf kind. This was the handoff's "biggest open substrate question" — answered by keeping entities out of the substrate entirely.
- **Why broadcast targeting suffices (v1).** The driving cases apply *one* command to each activator (damage) or target *the* fired volume (arm/disarm). Per-edge dispatch means the activator set is a single pawn today, so broadcast and fan-out are indistinguishable in practice; fan-out earns its IR control-flow cost only when a case needs per-activator *sequences*.
- **Grounding (verified this session):** `TriggerEventFire { trigger, player: PlayerId, event_name }` (`trigger_system.rs:42`); edges detected per-player and sorted (`:211`, `:236`); the executor drops the player today (`bindings: HashMap<(EntityId, TriggerEventEdge), TriggerBinding>`, `trigger_bindings.rs:55`). `BoundTriggerCommand::Damage { target, amount }` already exists (`:96`) — the presser case is a new target variant, not a new command. The damage chokepoint is `apply_damage_with_context` (`crates/entities/src/components/health.rs:319`); the reaction handler receives resolved `EntityId` slices (`crates/postretro/src/health/reactions.rs:47`). Entity-targeted steps already cross the FFI as `{ id: EntityId, primitive, args }` (`sdk/lib/entities/triggers.ts:26`, Rust mirror `crates/entities/src/data_descriptors/types/reactions.rs:20`) — the sentinel widens that id position.
- **Oversized-file flags:** `trigger_bindings.rs` 986 → Task 1 splits it. `trigger_system.rs` 1695 — the coop spec's Task 1 already relocates occupancy out; this spec's edit there is the dispatch-closure fire context only (localized; no second split). `sim/mod.rs` edits are parameter threading.
- **Fire-context timing:** enter fires dispatch after the occupant-map insert and after the arm/disarm re-check (`trigger_system.rs:270-280`), so `@occupancy` naturally includes the entrant and honors same-tick disarms.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| activators target | `BoundTarget::Activators`, resolved from fire context | `"@activators"` in a step `id` or primitive `target` position | `on.activators` (opaque token) via `damage(on.activators, n)` | same | n/a (KVP names the reaction; the sentinel lives in its body) |
| fired-trigger target | `BoundTarget::FiredTrigger` | `"@trigger"` in a step `id` position | `on.trigger` via `armTrigger`/`disarmTrigger` | same | n/a |
| occupancy input | `DispatchScope` contact vocabulary `("@occupancy", Number)` | `{"op":"input","name":"@occupancy"}` | `on.occupancy` (`RuntimeValue`) | same | n/a |
| contact observer | contact descriptor + install into `TriggerBindingTable` | `contacts: [{ tag, event: "enter"\|"exit", fire, levels? }]` on `setupLevel` return / `ModManifest` | `onContact({ tag }, event, fire)` | same | brush `on_fire`/`on_exit` KVPs remain the map-authored referrer |

Step `id` widens from `EntityId` to `EntityId | "@activators" | "@trigger"` (numeric vs string discriminates); `PrimitiveReactionDescriptor` gains optional `target`, mutually exclusive with `tag`. No new binary/PRL section.

## Script syntax examples

```ts
import { world, defineReaction, onContact, damage, disarmTrigger, type ContactParams } from "postretro";

// One-shot spike trap: hurt whoever pressed, then that plate never fires again.
// Written once; works for every plate tagged "trap". No string name — the const
// is the identity (name-as-address), so onContact holds the reference directly.
const zap = defineReaction((on: ContactParams) => damage(on.activators, 25));
const oneShot = defineReaction((on: ContactParams) => ({ sequence: disarmTrigger(on.trigger) }));

export function setupLevel() {
  return {
    reactions: [zap, oneShot],
    contacts: [onContact({ tag: "trap" }, "enter", [zap, oneShot])],
  };
}
```

```luau
-- Luau parity: the tracer runs once; the engine gate enforces scope.
local Postretro = require("postretro")
local zap = Postretro.defineReaction(function(on)
  return Postretro.damage(on.activators, 25)
end)
```
