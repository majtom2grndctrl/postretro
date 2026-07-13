# E18 — State-Delta Reactions

## Goal

Add two general consequential reactions — `incrementState` / `decrementState` — that add to and subtract from a numeric state slot, usable from **any** reaction context the way `setState` already is: trigger `on_fire`/`on_exit`, state crossings, named events (`playerDied`, progress thresholds), and `levelLoad`. This is shared co-op plumbing, deliberately decoupled from any one consumer: cross-volume simultaneity counters (`E18--coop-cross-volume-simultaneity`), a players-alive counter (E18-R), and a monsters-alive counter (E18-C) all ride on it. Mirrors `setState`'s two shipped execution surfaces so the reactions work identically whether fired in-tick from a trigger or app-side from a crossing/event.

## Scope

### In scope

- `incrementState` / `decrementState` reactions: read a numeric slot's current value, apply a signed delta, validate-and-clamp to the slot's declared range, write it back. `incrementState` applies `+by`; `decrementState` applies `-by`; `by` is a positive magnitude, default 1; a negative or non-finite `by` is rejected. Args `{slot: string, by?: number}`. The default is materialized Rust-side: when `by` is omitted the SDK emits `args: {slot}` and both the system handler and the trigger binder substitute 1, so TS and Luau emit identical descriptors.
- **System-command path** (the general path): fired from a crossing, named event, progress threshold, or `levelLoad`; the read-modify-write runs on the app-side system-command drain, through the same readonly-gated write path `setState` uses.
- **Bound-in-tick trigger path**: a trigger's `on_fire`/`on_exit` executes the read-modify-write inside the sim tick against the tick-context slot table; readonly/non-numeric targets reject at bind.
- SDK system-reaction body builders (`incrementState`/`decrementState`), both runtimes, plus typedefs and the drift check.

### Out of scope

- The cross-volume simultaneity fixture and any puzzle authoring — `E18--coop-cross-volume-simultaneity` owns that.
- Any specific counter consumer (players-alive is E18-R; monsters-alive/kill-progress is E18-C).
- Non-numeric slot deltas; a runtime-authored `by` that varies per fire (bound at install/parse).
- A `multiplyState`/`scaleState` or other arithmetic verbs — add only when a consumer needs them.

## Acceptance criteria

- [ ] `incrementState` with `by: 1` raises a numeric slot by 1; `decrementState` lowers it by 1; both validate-and-clamp to the slot's declared range (a decrement below `min` floors at `min`, not negative; an increment past `max` caps). A negative or non-finite `by` is rejected on both paths.
- [ ] **Bound path, positive:** a trigger `on_fire = incrementState` raises the counter within the same sim tick it fires (observable in a headless `simulate_tick` test with no app loop).
- [ ] **System path, positive:** `incrementState` fired from a state crossing (or a named event) raises the counter via the system-command drain, with no trigger involved.
- [ ] Read-only slot: an `incrementState`/`decrementState` bound to a trigger rejects at bind time with a warning and binds nothing; fired as a system reaction, it warns and no-ops (the drain uses the readonly-gated write path, not a raw slot write). A non-numeric current value no-ops on both paths.
- [ ] Two triggers whose `on_fire` increments the same counter on one tick both apply (final value rises by 2) — read-modify-write in stable `(trigger, player)` order, deterministic across two identical headless runs.
- [ ] SDK: the typedef drift check passes with regenerated `.d.ts`/`.d.luau`; the same increment/decrement reaction authors identically from a TS mod and a Luau mod.

## Tasks

### Task 1: System-command path

The general path, mirroring `setState`'s system-reaction shape. Register `incrementState`/`decrementState` in `register_system_reaction_primitives` (`crates/postretro/src/scripting/systems/system_reactions.rs`, the fn at ~line 33; `setState` handler ~line 207) as system reactions that push a new `SystemReactionCommand::AddState { slot, by }` variant (the enum is imported from `reaction_registry`; add the variant there). Handle `AddState` in `App::dispatch_system_commands` (`crates/postretro/src/main.rs`, ~line 4005, beside the `SetState` arm at ~line 4126): read the current `SlotValue::Number` from `script_ctx.slot_table` (the app-side `Rc<RefCell<SlotTable>>`), compute `current + by`, where `decrementState` negates the validated positive magnitude at command construction so `AddState.by` carries the signed delta, and write it through the **readonly-gated** write path `SetState` uses — `write_state_slot_json` (`main.rs` ~4126-4137, whose readonly gate lives at `crates/scripting-core/src/store_bridge.rs:111-114`), **not** `apply_store_slot_batch` (which validates and clamps range but has **no** readonly gate). Clamp to the slot's `NumericRange` comes from the write path's `validate_slot_value`. A non-numeric current value or absent slot warns and no-ops; a negative or non-finite `by` is rejected when the command is constructed in the handler.

### Task 2: Bound-in-tick trigger path + consequential classification

