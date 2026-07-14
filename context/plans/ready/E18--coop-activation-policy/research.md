# E18-B research notes

Code-anchored facts behind the spec. Line refs are as-of drafting — pointers, not contracts; reconfirm before editing. Prior activation-gate/KVP/format anchors were dropped with the all-script rescope (see git history if the KVP direction ever returns). Design rationale: `context/research/co-op-triggers-trap-pools.md` §4.3.

## Trigger occupancy (shipped, E18 trigger-event-fanout)

- `TriggerSystem::occupancy(&self, trigger: EntityId) -> usize` — `crates/postretro/src/trigger_system.rs:109`. `#[cfg(dev-tools)]`-gated, **no non-test caller**. Returns raw overlap count (`occupants[trigger].len()`), **not** effective — no alive filter.
- Occupant map: `occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` on `TriggerSystem` (`:94-98`), private. Overlap math `canonical_player_capsules` (`~:317`) reads only `Transform` + `PlayerMovementComponent` — no health read today (the corpse-on-a-plate gap E18-B closes).
- Fire stream: `TriggerFireReport { fires: Vec<TriggerEvent> }`; `TriggerEvent { fire: TriggerEventFire { trigger, player, event_name }, edge: TriggerEventEdge::{Enter,Exit} }` (`:41-90`). `enters()`/`exits()` are `#[cfg(test)]` only.
- File is **1695 lines** → split-before-extend.

## Activator available but dropped (informs the deferred activators work, not E18-B)

- Dispatch closure `crates/postretro/src/sim/mod.rs:179-197` has `event.fire.player: PlayerId` in scope but calls `TriggerBindingTable::execute(trigger, edge, registry, slot_table)` (`trigger_bindings.rs:231`) with no player. No `BoundTriggerCommand` carries an activator.
- `PlayerId` (`trigger_system.rs:19-24`): `enum { Local(EntityId), Remote(u64) }`, derives `Ord + Hash + Copy`, `pub(crate)`. `Local` wraps the pawn `EntityId`; `Remote(client_id)` needs `AuthoritativePlayer { id, pawn }` (`:26-30`) to reach the pawn.
- `players: Vec<AuthoritativePlayer>` built at `sim/mod.rs:156-170` (Remote from `RemotePawnCommand::owner_client_id`, Local from `registry.local_player_movement_pawn()`). This is the list the aggregation pass uses to map occupant `PlayerId`→pawn for the alive check.

## Aliveness

- **No `is_alive`/`is_dead` helper exists.** Dead ⇔ `current <= 0.0 || !current.is_finite()` (the `sweep_deaths` predicate, `crates/postretro/src/scripting/systems/health.rs`). `HealthComponent` (`crates/entities/src/components/health.rs:230-252`) is `ComponentKind::Health = 10` (`registry.rs:107`); read via `registry.get_component::<HealthComponent>(pawn)`.
- **Absent health component ⇒ treat as alive** (movement-only test pawns have no health). Corpses persist (players never despawn); `death_handled` is a one-shot latch.
- `player.health` slot is a readonly local-pawn display slot, not a per-activator source.

## State-crossing watcher (the observer E18-B rides — unchanged)

- `CrossingDetector` — `crates/scripting-core/src/state_crossings.rs:36` (file 425 lines). Private `Watcher { slot, condition: CrossingCondition::{Below,Above}{threshold}, max, fire, previous }`.
- Reads the **`SlotTable` directly** via `read_number` (`:138`); type-guards Number slots at `initialize` (`:54`), warns+skips non-Number.
- Edge test `Watcher::crosses` (`:119`): `Above` fires on `prev <= threshold && cur > threshold`, re-arms only after crossing back. `detect(&SlotTable) -> Vec<String>` returns fire names; driven by `dispatch_state_crossings_with_sequences` (`crates/postretro/src/scripting/reactions/mod.rs:32`) after the tick's slot writes.
- **An integer occupancy count crossing N-1→N fires an `above: N-1` watcher — no IR predicate needed.** This is why E18-B is IR-free and self-contained.

## world.query trigger surface (additive; nothing to remove)

- `collect_trigger_volume_handles_json` (`crates/postretro/src/scripting/entity_world_primitives.rs:269-299`) emits `{id, position, tags}` only; test `:739` asserts no runtime state. `parse_query_filter` maps `"trigger_volume"→QueryFilter::TriggerVolume{tag}` (`:85`).
- SDK `TriggerVolumeHandle` (`sdk/lib/entities/triggers.ts:12`) has `arm()`/`disarm()` only; `wrapTriggerVolumeEntity` (`:19`); `world.ts:105` delegates trigger snapshots.
- `ReadonlyStateRef<T>` (`sdk/lib/ui/widgets.ts:46`): runtime shape exactly `{ slot: string }`; `stateSlot` (`sdk/lib/ui/reactions.ts:33`) extracts it into descriptors. `defineStore` builds state refs the same way (`sdk/lib/data_script.ts:280`, frozen `{ slot }`). No `ReadonlyStateRef` is produced by `world.query` today — occupancy refs are net-new.

## Engine-capability slot writes / typedefs

- Engine writes bypass readonly with validation via `write_store_slot` / `apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs`); `StoreCapability::Script` denies readonly at `crates/scripting-core/src/ir/scopes.rs:104`. E18-B occupancy slots are engine-written, script-readonly.
- SDK helper signatures reach `sdk/types/postretro.d.ts` via typedef templates under `crates/scripting-core/src/typedef/templates/` (e.g. `onStateCrossing` at `ui_sdk_module.d.ts:174`), regenerated by `gen-script-types` with a `cargo test` drift check.

## Dev-tools Triggers tab

- `collect_trigger_diagnostics_rows(registry, bridge, trigger_system, bindings)` — `crates/postretro/src/trigger_diagnostics.rs:21-59`; already reads `trigger_system.occupancy(id)` at `:49`.
- `TriggerDiagnosticsRow` — `crates/renderer/src/render/debug_ui/mod.rs:97-110`; `draw_triggers_tab` `num_columns(10)` at `:709`. Overlay label path `collect_trigger_overlay_labels` also reads occupancy.
