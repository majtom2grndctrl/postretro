# E18 — Trigger Event Fan-out + Plate Semantics

## Goal

Extend the shipped E17-C trigger substrate so triggers fire named reactions (not only mover commands), with consequential effects executing inside the fixed sim tick, plus leave-edge/occupancy plate semantics and runtime arm/disarm. Opens Epic 18: every later spec (co-op policy, spawner, trap pools) binds to the surfaces this ships. Design intent: `context/research/co-op-triggers-trap-pools.md` §4.1–4.4, §4.7.

## Scope

### In scope

- `on_fire` / `on_exit` KVPs on `trigger_volume`: FGD → compiler → PRL section v2 → component.
- Effect-based dispatch split: **consequential** reaction steps (mover commands, `applyDamage`, `armTrigger`/`disarmTrigger`, `setState`, `setAnimationState`) execute in-tick from install-time-bound command lists; **presentation/system** residue fires on the existing app-level drain; **lifecycle** verbs (`loadLevel`, `restartLevel`, `returnToFrontend`) stay queued app-level requests.
- Paired enter/exit gating: an exit event fires iff that player's enter fired, independent of `once` latching and the rearm window.
- Per-trigger occupancy count (consumed by E18-B's policy vocabulary later).
- `armTrigger` / `disarmTrigger` tag-targeted reaction primitives; runtime arming of `enabled_on_spawn = false` triggers.
- Persistent-atmosphere co-op channel proof: host trigger → in-tick `sharedGlobal` slot write → replication → client crossing fires a client-local presentation reaction. Machinery exists; this spec makes `setState` reachable in-tick and proves the path end-to-end.
- Compiler validation: warn on a trigger with neither `target_tag` nor `on_fire` (inert).
- Split-before-extend: relocate trigger parsing out of `parse.rs` (3,626 lines) into the existing dedicated compiler module.
- Dev-tools Triggers diagnostics tab + world overlay labels.
- SDK: `trigger_volume` `world.query` kind, TS/Luau trigger handle with `arm()`/`disarm()` builders, typedef updates, both-runtime parity.

### Out of scope

- Activation-policy vocabulary beyond any-player (E18-B; this spec ships the occupancy input only).
- Spawner entity / `spawnFromSpawner` (E18-C). The consequential classification table is built to admit it later.
- Trap pools, seeded arming (E18-D).
- Activator parameterization of reactions ("damage whoever pressed").
- Replicated transient-event broadcast; reliable co-op delivery of `playSound`/`flashScreen` stings (host-local in v1 — structural, see research doc §4.1).
- Trigger state wire mirror / late-join trigger replication.
- Line-of-sight or aim-ray activation; logic-gate entity graphs; `func_button` sugar class (buttons are the documented composite pattern: `use` trigger + visible geometry).

## Acceptance criteria

- [ ] A map authoring `on_fire` on a `trigger_volume` compiles; a v1 `.prl` (no such field) still loads with fan-out inert and mover commands unchanged.
- [ ] A touch trigger with `on_fire` naming a composed reaction executes its consequential steps within the same sim tick it fires (observable in a headless `simulate_tick` test with no app loop: mover starts, damage applies, `setAnimationState` lands, and slot value changes on the same tick).
- [ ] Presentation/system steps of the same reaction fire through the app drain in the same frame; no consequential step executes twice per fire.
- [ ] `on_exit` fires when a player leaves a trigger they previously entered-and-fired; a suppressed enter produces no exit; a `once` trigger's latch does not block its paired exit; `fire_mode`/`rearm_ms` gate `on_fire` only.
- [ ] Per-trigger occupancy count tracks concurrent overlapping players (0→1→2→1→0 across two-pawn enter/leave in a test), counted independently of the activation gate.
- [ ] A `disarmTrigger` reaction targeting a trigger's tag prevents subsequent activations; `armTrigger` re-arms it, clears a `once` latch, and zeroes any pending rearm so it can fire again.
- [ ] A trigger authored `enabled_on_spawn = false` never fires until an `armTrigger` reaction arms it, then fires normally.
- [ ] An `on_fire` name that matches no composed reaction at level install logs a warning; the trigger's direct `target_tag`/`command` path still works.
- [ ] `prl-build` warns (does not fail) on a `trigger_volume` with neither `target_tag` nor `on_fire`.
- [ ] Two-endpoint test: host trigger fires a reaction whose `setState` step writes a `sharedGlobal` slot in-tick; a connected client's crossing watcher fires a client-local reaction after replication. Late-join: a client joining after the write observes the crossing fire once from its baseline.
- [ ] Behavior parity: the same trigger authoring works from a TS mod and a Luau mod (arm/disarm reactions and handle builders included).
- [ ] `cargo test` typedef drift check passes with regenerated `sdk/types/postretro.d.ts` / `.d.luau`.
- [ ] `world.query({component: "trigger_volume"})` returns a `{id, position, tags}` snapshot per trigger from both TS and Luau; armed/phase state is not exposed.
- [ ] With `--features dev-tools`, the diagnostics window shows a Triggers tab listing each trigger's name, tags, armed/latched state, rearm countdown, occupancy, and bound event names, and the world overlay draws labels + projected AABB edges at trigger volumes; without the feature, no trigger UI or overlay code is compiled in.
- [ ] Compiler trigger parsing lives in the dedicated module with `parse.rs` behavior unchanged (existing trigger compile tests pass unmodified).
- [ ] Determinism: two identical headless runs with scripted inputs produce identical trigger fire sequences and identical post-tick registry/slot state.

## Tasks

### Task 1: Relocate trigger compiler parsing (split-before-extend)

Behavior-preserving move: relocate `resolve_trigger_volume` (currently `crates/level-compiler/src/parse.rs:96`, dispatched at the `classname == "trigger_volume"` branch) and its trigger-specific tests (`trigger_volume_is_extracted_as_aabb_and_excluded_from_static_geometry`, `trigger_volume_rejects_invalid_command_argument_and_rearm`) into `crates/level-compiler/src/trigger_volumes.rs` (currently 25 lines, holds `encode_trigger_volumes_section`). `parse.rs` keeps only the dispatch call. No behavior change; all existing tests pass unmodified. This unblocks Task 2 from extending a 3,626-line file.

### Task 2: Deliver `on_fire`/`on_exit` from FGD to component

End-to-end field delivery. FGD (`sdk/TrenchBroom/postretro.fgd` `trigger_volume` entry): add `on_fire(string)` and `on_exit(string)`, default `""`, descriptions naming the reaction-name semantics. Compiler (`crates/level-compiler/src/trigger_volumes.rs` after Task 1): parse both keys with trim, default empty; add the inert-trigger warning (both `target_tag` and `on_fire` empty → `log::warn!`, not a bail — `target_tag` already defaults silently today). Intermediate `MapTriggerVolume` (`crates/level-compiler/src/map_data.rs`) gains both fields. Wire format (`crates/level-format/src/trigger_volumes.rs`): append `on_fire: String`, `on_exit: String` to `TriggerVolumeRecord` after `enabled_on_spawn`, bump `TRIGGER_VOLUMES_VERSION` 1→2, branch `from_bytes` on version — v1 blobs decode with both fields empty; keep the trailing-bytes check per version. Update the round-trip test and add a v1-decode test. Component (`crates/entities/src/components/trigger_volume.rs`): `TriggerVolumeComponent` gains `pub on_fire: String` and `pub on_exit: String` with `#[serde(default)]` (existing serde round-trip test extends); `new(...)` gains both params. Bridge (`crates/postretro/src/scripting/systems/trigger_volume_bridge.rs` `populate_from_level`): pass the record fields through. Empty string means "no event" throughout — no `Option` on the wire.

### Task 3: Occupancy, paired exit edges, arm/disarm

Runtime semantics in `crates/postretro/src/trigger_system.rs` (536 lines) plus reaction primitives. Replace `TriggerSystem::prior_overlap: HashMap<(EntityId, PlayerId), bool>` with per-trigger occupant tracking that yields (a) the same rising-edge enter detection, (b) a leave edge per `(trigger, player)`, and (c) a per-trigger occupancy count exposed via a `pub(crate)` accessor (E18-B and the Task 7 overlay consume it; count all overlapping players regardless of gate outcome). Use an order-stable representation (see Rough sketch) — deterministic edge-emission order is load-bearing for AC15. Paired gating: record per `(trigger, player)` that an enter *fired* (gate returned Fire); on that player's leave edge, emit the trigger's exit event iff the flag is set, then clear it — `fire_mode` latching and `rearm_remaining_ms` gate enters only, never the paired exit. `run_authoritative_tick` reports fired enter/exit events to its caller (return value or out-param — Task 4 consumes; keep the existing gate as the sole enter-fire path and extend the `#[cfg(test)]` gate-fire recorder to cover exits). Arm/disarm: `pub(crate)` mutation helpers that set `armed = true` + clear `latched` + zero `rearm_remaining_ms` (arm) and set `armed = false` leaving latch/rearm untouched (disarm); register `armTrigger`/`disarmTrigger` in a new `register_trigger_reaction_primitives(registry: &mut ReactionPrimitiveRegistry)` following the mover template (`crates/postretro/src/kinematic_mover.rs` `register_mover_reaction_primitives`), targets resolved by the caller's tag query, non-trigger targets skipped with a once-warn; wire registration alongside the mover call in `crates/postretro/src/scripting/reactions/registry.rs`. Tests mirror the existing `trigger_system.rs` test module style.

### Task 4: Bind-at-install + in-tick consequential dispatch

The dispatch split. **Classification:** a binder-owned const table classifies reaction primitive names: consequential = `moverStart`, `moverStop`, `moverReverse`, `moverGoToPathNode`, `applyDamage`, `armTrigger`, `disarmTrigger`, `setState`, `setAnimationState`; lifecycle = `loadLevel`, `restartLevel`, `returnToFrontend`; everything else presentation. **Bind:** in the free fn `install_world_cpu` (`crates/postretro/src/startup/lifecycle.rs`, reached from `App::install_level_payload`), immediately after its `rebuild_reaction_subscribers(...)` call — the point where `populate_level` has committed and the composed reaction set is final, with the sequence/reaction/system registries in scope (the same set it uses for the `levelLoad` fire) — build a `TriggerBindingTable`: for each spawned trigger with non-empty `on_fire`/`on_exit`, resolve the name against the composed `DataRegistry` reactions; partition each matched reaction's steps into a `Vec<BoundTriggerCommand>` (closed enum — mover command + tag, damage amount + tag, arm/disarm + tag, slot write name + validated `SlotValue`, animation state + tag) and a residual reaction (presentation/system/lifecycle steps + `on_complete` chains) stored in the table; unknown names warn and bind nothing (the KVP `command` path is independent and untouched). `setState` binds as a slot write validated against the slot schema at bind time and executes via the existing validated batch-write path (`apply_store_slot_batch`), script-capability (readonly slots reject at bind with a warning). **Execute:** extend `TriggerTickContext` (`crates/postretro/src/sim/mod.rs`) with the binding table and the `Rc<RefCell<SlotTable>>` (entities-crate type — the sim seam stays VM-free); on an enter/exit fire, `run_authoritative_tick`'s caller inside `simulate_tick` executes the trigger's bound commands against the registry and slot table in the same tick, and pushes the trigger's residual handle onto a new `TickEvents.trigger_residuals` field; the handle is an index/key into the `TriggerBindingTable`'s stored residual entry (not a reaction name), so the drain resolves it without re-matching. **Drain:** in `main.rs`, alongside the existing death-event drain, fire each residual through a new dispatch entry that executes a stored, pre-partitioned step list directly — reusing `fire_named_event_with_sequences`' execution core but bypassing name re-lookup, so the consequential steps already run in-tick never re-run — followed by `dispatch_system_commands`. Deterministic-harness test: a headless `simulate_tick` sequence fires a trigger and asserts mover motion + slot value + arm state changed in-tick with no app loop.

### Task 5: SDK surface + typedefs

Author-facing vocabulary, following the mover precedent end-to-end. Add `trigger_volume` to the `world.query` filter vocabulary (`crates/postretro/src/scripting/entity_world_primitives.rs` — `parse_query_filter`, `WORLD_QUERY_DOC`, a snapshot collector returning `{id, position, tags}`; phase/armed state stays engine-owned and unexposed, matching the mover decision). New `sdk/lib/entities/triggers.ts` and `.luau` with a trigger handle exposing `arm()` / `disarm()` returning `SequenceStep[]` whose `id` is the trigger entity resolved by the `world.query` handle — the target, mirroring mover steps; `args` carries no payload: `{id, primitive: "armTrigger" | "disarmTrigger", args: {}}`. (The name-fired `ReactionPrimitiveFn` form from Task 3 instead resolves targets from the descriptor `tag`, like other tag-targeted primitives.) Delegated from `sdk/lib/world.{ts,luau}` (Luau: evaluate before `world.luau`, capture the wrap fn as an upvalue, nil the bridge after — the movers file shows the pattern). Register the sequenced per-entity variants in the Task 3 module via a `register_sequenced_trigger_primitives(registry: &mut SequencedPrimitiveRegistry, ctx: ScriptCtx)` mirroring `register_sequenced_mover_primitives`. Typedefs: add `ArmTriggerStep`/`DisarmTriggerStep` literal step types to `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` beside `MoverStartStep`, extend the tag-targeted primitive doc-comment list with `armTrigger`/`disarmTrigger` (+ typed args interfaces), regenerate via `gen-script-types`, and update the committed/snapshot typedef tests. Cover TS and Luau parity with a fixture that authors the same arm/disarm reaction in both runtimes.

### Task 6: Co-op channel proof + net QA

Prove the persistent-atmosphere path end-to-end and pin its assumption. Fixture: a level whose trigger fires an `on_fire` reaction containing `setState` on a `sharedGlobal`-declared slot plus a presentation step (e.g. fog density). Two-endpoint test (loopback harness, E17-C net-QA precedent): host pawn enters the trigger; assert the host slot write lands in-tick, the client's slot table converges via the P3.5 apply path (`ClientStateApply::apply_snapshot_state`), the client's `CrossingDetector` fires the bound crossing, and the client-local presentation reaction executes on the client only. Late-join case: a client connecting after the fire observes exactly one crossing fire from its baseline. Add an assertion or documented invariant at the free `rebuild_reaction_subscribers` call in `install_world_cpu` naming the load-bearing ordering: crossing-detector initialization precedes any network baseline apply for the session (research doc §6 "crossing-channel ordering assumption"). Extend the determinism harness to include a trigger-firing tick sequence in its green-and-stays-green gate.

### Task 7: Triggers diagnostics tab + overlay

Dev-tools instrumentation (E10 agent-diagnostics precedent: instrument before feel work). Add `DiagnosticsTab::Triggers` to `crates/renderer/src/render/debug_ui/mod.rs` (extend the `ALL` array and `label()`), with a table of per-trigger rows: name, tags, activation, armed/latched, `rearm_remaining_ms`, occupancy, bound `on_fire`/`on_exit` names and their resolved/unresolved status. Plumbing: the renderer crate cannot see trigger types — `postretro` builds a plain row struct (mirroring how `agent_rows` reach `draw_diagnostics_panel`) from the registry + `TriggerVolumeBridge` AABBs + `TriggerSystem` occupancy + `TriggerBindingTable`, passed into the existing panel call in `main.rs`. World overlay: screen-projected labels at trigger AABB centers plus projected AABB edges via the egui painter, following `paint_agent_overlay_labels` (`crates/postretro/src/agent_diagnostics.rs`), in a dev-tools-gated sibling module. Everything compiles out without `--features dev-tools`.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; blocks Task 2's compiler edits.
**Phase 2 (sequential):** Task 2 — field delivery; Tasks 3/4 consume the component fields.
**Phase 3 (sequential):** Task 3 — occupancy/edges/arm-disarm; Task 4 consumes its fire reporting, Task 5 its primitives.
**Phase 4 (concurrent):** Task 4 (sim/lifecycle/dispatch) and Task 5 (SDK/typedefs) — disjoint files; Task 5's sequenced registration touches Task 3's module, merge-coordinate.
**Phase 5 (concurrent):** Task 6 (net QA) and Task 7 (overlay) — both consume Task 4's binding table read-only.

## Rough sketch

- Fire flow after this spec: `simulate_tick` → trigger stage → gate `Fire` → (existing) `apply_mover_command_to_targets` + (new) execute `TriggerBindingTable` bound commands in-tick → push residual handle to `TickEvents.trigger_residuals` → app drain fires residual + `dispatch_system_commands`.
- `BoundTriggerCommand` lives beside the trigger system in `postretro` (it references `MoverCommand`, `SlotValue`, tags — all entities-crate types; no VM coupling). The owning `TriggerBindingTable` is held on `App` across the install→tick boundary and passed by reference into `TriggerTickContext` each tick.
- Slot writes reuse `apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs` re-export path used by netcode's client apply) — one validated write path for scripts, netcode, and triggers.
- A composed name may match multiple reactions (composition is additive; same-name collisions warn) — the binder binds all matches in composition order, mirroring `fire_named_event` semantics.
- `fire_named_event` (non-`_with_sequences`) never executes primitives — residual drain must use the executing path; the movement/ai/weapon drains are *not* the precedent to copy.
- Occupancy representation must iterate deterministically (AC15): `BTreeMap<EntityId, BTreeSet<PlayerId>>` replacing the boolean pair map — derive `Ord` on `PlayerId` alongside its existing `Hash + Eq + Copy` — or a `HashMap` whose emitted enter/exit edges are sorted by `(EntityId, PlayerId)` before dispatch. Enter edge = insertion, leave edge = removal, count = set len. Multi-player edges resolved on one tick emit in stable `(EntityId, PlayerId)` order.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| fire event name | `TriggerVolumeComponent::on_fire: String` | `"on_fire"` (serde default `""`); PRL v2 appended string | n/a (authored in map) | n/a | `on_fire` |
| exit event name | `TriggerVolumeComponent::on_exit: String` | `"on_exit"` (serde default `""`); PRL v2 appended string | n/a | n/a | `on_exit` |
| arm primitive | handler in `register_trigger_reaction_primitives` | reaction step `"armTrigger"` | `"armTrigger"` / `ArmTriggerStep` | `"armTrigger"` | n/a |
| disarm primitive | ditto | `"disarmTrigger"` | `"disarmTrigger"` / `DisarmTriggerStep` | `"disarmTrigger"` | n/a |
| trigger query kind | `QueryFilter::TriggerVolume` | `"trigger_volume"` | `world.query({component: "trigger_volume"})` | same | n/a (classname is `trigger_volume`) |

Empty string = "no event" on every boundary; no `Option`/null encoding. Arm/disarm sequenced steps target via `SequenceStep.id` (the resolved trigger entity); the primitive-descriptor form targets via `descriptor.tag` — no target payload rides in `args`.

## Wire format

`TriggerVolumesSection` (SectionId 44) v1→v2: append two length-prefixed strings (`u32` LE length + UTF-8 bytes — the section's existing string codec) after `enabled_on_spawn`, in order `on_fire` then `on_exit`. `TRIGGER_VOLUMES_VERSION` bumps to 2; decoder accepts v1 (fields default empty) and v2, rejects others; trailing-bytes check enforced per version. Mirrors the section's own existing hand-rolled LE cursor codec — no new patterns.

## Script syntax examples

```ts
// setupLevel — puzzle disarms a trap, opens the vault, and flips shared atmosphere state
import { world, defineReaction, updateState, getGameState } from "postretro";

const vaultDoor = world.query({ component: "kinematic_mover", tag: "vault-door" });
const hallTraps = world.query({ component: "trigger_volume", tag: "hall-traps" });

export function setupLevel(ctx) {
  return {
    reactions: [
      defineReaction("puzzleSolved", () => [
        ...hallTraps.disarm(),
        ...vaultDoor.start(),
      ]),
      // authored on the plate brush: on_fire = "puzzleSolved", on_exit = "plateReleased"
      defineReaction("plateReleased", () => [...vaultDoor.reverse()]),
      // consequential setState → sharedGlobal slot → each client's crossing fires locally
      { name: "lightsOut", descriptor: { primitive: "setState",
          args: { slot: "encounter.blackout", value: 1 } } },
    ],
    crossings: [
      // `blackoutPresentation` reaction + the `sharedGlobal` `encounter.blackout` store declaration elided for brevity
      { slot: "encounter.blackout", condition: { above: 0.5 }, max: 1,
        fire: ["blackoutPresentation"] },
    ],
  };
}
```

```luau
-- Luau parity: same authoring through require("postretro")
local Postretro = require("postretro")
local traps = Postretro.world.query({ component = "trigger_volume", tag = "hall-traps" })
-- traps:disarm() / traps:arm() return the same sequence steps
```

## Open questions

None blocking. Two decisions pinned here rather than left open: `armTrigger` performs a *full* re-arm (clears `once` latch and pending rearm) — pool-arming and puzzle-reset both want it, and a softer variant can be added later without breaking this one; `on_exit` is ungated by `fire_mode` because a plate that latches its exit strands doors open (research doc §4.1).
