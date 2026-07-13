# Code-grounding notes (2026-07)

Source-verified facts. Line refs as-of drafting; pointers, not contracts. `incrementState`/`decrementState` mirror the two shipped surfaces of `setState`.

## `setState` has two surfaces — copy both

- **System path:** `setState` is a system reaction registered in `register_system_reaction_primitives` (`crates/postretro/src/scripting/systems/system_reactions.rs`, fn ~line 33; handler ~207) that pushes `SystemReactionCommand::SetState { slot, value }` (enum imported from `reaction_registry`, `system_reactions.rs:11-13`). Drained in `App::dispatch_system_commands` (`crates/postretro/src/main.rs:4005`); the `SetState` arm (~`:4126`) writes via `write_state_slot_json` (`main.rs:4126-4137`), which **gates readonly** (`crates/scripting-core/src/store_bridge.rs:111-114`). Slot-table access for the read-modify-write is `script_ctx.slot_table` (`Rc<RefCell<SlotTable>>`).
- **Bound-in-tick path:** for a trigger `on_fire`/`on_exit`, `setState` binds to `BoundTriggerCommand::StoreSlot { slot, value }` (`crates/postretro/src/trigger_bindings.rs:111-114`) via `bind_store_slot` (`:621-662`), value precomputed and validated at **bind** (`json_value_for_slot` → `validate_slot_value`, `:646-657`; readonly rejected at bind). Execute is `TriggerBindingTable::execute` (`:231-237`) → `BoundTriggerCommand::execute` (`:277`), which receives `slot_table: &mut SlotTable` (a plain borrow — **not** `Rc<RefCell<SlotTable>>`); the `StoreSlot` arm calls `apply_store_slot_batch(slot_table, ...)` at `:296-302`.

The `Rc<RefCell<SlotTable>>` shape belongs to the app-side drain (`store_bridge.rs:107` `ctx.slot_table.borrow_mut()`), not the in-tick trigger path. Do not conflate them.

## Write paths differ in readonly enforcement

`apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs:81-100`) validates/clamps a Number into `schema.range` via `validate_slot_value` (`:553-568`) but performs **no** readonly gate. `write_state_slot_json` gates readonly (`:111-114`). So: the bound path (readonly rejected at bind) may write via `apply_store_slot_batch`; the system path (no bind step) must write via `write_state_slot_json` to refuse read-only at runtime. Current-value read is `SlotTable::get` (`crates/entities/src/slot_table.rs:328`) → `record.value` (no `read_number` on `SlotTable`; that helper is private to `state_crossings.rs`). `SlotValue::Number` at `slot_table.rs:11`; `NumericRange{min,max}` at `:30-33`; `SlotSchema.range` at `:66`.

## Consequential classification — two lists

- `CONSEQUENTIAL_PRIMITIVES` (`crates/postretro/src/trigger_bindings.rs:25-35`; `setState` at `:33`), consumed by `classify` (~497-505) — authoritative.
- Debug mirror `is_trigger_consequential_primitive` (`crates/scripting-core/src/reaction_dispatch.rs:301-314`; `setState` at `:311`), asserted by `debug_assert!`s at `reaction_dispatch.rs:203,215`. A stale mirror trips the assert. Add both new names to both.

## Non-trigger firing contexts already in use (parity consumers)

`setState`/reactions already fire from these paths in shipped content, so `incrementState` must reach them too:

- `levelLoad` reactions: `content/dev/scripts/fog-pulse-demo.ts`, `arena-lights.ts`, `anim-demo-reaction.ts`.
- Named events + progress thresholds: `content/dev/scripts/combat-demo-reaction.ts` (`defineReaction(RETALIATION_EVENT, { progress: { tag, at, fire } })`, fired when `killed/total >= at`).
- State crossings: `arena-lights.ts` (`onStateCrossing`).
- `playerDied`: `PLAYER_DIED_EVENT = "playerDied"` (`crates/postretro/src/scripting/systems/health.rs:25`), pushed as a named event at `sim/mod.rs:647`; `scripting.md:331` — mods bind it to their own game-flow policy. The likely first delta consumer (E18-R players-alive counter).

## Validation / SDK

- `validate_primitive_name` (`crates/foundation/src/data_descriptors/validate/foundation.rs:18`) only rejects an empty name — **no allowlist**, so a new primitive name needs no central registration to pass authoring validation. A non-trigger firing of a primitive with no runtime handler warns-and-skips (`reaction_dispatch.rs:409,465`).
- SDK `setState`/`updateState` live in `sdk/lib/ui/reactions.{ts,luau}` (`updateState` `reactions.ts:309` / `.luau:263`), returning `PrimitiveReactionDescriptor`; not in the `SequenceStep` union. The reaction-body primitive list is hand-authored in `crates/scripting-core/src/typedef/templates/ui_sdk_module.d.ts:176-208`; `gen_script_types` (`crates/postretro/src/bin/gen_script_types.rs`) does not auto-emit names.
