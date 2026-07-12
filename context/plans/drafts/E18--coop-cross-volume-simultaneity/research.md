# Code-grounding notes (2026-07)

Source-verified facts behind the spec. Line refs are as-of drafting; pointers, not contracts. Rides entirely on E18-A machinery (`context/plans/done/E18--trigger-event-fanout/`).

## Slot deltas mirror `setState`, which has TWO surfaces

`setState` is not a single mechanism — copy both halves:

- **System path:** `setState` is a *system* reaction registered in `crates/postretro/src/scripting/systems/system_reactions.rs` (~line 33; `setState` handler ~line 207) that pushes `SystemReactionCommand::SetState { slot, value }`; the command drains app-side in `App::dispatch_system_commands` (`main.rs`). A `ReactionPrimitiveFn` (`Fn(&mut EntityRegistry, &[EntityId], &Value)`, `crates/scripting-core/src/reaction_registry.rs:21`) has no slot-table access, so slot writes cannot register there (the `armTrigger`/`disarmTrigger` site is the wrong precedent). The delta needs a sibling system command (`AddState { slot, by }`) with read-modify-write in the drain.
- **Bound-in-tick path:** for a trigger `on_fire`/`on_exit`, `setState` binds to `BoundTriggerCommand::StoreSlot { slot, value }` via `bind_store_slot` (`crates/postretro/src/trigger_bindings.rs`), value precomputed and validated at **bind**. A delta cannot precompute (current value unknown at bind); the new variant stores `{slot, by}` and reads current at **execute** (the executor holds the tick-context `Rc<RefCell<SlotTable>>`).

## Two consequential lists (both must list the new names)

- `CONSEQUENTIAL_PRIMITIVES` (`crates/postretro/src/trigger_bindings.rs`, ~25-35; `setState` at ~33), consumed by `classify` (~497-505) — the authoritative partition.
- Debug mirror `is_trigger_consequential_primitive` (`crates/scripting-core/src/reaction_dispatch.rs`, ~301-314; `setState` ~311) — a `debug_assert!` that residuals hold no consequential primitive. Stale mirror trips the assert.

## Write path (write-only; caller does the read)

`apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs:81-100`) validates each `(name, value)` via `validate_slot_value` and writes; it never reads current values. Clamp lives here — `validate_slot_value` clamps a `Number` into `schema.range` (`store_bridge.rs:563-572`). So the delta reads current itself (`slot_table` get/read_number), computes `current ± by`, then writes via this path. `SlotValue::Number` at `slot_table.rs:11`; `NumericRange{min,max}` at `:30-33`; `SlotSchema.range` at `:66`.

## Crossing math (the fixture is correct)

`build_crossing` (`crates/scripting-core/src/data_descriptors/mod.rs`) stores `threshold: above / max` (line 45; `below / max` line 35); `max` defaults to 1.0 (line 21). The watcher normalizes the observed value as `raw / max` (`state_crossings.rs:64,82`) and fires on `prev <= threshold && cur > threshold` (`:122`). So `above: 2.5, max: 3` → threshold 0.833; counter 3 → 1.0 fires, counter 2 → 0.667 does not. The authored `above` is a **raw count**, divided by `max` at build time — not a pre-normalized fraction. Verified against the test helpers (`state_crossings.rs:196,212`, `raw_threshold / max`).

## State declaration + SDK

- `setupLevel` returns `LevelManifest { reactions, crossings, uiTrees }` (`sdk/lib/data_script.ts:84-92`) — **no `state` field**. Slots declare via `defineStore` → `StoreDeclaration { namespace, schema }` (`:97-100`), per-slot `StoreSlotSchema` (`:95`); shared-global is the `network` attribute on a slot. Mod-facing literal is `network: "shared"` (maps to `ReplicationScope::SharedGlobal`).
- SDK builders: `setState`/`updateState` live in `sdk/lib/ui/reactions.{ts,luau}` (`updateState` at `reactions.ts:309` / `.luau:263`), **not** `world.*` (134 lines, no reaction builders). They return `PrimitiveReactionDescriptor` (system-targeted, no `tag`); they are not `SequenceStep`s (union at `sdk_lib.d.ts:304`, entity-targeted only). `gen-script-types` does not auto-emit primitive names; the primitive doc list is hand-authored in the typedef template.

## Client converge / mover authority

- `ClientStateApply::apply_snapshot_state` (`crates/postretro/src/netcode/state_slots.rs:674`) is the P3.5 client converge path.
- Mover commands are consequential/replicated; clients receive mover phase, they do not evaluate triggers or author movers (`networking.md`). So `solve` (a mover start) fires host-side via the host's crossing detector and reaches clients through replication — driving it from each client's local crossing would fight host authority. The per-client crossing channel is for presentation reactions only.
