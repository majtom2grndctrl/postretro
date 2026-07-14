# E18 — Dispatch-Scope Parameters (crossing + tick)

## Goal

Let a reaction read the ephemeral inputs that exist *because* a particular fire happened — the crossing direction, the tick `dt` — through a typed `param` proxy, per the §12 Reaction Dispatch Model (`scripting.md`). Ships the shared dispatch-param mechanism (author-time tracer, reserved input names, a source-layered `BindingScope`) plus the two purely numeric scopes: `CrossingScope` and `TickScope` with the slot accumulator. Unblocks King-of-the-Hill (countdown drains per body on the hill) and direction-sensitive crossings (shields up on falling, all-clear on rising). The entity-bearing `TriggerEventScope` is the sibling spec `E18--trigger-event-params`, which consumes this spec's mechanism.

## Scope

### In scope

- **Tracer form of `defineReaction`.** New overloads `defineReaction((on: CrossingParams) => body)` and `defineReaction(name, (on) => body)` beside the shipped data forms. The arrow is an **author-time tracer**: it runs exactly once at authoring, receives a frozen params object whose leaves are `RuntimeValue` input nodes, and returns a plain `ReactionBody`. No function survives; the VM still drops (§1/§2). Type annotations are erased at runtime (Luau has none), so every tracer receives the **same SDK-internal merged dispatch-params object** — the union of all dispatch leaves, extended by sibling specs; `CrossingParams`/`TickParams` are exported as **types only** that narrow it. Scope discipline is TS-nominal plus the engine bind gate (Luau: engine gate only).
- **Reserved dispatch-input namespace.** Dispatch inputs are IR `input` leaves with `@`-prefixed names (`@rising`, `@dt`; the sibling spec adds `@occupancy`). Store declarations reject namespaces and slot names starting with `@`, reserving the prefix.
- **`DispatchScope` — a layered `BindingScope`.** New scripting-core scope wrapping `StoreScope`: `@`-names resolve from a typed per-source input set (names + types fixed at bind; values set per fire); all other names delegate to the store; outputs always delegate. Bind once at install, eval per fire — never re-bind.
- **`CrossingScope` publishes `rising: Bool`.** Every crossing fire publishes the transition direction. `crossed`/`value` from the §12 table are **dropped** (see Rough sketch); §12 is updated at promotion.
- **Opt-in both-edge crossings.** Optional `edge: "both"` on both crossing forms: the watcher fires on each transition through the condition in either direction, publishing `rising` accordingly. Absent `edge` preserves the shipped single-edge fire/re-arm lifecycle exactly.
- **Dispatch-value carriage.** A crossing-fired `setState` whose bound IR reads `@rising` captures the fire's dispatch values onto the enqueued system command; the app-drain evaluates at the write point with those values layered over live slots — the same-frame accumulation contract of `ready/E18--ir-valued-reactions` is preserved.
- **Slot accumulators (`TickScope`).** `defineStore` number slots accept `accumulate: (t) => expr`, traced at `defineStore` time into a delta IR node over `@dt` and ambient slot reads. The engine evaluates `slot += expr` once per authoritative sim tick, clamped to the declared range. Host-authoritative; clients receive via the slot's declared replication.
- **`runtime.read` accepts state refs.** `runtime.read(occupants("hill"))` and passing a `StateRef` as any builder operand auto-wrap to `{op:"input",name:<slot>}` — TS and Luau byte-identical.
- **Nominal scope typing (TS).** `Reaction<S>` carries a phantom scope marker; a source's fire list accepts `Reaction<{}>` and its own scope only. Engine-side enforcement is bind failure at install (warn + skip that subscription) — no wire scope tag.
- SDK builders, typedef regen + drift check, TS/Luau parity, determinism coverage.

### Out of scope

