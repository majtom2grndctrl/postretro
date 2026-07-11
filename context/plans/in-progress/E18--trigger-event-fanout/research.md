# Code-grounding notes (2026-07)

Source-verified facts behind the spec's decisions. Line refs are as-of drafting; treat as pointers, not contracts. Design rationale lives in `context/research/co-op-triggers-trap-pools.md`.

## Trigger system (shipped, E17-C)

- `TriggerSystem::run_authoritative_tick(&mut self, registry, bridge, players: &[AuthoritativePlayer], use_pressed: &HashMap<PlayerId, bool>, tick_dt)` — `crates/postretro/src/trigger_system.rs:53` (file: 536 lines).
- Gate: `evaluate_trigger_activation(state, activator) -> TriggerActivationDecision{Fire|Suppress}` at `:138`; condition `armed && !matches!(fire_mode, Once if latched) && rearm_remaining_ms <= 0.0`. `rearm_ms` is the authoring reload amount; `rearm_remaining_ms` the runtime countdown.
- Overlap tracking today: `prior_overlap: HashMap<(EntityId, PlayerId), bool>` — boolean only, no occupancy count, no leave edge consumer.
- `PlayerId { Local(EntityId), Remote(u64) }` at `:20`; `AuthoritativePlayer { id, pawn }` at `:26`.
- Use activation is a same-tick boolean check (`overlapping && use_pressed[player]`); the rising edge is computed upstream (sim comment `sim/mod.rs:37-40`). Exit-edge work must not assume an edge inside the system.
- Sim ordering (`crates/postretro/src/sim/mod.rs:104-169`): mover tick → movement → **trigger stage (`:135-159`)** → AI. `TriggerTickContext { system, bridge, use_edges }` at `sim/mod.rs:63-67`; `simulate_tick` param `trigger_context: Option<TriggerTickContext>` (signature `:86-103`, already `too_many_arguments`).
- Test module `trigger_system.rs:255-536` with a `#[cfg(test)]` gate-fire recorder proving the gate is the sole fire path — extend it for exits.

## Component / format / compiler / FGD

- `TriggerVolumeComponent` fields: `activation, target_tag, command, fire_mode, rearm_ms, enabled_on_spawn, armed, latched, rearm_remaining_ms` (`crates/entities/src/components/trigger_volume.rs:20-31`); `ComponentKind::TriggerVolume = 14`. Enums serde `snake_case`: `TriggerActivation{Touch,Use}`, `TriggerFireMode{Once,Multiple}`; `MoverCommand{Start,Stop,Reverse,GoToPathNode(String)}` from `kinematic_mover.rs:19`.
- `TriggerVolumeRecord` (`crates/level-format/src/trigger_volumes.rs:13-26`, "Field order is persistent wire layout"): name, tags, aabb_min/max, activation u8, target_tag, command u8, command_arg, fire_mode u8, rearm_ms f32, enabled_on_spawn bool. `TRIGGER_VOLUMES_VERSION: u16 = 1` (`:5`), LE cursor codec, u32-length-prefixed strings, hard version check + trailing-bytes check — appending fields **requires** the v2 bump and a decode branch.
- Compiler: `resolve_trigger_volume` at `crates/level-compiler/src/parse.rs:96` (file: **3,626 lines** → Task 1 split into `level-compiler/src/trigger_volumes.rs`, currently 26 lines). Unknown enum KVP values hard-bail; **missing `target_tag` silently defaults to `""`** (no inert check today); unknown KVPs silently ignored.
- FGD `trigger_volume` at `sdk/TrenchBroom/postretro.fgd:318-349`; note `enabled_on_spawn` default is bare int `1` while other choices defaults are quoted strings.
- Bridge: `TriggerVolumeBridge { aabbs: HashMap<EntityId, (Vec3, Vec3)> }`, `populate_from_level(&mut self, registry, records)` spawns one entity per record at AABB center with record tags (`trigger_volume_bridge.rs:27-78`, 96 lines).

## Reaction machinery

