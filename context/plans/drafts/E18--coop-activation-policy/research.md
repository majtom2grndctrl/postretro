# Code-grounding notes (2026-07)

Source-verified facts behind the spec's decisions. Line refs are as-of drafting; treat as pointers, not contracts. Design rationale lives in `context/research/co-op-triggers-trap-pools.md` §4.3.

## Activation gate (the E18-B seam)

- `evaluate_trigger_activation(state: &TriggerVolumeComponent, activator: PlayerId) -> TriggerActivationDecision{Fire|Suppress}` — `crates/postretro/src/trigger_system.rs:456`. Currently ignores `activator` except a dev-tools/test log. Condition: `armed && !matches!(fire_mode, Once if latched) && rearm_remaining_ms <= 0.0`.
- Called per-`(trigger, player)` enter edge at `trigger_system.rs:276`, inside the per-player loop (edges built 211–235, dispatched 238–310). This per-edge shape fits per-edge policies directly; threshold policies need a per-trigger transition instead.
- `PlayerId{Local(EntityId), Remote(u64)}` at `:20`; `AuthoritativePlayer{id, pawn}` at `:26`. Per-trigger transient state on `TriggerSystem`: `occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>`, `paired_enters: BTreeSet<(EntityId, PlayerId)>` (`:96–98`). The satisfaction latch (`satisfied: BTreeMap<EntityId, bool>`) is a third map of the same shape.
- `occupancy(trigger) -> usize` at `:109` (`occupants[trigger].len()`). Overlap math `canonical_player_capsules` at `:317` reads only `Transform` + `PlayerMovementComponent` — **no health read today** (the corpse-on-a-plate gap).

## Host / local identity

- `players` slice built in `crates/postretro/src/sim/mod.rs:156–170`: `PlayerId::Remote` from each `RemotePawnCommand::owner_client_id`, `PlayerId::Local` from `registry.local_player_movement_pawn()`. On a listen server (`NetRole::Host`) the local pawn is always the host's own; remote slot pawns are **never** marked local (`scripting/builtins/net_descriptor.rs:24`). A future dedicated server (E15 P4) attaches no local pawn — no `PlayerId::Local` — so `host_only` is inert there.
- Trigger stage runs host/single-player only; the connected-client branch returns early at `main.rs` before `simulate_tick`. Clients observe consequences via replication.

## Alive / health

- `HealthComponent` at `crates/entities/src/components/health.rs`; dead ⇔ `current <= 0.0 || !current.is_finite()` (predicate used by `sweep_deaths`, `scripting/systems/health.rs`). Corpses persist (players never despawned); `death_handled` one-shot latch. Absent health component ⇒ treat as alive (movement-only test pawns). Readable in the tick via `registry.get_component::<HealthComponent>(pawn)`.
- Player death fires `playerDied` (`health.rs` const `PLAYER_DIED_EVENT`) at end of `simulate_tick`; respawn/re-arm is E18-R's, not B's.

## Component / format / compiler / FGD chain (mirror E18-A)

- `TriggerVolumeComponent` + `TriggerActivation{Touch,Use}` / `TriggerFireMode{Once,Multiple}` at `crates/entities/src/components/trigger_volume.rs:6–37`; `new(...)` is 8 args (`:43`), `#[allow(clippy::too_many_arguments)]`. Runtime `new(...)` callers to update: bridge `populate_from_level`, and two `#[cfg(test)]` constructors in `trigger_system.rs` (`spawn_trigger` ~`:683`, sequenced test ~`:1644`).
- Wire: `TriggerVolumeRecord` + `TRIGGER_VOLUMES_VERSION: u16 = 2` (SectionId 44), `crates/level-format/src/trigger_volumes.rs`. E18-A appended `on_fire`/`on_exit` in v2 with a `has_event_names` decode branch; range checks reject `activation > 1`, `command > 3`, `fire_mode > 1`. LE cursor codec, u32-length-prefixed strings, per-version trailing-bytes check.
- Compiler: `resolve_trigger_volume` at `crates/level-compiler/src/trigger_volumes.rs:13` (string→u8 for `activation`/`command`/`fire_mode`, bail on unknown; `enabled_on_spawn` 0/1→bool at 75–85). `MapTriggerVolume` at `crates/level-compiler/src/map_data.rs:177` (field-for-field mirror of the record). `encode_trigger_volumes_section` at `:133`.
- FGD `trigger_volume` at `sdk/TrenchBroom/postretro.fgd:318–351`; `enabled_on_spawn` default is a bare int `1`, other choices defaults are quoted strings.
- Bridge `populate_from_level` at `crates/postretro/src/scripting/systems/trigger_volume_bridge.rs` maps record u8s → enums and calls `TriggerVolumeComponent::new(...)`.

## Dev-tools

- `collect_trigger_diagnostics_rows(registry, bridge, trigger_system, bindings)` at `crates/postretro/src/trigger_diagnostics.rs:21–59`; reads `trigger_system.occupancy(id)` at `:49`.
- `TriggerDiagnosticsRow` at `crates/renderer/src/render/debug_ui/mod.rs:97–110`; `draw_triggers_tab` `num_columns(10)` at `:709`. Overlay label path `collect_trigger_overlay_labels` also reads occupancy.

## Why the fire-model split is the real work

The gate is per-`(trigger, player)`; `any`/`host_only` are per-edge filters that drop straight in. `count`/`all` are per-trigger rising/falling transitions over the effective-occupancy *set* — a different fire shape needing the satisfaction latch and a deterministic activator attribution (lowest occupant `PlayerId`). The unifying `effective occupant = overlap ∧ (alive ∨ occupancy_includes_dead)` predicate lets both paths share one occupancy computation and preserves E18-A's `(trigger, player)`-ordered stream and paired-exit invariants.