- **`TriggerEventScope`** (`activators`, `trigger`, `occupancy`) and every entity-targeted dispatch surface — sibling spec `E18--trigger-event-params`. (That spec also renames §12's "contact" event type to **trigger event**.)
- **`crossed`/`value` on `CrossingScope`** — dropped for v1 (author-time constant / ambient read; see Rough sketch). Re-add only when a use case needs snapshot semantics.
- **A bare per-tick reaction source.** Per §12, per-tick is accumulator-only; `TickScope` is the accumulator's param type, never a `defineReaction` param.
- **New IR opcodes.** The shipped node set is the whole vocabulary. (No `neg` — spell negation `runtime.sub(0, x)`.)
- **Dispatch-param reads outside IR-valued arg positions.** v1 IR-valued positions are the `setState` value (shipped by `ready/E18--ir-valued-reactions`) and `accumulate` (this spec). A `RuntimeValue` in any other descriptor arg is not evaluated and is rejected where the shipped arg validation already rejects malformed args.
- **Client-side accumulation** and any prediction of accumulated slots.
- Structural-minimal scope typing (accept-by-used-subset) — nominal-whole-scope only.

## Acceptance criteria

- [ ] A traced reaction is pure data: `defineReaction((on: CrossingParams) => …)` returns a frozen descriptor that round-trips `JSON.stringify` unchanged, carrying `{op:"input",name:"@rising"}` leaves — no function property survives.
- [ ] Direction: a **threshold-form** crossing with `edge: "both"` firing `setState(slot, runtime.select(on.rising, 1, 0))` leaves the slot at 1 after a rising transition (watched value moving upward) and 0 after a falling one (headless).
- [ ] Regression: crossings without `edge` — threshold and predicate forms — fire once on their authored edge and re-arm on cross-back, unchanged (existing crossing tests pass).
- [ ] An unrecognized `edge` value warns and degrades to the shipped single-edge behavior, identically in the TS and Luau parsers.
- [ ] Enforcement: a reaction whose IR reads `@rising`, fired from the `levelLoad` address or bound to a trigger `on_fire`, is rejected at level install with a warning and never fires; other reactions on the same source still fire.
- [ ] A countdown slot declared `range: [0, 60], default: 60` with `accumulate: (t) => runtime.mul(t.dt, -1)` reaches 0 after 60 simulated seconds and never goes below 0 (clamp).
- [ ] King-of-the-Hill scaling: an accumulator reading `occupants(tag)` drains twice as fast with two pawns on the hill as with one (headless, two-pawn).
- [ ] Same-frame ordering: a predicate crossing on the countdown slot fires on the exact tick the accumulator brings it to its bound — occupancy write → accumulators → crossing detection within one tick.
- [ ] Declaration validation: `accumulate` on a non-number slot rejects the whole store declaration; `accumulate` on a `readonly` slot rejects; a namespace or slot name starting with `@` rejects.
- [ ] An accumulator whose expr reads a slot absent at level install (e.g. `occupants` of a tag with no triggers in this level) warns and is inert for that level; the store's other slots work normally.
- [ ] Two-endpoint: an accumulated `SharedGlobal` slot converges on a connected client; the client does not evaluate the accumulator locally.
- [ ] `runtime.read(ref)` and a bare `StateRef` operand produce `{op:"input",name:<slot>}` byte-identically in TS and Luau.
- [ ] Type-level fixtures (compile-must-fail): `(t: TickParams) => t.rising` and `(on: CrossingParams) => on.dt` both fail `tsc --noEmit`; a plain `Reaction<{}>` is accepted by every source.
- [ ] Typedef drift check passes with the tracer overloads, `CrossingParams`/`TickParams`, `edge`, `accumulate`, and widened `runtime.read` in `postretro.d.ts` / `.d.luau`; a TS and a Luau fixture author an identical direction-and-accumulator flow producing identical IR.
- [ ] Determinism: two identical headless runs produce identical slot timelines and crossing-fire sequences with accumulators and both-edge crossings active.

## Tasks

### Task 1: `DispatchScope` substrate

New scripting-core `BindingScope` that layers a typed dispatch-input set over `StoreScope`. Shape: `DispatchScope` owns an inner `StoreScope` (constructed with the same `StoreCapability` split) plus a fixed input vocabulary — an ordered list of `(&'static str, IrType)` pairs chosen per source (crossing: `[("@rising", Bool)]`; tick: `[("@dt", Number)]`) and a same-length value array set before each eval, following the `MovementScope` snapshot pattern (`crates/foundation/src/movement/scope.rs:45`). `InputHandle` is a two-arm enum: dispatch index or delegated `StoreHandle`. `resolve_input`: names starting with `@` resolve only from the dispatch vocabulary (unknown `@`-name → `None`, so bind fails — this is the enforcement seam); all others delegate to the inner store scope. `resolve_output` always delegates (dispatch inputs are read-only). `read` dispatches on the handle arm; `write` delegates. Unit tests in the module: bind resolves `@`-names by type, an unknown `@`-name fails bind, store fallback reads/writes still work, per-fire value updates are observed by re-eval without re-bind. Lives beside `StoreScope` in `crates/scripting-core/src/ir/scopes.rs` (428 lines — room to extend; split not needed).

### Task 2: Crossing direction — `rising`, `edge: "both"`, dispatch-value carriage

Engine side of `CrossingScope`. **Direction out of `detect`:** `CrossingDetector::detect` (`crates/scripting-core/src/state_crossings.rs:79`) currently returns bare `Vec<String>` fire names, discarding direction. Change it to return per-fire records carrying the names plus `rising: bool`. **`rising` is the watched signal's direction, per form** — threshold form: value-direction (an `Above` cross is rising, `Below` falling); predicate form: condition-direction (`false→true` is rising). The same logical watch spelled both ways reports opposite senses; deliberate — the author picked the signal. Update the dispatch call site `dispatch_state_crossings` (`crates/postretro/src/main.rs:3098`) and every caller the compiler flags. **`edge: "both"`:** add an optional field to `CrossingDescriptor` (`crates/entities/src/data_descriptors/types/reactions.rs:86`), default absent = shipped behavior. With `"both"`, the watcher fires on each transition in either direction (threshold: the shipped `Above` test for the rising side and the mirrored `Below` test at the same threshold for the falling side; predicate: both boolean transitions), publishing the actual direction; the `previous`-endpoint tracking already supports this. Any other `edge` value warns and degrades to single-edge, identically in both runtimes' parsers. **Bind per subscription:** the ready spec's install-time `setState` bind pass keys its bound-program side map to the reaction; generalize the key to (firing source subscription × reaction). A crossing `fire` entry binds against a crossing-vocabulary `DispatchScope` (Task 1) so `@rising` resolves; the same reaction's `levelLoad`/named-event subscription binds against a bare store scope, where an `@rising` read fails bind, warns, and skips **that subscription only** — one reaction may hold several bound programs simultaneously. The trigger in-tick path (`BoundTriggerCommand::StoreSlot`'s bound program, installed per the ready spec in `crates/postretro/src/trigger_bindings.rs`) fails an `@rising` bind the same way at trigger install: warn, skip that command, other commands on the binding unaffected. **Carriage:** thread the per-fire values down the dispatch path — `dispatch_state_crossings` hands each fire record's `(name, IrValue)` pairs into the named-event dispatch (`fire_named_event_with_sequences` gains an optional dispatch-values argument), and the `setState` system-reaction handler stamps them, plus the firing subscription's key, onto the enqueued `SystemReactionCommand::SetState` (`crates/entities/src/reactions/system_commands.rs`; `IrValue` is a foundation type, so the entities crate can carry both). At the app-drain write point the subscription key selects the bound program, the carried pairs seed the `DispatchScope` values, and evaluation runs in enqueue order, preserving same-frame accumulation. Wire pin only — the SDK builder edits are Task 3's: threshold form takes `edge` in its condition object; the predicate overload takes it in a new trailing options argument.

### Task 3: SDK author surface — tracer, params objects, `accumulate` field, ref-reads, typedefs

The whole TS + Luau author surface, in one task because it shares the SDK library files and the typedef templates. **Tracer overloads:** extend `defineReaction` (`sdk/lib/data_script.ts:173`; Luau twin in `sdk/lib/data_script.luau`) with `(tracer)` and `(name, tracer)` forms; detect a function argument, invoke it once, and pass the returned body through the existing name/auto-id path (the FNV auto-id hashes the traced body's stable stringify — IR nodes are plain objects, so this already works). **One merged params value:** every `defineReaction` tracer receives the same SDK-internal frozen plain object (no JS `Proxy`) holding all dispatch leaves — `{ rising: {op:"input",name:"@rising"} }` today; sibling specs add keys. `defineStore`'s `accumulate` tracer receives the same object, typed as `TickParams` (`{ dt: {op:"input",name:"@dt"} }` is its visible surface). `CrossingParams`/`TickParams` are exported as types only. **Nominal typing:** `Reaction<S>` = `NamedReactionDescriptor` plus a type-level-only phantom scope marker spelled for contravariance so `Reaction<CrossingParams>` is not assignable to `Reaction<{}>` while `Reaction<{}>` is accepted by every source; `onStateCrossing`'s fire list accepts `Reaction<{}> | Reaction<CrossingParams> | string`. Exact phantom spelling is implementation-chosen, verified by the compile-must-fail fixtures. **`edge` builders:** extend `onStateCrossing` (`sdk/lib/ui/reactions.ts:79`; Luau twin `sdk/lib/ui/reactions.luau:80`) — the threshold condition object accepts `edge: "both"`, the predicate overload gains the trailing options argument, and the condition validation admits the new key in both runtimes. **`accumulate`:** `StoreSlotSchema` number slots accept `accumulate?: (t: TickParams) => RuntimeValue`; `defineStore` (`data_script.ts:273` and the Luau twin) traces it **before** `cloneAndFreeze` runs (the freezer passes functions through untouched and `JSON.stringify` would then drop the key silently) and substitutes the traced node under the `accumulate` key in the declaration schema. **Ref reads:** widen `runtime.read` (`sdk/lib/runtime.ts:50`) to accept a `StateRef` (reads its `.slot`), and widen the `wrap` operand rule (`runtime.ts:35`) to auto-wrap a `StateRef` into the same input node; mirror both in `runtime.luau` byte-identically. **Typedefs:** the declaration templates live at `crates/scripting-core/src/typedef/templates/` — extend them with the tracer overloads, `CrossingParams`/`TickParams`, the `edge` option, the `accumulate` slot field, and widened `read`; regenerate `sdk/types/postretro.d.ts` / `.d.luau` via `gen-script-types` and update the committed drift snapshot (drift tests at `crates/postretro/src/scripting/typedef/tests/`). Add the compile-must-fail type fixtures (`tsc --noEmit` gate).

### Task 4: Slot accumulators — declaration validation, install-time bind, per-tick eval

Engine side of `TickScope`. **Declaration:** `SlotSchemaInput` (`crates/scripting-core/src/store_bridge.rs:24`) gains an `accumulate` field parsed via `ir_node_from_json`; `validate_slot_schema` (`store_bridge.rs:354`) rejects the whole declaration when `accumulate` appears on a non-number or `readonly` slot, and rejects slot names starting with `@`. Namespace rejection lives on the namespace-validation path (`validate_namespace_records`, `crates/entities/src/slot_table.rs:368`) — `validate_slot_schema` never sees the namespace. `SlotSchema` (`crates/entities/src/slot_table.rs:63`) gains `accumulate: Option<IrNode>` (`IrNode` is foundation — legal dependency); `IrNode` is `PartialEq`, so the identical-redeclaration check keeps working. **Bind at level install:** for each committed slot carrying an accumulator, wrap `BakedIr { version: CURRENT_IR_VERSION, output: Some(slot), root: Add(Input(slot), expr) }` — the accumulation composes in the IR, so `eval_and_write` (`crates/foundation/src/ir/eval.rs:41`) plus `write_store_slot`'s range clamp do all the work with zero new eval machinery — and bind against a tick-vocabulary `DispatchScope` (Task 1) with `Script` capability. Bind in the level-install pass (`rebuild_reaction_subscribers` at `crates/postretro/src/startup/lifecycle.rs:890` already holds `script_ctx`); a bind failure (e.g. an `occupants` slot for a tag absent from this level) warns and leaves that accumulator inert for the level. Bound programs live in a postretro-side map keyed by slot name (§13: the descriptor carries the raw node; the bound program lives on runtime state). **Per-tick eval:** the bound-program map and `script_ctx` live binary-side, so eval runs at the authoritative-tick call site in `postretro` — not inside `simulate_tick` (which today receives neither): each host/single-player tick, seed `@dt` from that tick's `tick_dt` (originating as the `simulate_tick` parameter at `crates/postretro/src/sim/mod.rs:116`), then `eval_and_write` every bound accumulator in sorted slot-name order (determinism). Within-frame order: trigger-occupancy slot writes land inside the host tick (coop spec), accumulators run at the tick call site immediately after it returns, and state-crossing detection runs later the same frame at the app frame-order point (`dispatch_state_crossings`, `main.rs:3098`) — so a countdown crossing fires the frame it completes. Clients never run accumulators — replication of the written slot follows its declared network scope, nothing new.

### Task 5: Tests and fixtures

Prove the surfaces end to end. Headless: direction crossing (`edge: "both"` + `select(on.rising, 1, 0)` observes 1 then 0); single-edge regression; install-time rejection of a `@rising` reaction on `levelLoad` and on a trigger binding (other subscriptions unaffected); countdown drain-and-clamp over simulated seconds; KoH two-pawn 2× drain via `occupants(tag)`; same-tick crossing on accumulator completion; declaration rejections (non-number, readonly, `@`-prefix) and the absent-input inert path. Parity: a TS fixture and a Luau fixture author an identical direction-and-accumulator flow and produce byte-identical IR (including `runtime.read(ref)` and bare-ref operands). Two-endpoint (loopback harness, E18 net-QA precedent): accumulated `SharedGlobal` slot converges on the client; client runs no accumulator. Determinism: extend the headless green-and-stays-green gate with an accumulator + both-edge-crossing tick sequence. A KoH dev fixture (level + script) demonstrates the composed pattern for modders.

## Sequencing

**Phase 1 (sequential):** Task 1 — `DispatchScope` substrate; Tasks 2 and 4 bind against it.
**Phase 2 (concurrent):** Task 2 (crossing engine path), Task 3 (SDK + typedefs), Task 4 (accumulator engine path) — disjoint files; Tasks 2/4 consume Task 1's scope, Task 3 authors against the wire pinned in the Boundary inventory.
**Phase 3 (sequential):** Task 5 — tests/fixtures over the finished surfaces.

This spec assumes `ready/E18--ir-valued-reactions` has landed (install-time `setState` bind pass, predicate crossings, `RuntimeValue` wire) and `ready/E18--coop-activation-policy` for the `occupants(tag)` refs the KoH fixture reads. The sibling `E18--trigger-event-params` consumes Task 1 and Task 3's params-object layout.

## Rough sketch

- **Promotion-time §12 updates (enumerated).** The crossing row drops `crossed`/`value` and respells `direction: "rising"|"falling"` as the boolean `rising` leaf; `slot.integrate((t) => expr)` respells as the `accumulate` slot-schema field; the `param`-type column renames to the author-facing `*Params` names. The sibling spec's renames (contact → trigger event, `occupied` drop, source-spelling column) land in the same pass.
- **Why `crossed`/`value` are dropped from `CrossingScope` (v1).** `crossed` (the threshold) is an author-time constant — the author wrote it; a dispatch param adds nothing. `value` (the watched slot's value at detection) is readable ambiently via `runtime.read(slot)` at eval time; the only difference is detection-time snapshot vs write-point read, and no driving use case needs the snapshot. The predicate form has no single watched slot, so a uniform nominal `CrossingScope` couldn't publish them anyway without per-form scope splits. `rising` is the one input that is genuinely dispatch-time and uniform across both forms. Update the §12 table at promotion.
- **Enforcement is bind failure, not a wire tag.** The descriptor carries no "required scope" field; the engine derives requirements by attempting the bind — a source's `DispatchScope` vocabulary either resolves the IR's `@`-names or the bind fails (`BindError::UnknownInput`) and the subscription is skipped with a warning. TS nominal typing (`Reaction<S>`) is the author-time mirror of the same contract; Luau relies on the engine gate.
- **Accumulate wire carries the delta, engine composes the sum.** The declaration's `accumulate` node is the per-tick delta expr; the engine wraps `root = Add(Input(slot), expr)` at bind. Keeps the wire semantic ("this is a rate") and the composition engine-owned.
- **Both-edge is an option, not a body inference.** Whether a watcher fires both directions is a source property (`edge: "both"`), never inferred from what its reactions read — a reaction body must not change source behavior. A single-edge watcher still publishes `rising` (constant per its authored sense), so `on.rising` reactions are valid on any crossing.
- **Grounding (verified this session):** `TriggerEventFire` carries `PlayerId` at every fire (`crates/postretro/src/trigger_system.rs:42`, field at `:44`) — unused here, load-bearing for the sibling spec. `detect` drops direction at its `Vec<String>` return (`state_crossings.rs:79`). `tick_dt` is a `simulate_tick` param only (`sim/mod.rs:116`), reaching no scope today. `MovementScope`'s fixed-vocabulary snapshot (`foundation/src/movement/scope.rs:28`) is the template Task 1 generalizes. `state_crossings.rs` is 425 lines, `scopes.rs` 428 — no split-before-extend needed; `store_bridge.rs` edits are localized to the two named fns.
- **Watcher normalization caveat:** threshold watchers compare normalized values (`raw / max`, `state_crossings.rs:81-92`). Direction is unaffected (monotone transform), but any future `value` param must decide raw-vs-normalized — one more reason it waits for a use case.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| crossing direction input | `DispatchScope` crossing vocabulary `("@rising", Bool)` | `{"op":"input","name":"@rising"}` | `on.rising` on `CrossingParams` | same table shape |
| tick dt input | `DispatchScope` tick vocabulary `("@dt", Number)` | `{"op":"input","name":"@dt"}` | `t.dt` on `TickParams` | same |
| both-edge option | `CrossingDescriptor` optional edge field | `"edge":"both"` (absent = shipped single-edge) | threshold: `{ above, edge: "both" }`; predicate: trailing options `{ edge: "both" }` | same |
| slot accumulator | `SlotSchema.accumulate: Option<IrNode>` | slot schema key `"accumulate"`: a `RuntimeValue` node (the per-tick delta) | `accumulate: (t: TickParams) => RuntimeValue`, traced by `defineStore` | same |
| ref-as-operand | n/a (arrives as `input` node) | `{"op":"input","name":"<slot>"}` | `runtime.read(ref)` or bare `StateRef` operand | same |
| scope marker | n/a (bind-time enforcement) | none — no wire scope tag | `Reaction<S>` phantom (type-level only) | n/a |

No new binary/PRL section, no FGD KVP. Captured dispatch values ride the existing system-command queue as `(String, IrValue)` pairs (state the constraint: entities-crate command, foundation value type — exact field layout implementation-chosen).

## Script syntax examples

```ts
import { defineStore, defineReaction, onStateCrossing, updateState, occupants, getGameState, runtime, type CrossingParams } from "postretro";

// King-of-the-Hill: countdown drains 1/sec per body on the hill. `t.dt` is the
// ephemeral TickScope param; `occupants("hill")` is the ambient readonly ref
// (E18--coop-activation-policy) — ambient and ephemeral compose side by side.
const koh = defineStore("koh", {
  countdown: {
    type: "number", default: 60, range: [0, 60],
    accumulate: (t) => runtime.mul(t.dt, runtime.sub(0, occupants("hill"))),
  },
});

const win = defineReaction({ primitive: "playSound", args: { sound: "fanfare" } });

// Direction-sensitive shield: one crossing, both edges, one reaction.
const shield = defineStore("shield", { up: { type: "boolean", default: false } });
const shieldToggle = defineReaction((on: CrossingParams) =>
  updateState(shield.state.up, runtime.select(on.rising, false, true))); // up while health is low

export function setupLevel() {
  const crossings = [
    onStateCrossing(runtime.le(runtime.read(koh.state.countdown), 0), [win]),
    // max normalizes the watch: fires crossing 25% of 100 HP, both directions.
    onStateCrossing(getGameState().player.health, { below: 0.25, max: 100, edge: "both" }, [shieldToggle]),
  ];
  return { reactions: [win, shieldToggle], crossings };
}
```

```luau
-- Luau parity: same tracer, same wire. No annotations — the engine bind gate enforces scope.
local Postretro = require("postretro")
local shield = Postretro.defineStore("shield", { up = { type = "boolean", default = false } })
local shieldToggle = Postretro.defineReaction(function(on)
  return Postretro.updateState(shield.state.up, Postretro.runtime.select(on.rising, false, true))
end)
```