- Registries live in `scripting-core`: `ReactionPrimitiveFn = Box<dyn Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>>` (`crates/scripting-core/src/reaction_registry.rs:21`); `register(name, handler)`; system twin `SystemReactionFn(&Value, &SystemCommandQueue)` at `:79`.
- Descriptor types in `crates/entities/src/data_descriptors/types/reactions.rs`: `NamedReaction{name, descriptor}`, `ReactionDescriptor{Progress|Primitive|Sequence}`, `PrimitiveDescriptor{primitive, tag: Option<String>, on_complete, args}`, `SequenceStep{id: EntityId, primitive, args}` — **`id` is required** on sequence steps.
- **`fire_named_event` does not execute primitives** — it returns the `on_complete` chain only (`reaction_dispatch.rs:111`, "deferred" log `:122-129`). Only `fire_named_event_with_sequences(event, data_registry, sequence_registry, reaction_registry, system_registry, script_ctx)` (`:142`) executes. The movement/ai/weapon drains (`main.rs:2036-2044`) use the non-executing form — not the residual-drain precedent; the death drain (`main.rs:2050`) and crossing drain (`main.rs:2118-2134`) are.
- Tag targeting resolves via `query_by_component_and_tag(ComponentKind::Transform, Some(tag))` (`reaction_dispatch.rs:191-196`).
- System reactions (registered `crates/postretro/src/scripting/systems/system_reactions.rs:33`): `playSound, rumble, flashScreen, vignette, screenShake, showDialog, openMenu, closeDialog, loadLevel, restartLevel, returnToFrontend, setState, cellWrite, appendText, backspaceText, clearText`. `setState` at `:207` pushes `SystemReactionCommand::SetState{slot, value}`. Drain: `App::dispatch_system_commands` (`main.rs:3882`), invoked post-tick (`:2071`), post-crossings (`:2133`), and `:3478`.
- Mover primitive template: `register_mover_reaction_primitives(&mut ReactionPrimitiveRegistry)` at `kinematic_mover.rs:234` (file: **910 lines** — arm/disarm registration goes in a trigger-side module, not here); sequenced twin `register_sequenced_mover_primitives(&mut SequencedPrimitiveRegistry, ScriptCtx)` at `:260`; wired from `crates/postretro/src/scripting/reactions/registry.rs:18`.

## Why bind-at-install (not passing registries into the tick)

`simulate_tick` sees `EntityRegistry`, collision/nav/mover data, `ProgressTracker`, `TriggerTickContext` — **no** `ScriptCtx`, `DataRegistry`, or any reaction registry (`sim/mod.rs:86-103`). Today the tick only emits event *names* in `TickEvents{movement, ai, weapon, death, ...}` (`:75-83`) and the app dispatches after the loop. Threading the registries in would drag `ScriptCtx` into the headless seam; pre-bound closed commands keep it VM-free. `SlotTable` is `postretro_entities` (VM-free), so it may enter the tick context.

## Slots / crossings / install seam

- `SlotValue{Number,Boolean,String,Enum,Array}` and `ReplicationScope{None,SharedGlobal,OwnerPrivatePlayer}` on `SlotSchema.network` (`crates/entities/src/slot_table.rs:10,51-59,72`).
- Validated write path: `apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs:81`) — also used by netcode client apply (`crates/postretro/src/netcode/state_slots.rs:671`, `ClientStateApply::apply_snapshot_state` at `:600`). One write path for scripts, netcode, and (new) bound trigger slot writes.
- `CrossingDetector::{initialize(data_registry, slot_table), detect(slot_table) -> Vec<String>, clear}` (`crates/scripting-core/src/state_crossings.rs:54,79,96`); runs per redraw frame when a session is installed, after slot writes settle (`main.rs:2118-2132`) — per-peer, no role gate.
- Install seam: `App::install_level_payload` at `startup/lifecycle.rs:504`; bridge populate `:670-676`; data-script manifest `:804-816`; `DataRegistry::populate_level(...)` commit `:826-830`; `rebuild_active_reaction_subscribers()` `:831` (re-inits `ProgressTracker` + `CrossingDetector` from the composed registry, `:303-317`). **The binding table builds immediately after `:831`** — the composed reaction set is final there.

## SDK / typedefs / overlay

- `gen_script_types` emits the FFI-primitive surface only; **reaction primitive names are not auto-emitted**. `PrimitiveReactionDescriptor.primitive` is free-form `string` (`sdk/types/postretro.d.ts:736`); literal step types live in the template `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` (e.g. `MoverStartStep` → `postretro.d.ts:802`). Snapshot/committed tests: `crates/postretro/src/scripting/typedef/tests/`.
- Luau evaluation-order rule: entity handle files evaluate before `world.luau`, wrap fn captured as upvalue, bridge nil'd after (`scripting.md` §7; `sdk/lib/entities/movers.luau` is the model).
- Debug UI: `DiagnosticsTab` + `ALL: [Self; 5]` + `label()` + match arm in `crates/renderer/src/render/debug_ui/mod.rs:48-270`; panel entry `draw_diagnostics_panel(..., agent_rows)` `:217`; overlay-label painter precedent `paint_agent_overlay_labels` (`crates/postretro/src/agent_diagnostics.rs:182`, module dev-tools-gated at `main.rs:10-11`); `DebugUi` session-owned, `ensure_debug_ui` gated `main.rs:3070`.

## Oversized-file watch

`parse.rs` 3,626 (Task 1 splits trigger parsing out before Task 2 extends); `kinematic_mover.rs` 910 (not extended — trigger primitives get their own module); `trigger_system.rs` 536, `sim/mod.rs` arg-count pressure noted (context struct extension preferred over new params).