The in-tick path for trigger `on_fire`/`on_exit`. Add a `BoundTriggerCommand` delta variant (`crates/postretro/src/trigger_bindings.rs`) mirroring the shipped `StoreSlot` variant (`:111-114`) and its `bind_store_slot` binder (`:621-662`) — but, unlike `StoreSlot` which precomputes a validated `SlotValue` at bind, the delta variant stores `{slot, by}` and does the read-modify-write at **execute** time. The executor `BoundTriggerCommand::execute` (`:277`) receives `slot_table: &mut SlotTable` (a plain mutable borrow, borrowed once per tick by `TriggerBindingTable::execute` at `:231` — **not** an `Rc<RefCell<SlotTable>>`; that shape is the app-side drain's only): read current via `slot_table.get(slot)` → `record.value`, compute `current + delta` where the bound delta variant stores the signed delta (`decrementState` negates the positive magnitude at bind), write via `apply_store_slot_batch` (readonly is already rejected at bind for this path, so the range-only validation there suffices). The binder rejects a non-numeric or read-only target, and a negative/non-finite `by`, at bind time with a warning (mirroring `bind_store_slot`'s `validate_slot_value` bind-time checks). Because the bound path rejects a non-numeric target at bind, a declared-numeric slot's current value is always numeric at execute, so the bound path satisfies the non-numeric no-op by construction; only the system path needs the execute-time non-numeric guard (Task 1). Add `incrementState` and `decrementState` to **both** consequential lists: `CONSEQUENTIAL_PRIMITIVES` (`trigger_bindings.rs:25-35`, beside `setState` at `:33`) and its debug mirror `is_trigger_consequential_primitive` (`crates/scripting-core/src/reaction_dispatch.rs:301-314`, `setState` at `:311`) — the `debug_assert!`s at `reaction_dispatch.rs:203,215` trip on a stale mirror.

### Task 3: SDK surface + typedefs

Add `incrementState`/`decrementState` as system-reaction body builders following the `updateState` precedent — location `sdk/lib/ui/reactions.{ts,luau}` (mirroring the `updateState` builder, which emits the `setState` primitive, at `reactions.ts:309` / `.luau:263`), **not** `world.*`. They are slot-targeted system reactions returning a `PrimitiveReactionDescriptor` `{primitive, args: {slot, by}}`; they are **not** tag-targeted primitives and **not** `SequenceStep`s, so document them beside `setState`/`updateState`. Add the two primitive names by hand to the reaction-body primitive list in the typedef template `crates/scripting-core/src/typedef/templates/ui_sdk_module.d.ts` (~176-208, `updateState` at ~205 — `gen_script_types` does not auto-emit primitive names), regenerate via `gen-script-types`, and update the committed/snapshot typedef tests. Cover TS and Luau parity with a fixture authoring the same increment/decrement reaction in both runtimes.

## Sequencing

**Phase 1 (concurrent):** Task 1 (system path — `system_reactions.rs`, `main.rs`, `reaction_registry` enum), Task 2 (trigger-bindings path + classification lists), Task 3 (SDK/typedefs) — disjoint file surfaces. Task 2 owns the classification-list edit that the bound-in-tick path relies on for the consequential contract (the system path does not bind a trigger primitive and does not consult these lists).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| increment reaction | `SystemReactionCommand::AddState{slot, by}` (system path) + a `BoundTriggerCommand` delta variant (in-tick) | reaction primitive name `"incrementState"`; `args: {slot, by}` | `incrementState(slot, by?)` in `sdk/lib/ui/reactions.ts` | `.luau` twin | n/a |
| decrement reaction | ditto, negated `by` | `"decrementState"`; `args: {slot, by}` | `decrementState(slot, by?)` | `.luau` twin | n/a |

`by` defaults to 1 (a positive magnitude): the SDK omits `by` when it is at default and Rust substitutes 1; a negative or non-finite `by` is rejected. `incrementState` applies `+by`, `decrementState` applies `-by`. Clamp to the slot's range is enforced by the write path's validation on both surfaces; the readonly gate differs by path (bind-time for the bound path, `write_state_slot_json` runtime gate for the system path).

## Rough sketch

- Two write paths on purpose: the bound path already runs behind a bind-time capability check, so it writes via `apply_store_slot_batch` (range clamp only); the system path has no bind step, so it must write via the readonly-gated `write_state_slot_json` to refuse read-only slots at runtime. Both read the current value first — the system path from `script_ctx.slot_table.borrow()`, the bound path from the tick-context `&mut SlotTable`.
- The delta is a small closed operation, same shape as `setState`: no `ScriptCtx`, `DataRegistry`, or VM threads into the sim seam for the bound path.

## Open questions

None blocking.

- Sign is fixed in the verb, not the argument: `incrementState` adds, `decrementState` subtracts, `by` is always a positive magnitude. A single signed `addState` was considered and rejected — two named verbs read better at the authoring site and match the `setState`/`updateState` naming.
- Determinism rests on E18-B's stable `(trigger, player)` in-tick execution order, not on delta commutativity (clamped deltas are not commutative at saturation).
- Delta writes are count-sensitive, so E18-A's exactly-once in-tick guarantee and E18-B's paired enter/exit gating (each increment paired to exactly one decrement) are load-bearing here in a way they are not for idempotent setters — a doubled or dropped edge desynchronizes the count.
