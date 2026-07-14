# E18 — IR-Valued Reaction Arguments & Crossing Predicates

## Goal

Let reaction `setState` args and state-crossing conditions be `RuntimeValue` IR expressions over live store slots, evaluated by the engine at fire time — not frozen literals. This retires the need for dedicated `incrementState`/`decrementState` reactions (`setState(slot, runtime.add(runtime.read(slot), 1))` covers them) and gives crossings a multi-slot boolean predicate. It is the reaction-side adoption of the shipped typed command buffer (`scripting.md` §11), binding against `StoreScope` exactly as movement's dash binds against `MovementScope`. Unblocks the richer §12 observer and every counter consumer (players-alive, monsters-alive) that `state-delta-reactions` was carrying.

## Scope

### In scope

- **IR-valued `setState`.** `updateState(ref, value)` and the `setState` primitive accept `value: number | boolean | string | RuntimeValue`. A `RuntimeValue` deserializes to `IrNode`, wraps as `BakedIr { output: <slot>, root }`, binds against `StoreScope` at level install (type + readonly validated there), and evaluates against the live slot table at the write point.
- **Two execution surfaces, mirroring shipped `setState`.** System-command path (fired from a crossing, named event, `levelLoad`): the read-modify-write runs on the app-side drain. In-tick trigger path (`on_fire`/`on_exit`): runs inside the sim tick against the tick-context slot table. Both bind once at install and evaluate at the write point.
- **Same-frame accumulation correctness.** Evaluation happens at the write point in enqueue order, not at dispatch enqueue, so two increments firing the same frame each observe the prior write (a counter reaches +2, not +1).
- **IR-predicate crossings.** Generalize `onStateCrossing` (per §12 — one builder, not a second name) with a predicate overload: `onStateCrossing(predicate, fire[])` where `predicate` is a `Bool`-typed `RuntimeValue` over N slots (the single `ref` folds into the predicate via `runtime.read(slot)`). The shipped threshold form `onStateCrossing(ref, {above|below}, fire[])` stays. The runtime watcher binds the predicate against `StoreScope` at initialize and fires on the rising edge (predicate `false`→`true`), re-arming on `true`→`false` — the shipped edge lifecycle, generalized from the single-slot `above`/`below` threshold.
- **Readonly + type gating at bind** via `StoreCapability::Script` (readonly slots reject at bind; non-projectable slot types reject), reusing the shipped store write contract.
- **Retire `state-delta-reactions`.** Delete that draft; document `increment`/`decrement`/`clamp-to-range` as the `runtime`-builder pattern (the write path already validates and clamps to a slot's declared range).
- SDK builders + typedef regen + drift check + TS/Luau parity.

### Out of scope

- **Dispatch-scope params** — `ContactScope.activators`, `CrossingScope.direction`/`crossed`, `TickScope.dt` (§12). Reading ephemeral dispatch inputs needs a scope that composes store slots with source-published inputs; this spec binds against `StoreScope` (store slots only). Separate spec.
- **The `slot.integrate((t) => expr)` per-tick accumulator** (§12) — depends on `TickScope`; separate spec.
- **New IR opcodes.** The shipped node set (`const`/`input`/arithmetic/`clamp`/`lerp`/comparisons/`select`) is the whole vocabulary; no additions.
- **Non-`StoreScope` binding** (movement's `MovementScope` is unchanged and separate).
- Runtime-authored deltas that vary per fire beyond what IR-over-slots expresses.

## Acceptance criteria

- [ ] `updateState(ref, runtime.add(runtime.read(slot), 1))` fired twice in one frame raises the slot by 2 (same-frame accumulation); each surface independently reaches +2 whether fired from a crossing (app-drain) or a trigger `on_fire` (in-tick) — same end value, not a claim about cross-surface ordering.
- [ ] A derived write — `setState` whose value reads other slots (e.g. `runtime.clamp(runtime.sub(read(a), read(b)), 0, 100)`) — writes the evaluated result, validated and clamped to the target slot's declared range.
- [ ] A `setState` IR targeting a readonly slot is rejected at bind (level install) with a warning and never writes; an IR value targeting a non-projectable slot type (e.g. a string slot) likewise rejects at bind.
- [ ] A literal `updateState(ref, 5)` still works unchanged (the literal path is preserved; no regression to shipped `setState`).
- [ ] `onStateCrossing(predicate, [reaction])` (predicate overload) fires its reactions once when the predicate transitions false→true, and re-arms only after it returns to false — verified for a two-slot AND (`runtime.ge(read(a), 2)` AND `runtime.ge(read(b), 1)` via nested `select`/comparison) firing only when both hold. The shipped threshold form is unaffected.
- [ ] A predicate that never type-checks to `Bool` at bind is rejected with a warning and registers no watcher.
- [ ] Determinism: two identical headless runs produce identical slot timelines and identical crossing-fire sequences for both IR-valued writes and IR predicates.
- [ ] Typedef drift check passes with the widened `setState`/`updateState` value type and the `onStateCrossing` predicate overload in `postretro.d.ts` / `.d.luau`; a TS and a Luau fixture author an identical increment-and-predicate flow.
- [ ] `state-delta-reactions` no longer exists as a draft; no `incrementState`/`decrementState` symbol ships.

## Tasks

### Task 1: IR-valued `setState` — bind against `StoreScope`, evaluate at the write point

Widen the `setState` value from a frozen literal to a literal-or-IR, mirroring movement's dash-field pattern. **SDK:** `updateState<T>(ref, value)` (`sdk/lib/ui/reactions.ts:309`) accepts `value: T | RuntimeValue`; the emitted descriptor carries the `RuntimeValue` node (or the bare literal) under `args.value`. **Deserialize:** in the `setState` arg parse, branch on whether `value` is an IR node object vs a bare literal, reusing `ir_node_from_json` (`crates/foundation/src/data_descriptors/validate/foundation.rs:55`) and the `NumberOrIr`/`validate_dash_expr` shape (both defined in the **foundation** crate — `NumberOrIr` at `crates/foundation/src/data_descriptors/types/movement.rs:163`, `validate_dash_expr` at `crates/foundation/src/data_descriptors/validate/foundation.rs:29`; the scripting-core readers `read_dash_number_js` (`js/movement.rs:406`, returns `NumberOrIr`) and `read_dash_bool_js` (`:446`, returns `BoolOrIr`) only consume them). Arg-parse produces a type-agnostic `IrNode`; number-vs-boolean is settled by `StoreScope` type projection at bind against the target slot's declared type — the parse does not need the slot type. **Bind:** wrap the parsed `IrNode` as `BakedIr { version: CURRENT_IR_VERSION, output: Some(slot), root }` (the epoch is stamped here at wrap-time; the inline `args.value` node carries no version of its own) and bind against `StoreScope::script(ctx)` (`crates/scripting-core/src/ir/scopes.rs:62`) at level install — type projection and the readonly deny (`scopes.rs:104`) do the validation the current readonly gate did. **System path:** replace the frozen `SystemReactionCommand::SetState { slot, value: serde_json::Value }` (`crates/entities/src/reactions/system_commands.rs:44`, enqueued at `crates/postretro/src/scripting/systems/system_reactions.rs:212`, handler registered `:207`) so a bound-IR value is carried to the app drain and `eval_and_write` (`crates/foundation/src/ir/eval.rs:41`) runs there against a script-capability `StoreScope` over the live slot table, in enqueue order. **In-tick path:** extend `BoundTriggerCommand::StoreSlot { slot, value: SlotValue }` (`crates/postretro/src/trigger_bindings.rs:113`) to carry a bound IR alternative, evaluated in-tick via `eval_and_write` against a `StoreScope` over the tick slot table. A literal value keeps the shipped fast path in both surfaces. Plumbing: the install-time bind needs `ScriptCtx` (already threaded to reaction/trigger install); store the `BoundProgram` beside the command so no per-fire re-bind occurs.

### Task 2: IR-predicate crossings

Generalize the crossing watcher from an `above`/`below` literal to a `Bool` IR predicate. **Mind the crate boundary:** the descriptor/condition types live in the **entities** crate (`CrossingCondition` / `CrossingDescriptor` at `crates/entities/src/data_descriptors/types/reactions.rs:69` / `:86`), which depends only on `foundation`; `StoreScope` and `BoundProgram<StoreScope>` are **scripting-core** types. So the raw predicate (`IrNode`, a foundation type) is carried on the descriptor/condition, and the *bound* `BoundProgram<StoreScope>` lives only on the private `Watcher` (scripting-core) — mirroring how `Watcher` already holds derived `previous`/`max` state the descriptor does not. **SDK:** add a predicate overload `onStateCrossing(predicate: RuntimeValue, fire: (NamedReactionDescriptor | string)[]): CrossingDescriptor` beside the shipped threshold form `onStateCrossing(ref, condition, fire)` (`sdk/lib/ui/reactions.ts:79`), emitting a `CrossingDescriptor` predicate variant. The shipped descriptor is `{ slot, max?, fire, levels? } & ({above}|{below})` (`max` = the shipped optional fire-count cap; see `reactions.ts:16`); the predicate variant carries a `predicate` node in place of `slot`/`above`/`below`. **Pin the wire discriminant:** the deserializer distinguishes the two forms by the presence of `predicate` (IR form) vs `slot` (threshold form) — no threshold descriptor carries a `predicate` and vice versa. The shipped threshold form stays. **Entities-crate edit:** add an `Ir(IrNode)` variant to `CrossingCondition` (`reactions.rs:69`) carrying the raw predicate node, and drop the `Copy` from its `#[derive(Debug, Clone, Copy, PartialEq)]` (`reactions.rs:68`) since `IrNode` is not `Copy` — convert the by-value copy at `state_crossings.rs:110` to a `.clone()`; `IrNode` is `PartialEq`, so the descriptor/`Watcher` `PartialEq` derives still hold. **scripting-core edit:** extend the private `Watcher` in `crates/scripting-core/src/state_crossings.rs` (edge test at `:119`) to hold the bound `BoundProgram<StoreScope>` for the `Ir` case; at `initialize` (`:54`), bind the condition's `IrNode` against `StoreScope::script(ctx)` — type-check it to `Bool`, warn+skip if it does not (mirroring the non-Number-slot skip). Plumbing: `initialize` (`(&mut self, &DataRegistry, &SlotTable)` today) must additionally receive `ScriptCtx` to construct `StoreScope::script(ctx)` — thread it in from the crossing-install call site (the same `ScriptCtx` Task 1 threads to reaction/trigger install); the bound program is stored on the `Watcher` so no per-frame re-bind occurs. Each frame in `detect` (`:79`), `eval_value` the predicate against the slot table; a watcher fires on `false`→`true` and re-arms on `true`→`false`, reusing the existing `previous`-endpoint lifecycle. The predicate reads slots by name through `StoreScope`, so it needs no single `slot` field.

### Task 3: SDK typedefs, parity, and `state-delta-reactions` retirement

Land the author surface and remove the superseded draft. Regenerate `sdk/types/postretro.d.ts` / `.d.luau` via `gen-script-types` for the widened `setState`/`updateState` value type and the new `onStateCrossing` predicate overload, and update the committed snapshot + drift test. Ensure the Luau `updateState`/`onStateCrossing` builders and the `runtime` Luau twin canonicalize to byte-identical IR (the `wrap`-rule parity already asserted for `runtime`). Delete `context/plans/drafts/E18--state-delta-reactions/`; add a short "counters and derived state" note to the reaction docs showing `updateState(ref, runtime.add(runtime.read(slot), 1))` for increment, `runtime.sub(...)` for decrement, and `runtime.clamp(...)` (or the slot's declared range) for bounding — so the retired reactions' use cases have a documented home.

### Task 4: Tests and fixtures

Prove both surfaces and the accumulation contract. Unit/e2e: same-frame double-increment reaches +2; a derived clamped write lands the evaluated value; a readonly-target IR rejects at bind; a literal `setState` is unchanged; a two-slot AND predicate fires only when both hold and re-arms correctly; a non-Bool predicate is rejected. Parity: a TS fixture and a Luau fixture author an identical increment-and-predicate flow and produce identical IR. Determinism: extend the headless harness with an IR-write and an IR-predicate tick sequence in the green-and-stays-green gate.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the `RuntimeValue`→`BakedIr`→`StoreScope` bind/eval path for reactions; Task 2 reuses it.
**Phase 2 (sequential):** Task 2 — crossing predicate, on Task 1's bind path.
**Phase 3 (sequential):** Task 3 — typedef regen + parity + draft retirement; consumes the Task 1/2 wire shapes.
**Phase 4 (sequential):** Task 4 — tests/fixtures over the finished surfaces.

## Rough sketch

- **The BakedIr `output` field is the target slot.** `setState`-with-IR is a `BakedIr { output: Some(slot), root }`; `eval_and_write` reads inputs and writes the root value to `output`. This is why one node type covers both "compute" and "write" — no new command semantics.
- **Eval-at-write-point, not eval-at-enqueue,** is load-bearing for counters: the app drain and the in-tick executor evaluate in enqueue order so sequential reads observe prior writes. Enqueuing a pre-evaluated value would lose same-frame updates.
- **Reuse, don't re-invent, the movement precedent:** the scripting-core readers `read_dash_number_js` / `read_dash_bool_js` (`crates/scripting-core/src/data_descriptors/js/movement.rs:406`, `:446`) over the foundation-crate `NumberOrIr`/`BoolOrIr`/`validate_dash_expr`, plus `ir_node_from_json`, `load_baked_ir` (`foundation/src/ir/load.rs:51`). The only new axis is the scope: `StoreScope` (`scopes.rs:45`) in place of `MovementScope`.
- **Oversized-file note:** `crates/postretro/src/scripting/systems/system_reactions.rs` (999) and `crates/postretro/src/trigger_bindings.rs` (986) are past ~800 but are cohesive handler tables; the edits here are localized (one handler / one command variant), so no split-before-extend — flagged, judged cohesive.
- **Predicate-overload shape:** the predicate form reads N slots, so it hangs on `onStateCrossing`'s first argument as a `RuntimeValue` (the ref folds into the predicate via `runtime.read`), overloading rather than replacing the shipped threshold signature — the §12 single-builder generalization.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| setState value | literal `SlotValue` **or** `BakedIr { output: slot, root }` bound against `StoreScope` | `args.value`: bare literal, or a `RuntimeValue` op-node tree (`{op,...}`, the shipped IR wire shape) | `updateState(ref, value: T \| RuntimeValue)` | same |
| crossing predicate | `CrossingCondition::Ir(IrNode)` beside `Below`/`Above` (entities crate); bound `BoundProgram<StoreScope>` held on the private `Watcher` (scripting-core) | `CrossingDescriptor` predicate variant, discriminated by presence of `predicate` (vs `slot`), carrying a `RuntimeValue` node | `onStateCrossing(predicate, fire[])` (predicate overload) | same |

No new binary/PRL section: IR crosses as the shipped `BakedIr`/`RuntimeValue` JSON, version-epoch-validated at load (`CURRENT_IR_VERSION`). No FGD KVP.

## Script syntax examples

```ts
import { world, defineReaction, onStateCrossing, updateState, runtime, defineStore } from "postretro";

const puzzle = defineStore("puzzle", {
  charge: { type: "number", default: 0, range: [0, 3] },
});

export function setupLevel() {
  // Increment: a derived write over the slot's own value. Retires incrementState.
  const bumpCharge = defineReaction(updateState(puzzle.state.charge, runtime.add(runtime.read("puzzle.charge"), 1)));

  const door = world.query({ component: "kinematic_mover", tag: "gate" });
  const openGate = defineReaction({ sequence: door.flatMap((d) => d.start()) });

  // Predicate crossing: fire when charge reaches its cap AND a lever slot is set.
  const crossings = [
    onStateCrossing(
      runtime.select(runtime.ge(runtime.read("puzzle.charge"), 3), runtime.read("puzzle.lever"), false),
      [openGate],
    ),
  ];

  return { reactions: [bumpCharge, openGate], crossings, stores: [puzzle] };
}
```

