# Player revive and assist — grounding

Read at `b6fc0c4`, after `E18--player-life-states` landed. Cited by symbol. `[M]` marks a claim inferred rather than read.

## Owner decisions (2026-11-02)

- The client learns a teammate's assistability from a per-pawn `assistable` stamp replicated on the movement state, not from `TuningPayload`. The payload carries the pawn's own profiles, `assistable` included: it serializes `LifeStateProfile` verbatim, so `TUNING_PAYLOAD_EPOCH` bumps (review round 1).
- The shared-key press-time resolution (old AC8 / I4 Phase 3) moves to `input-contextual-priority` (its open question 2). This spec exposes `assist_candidate` for it.
- Interim binding: the dev mod's `input.ts` binds `assist` as a `hold` on Use's inputs (`KeyE`, gamepad `west`), the same pattern as its hold-Shift sprint beside tap-Shift dash. No engine default; engine defaults stay all `press` (decided after `/validate-plan`).
- `input-contextual-priority` is reconciled in the same session: its open question 2 is decided, and the duration split is revive's interim.

## Implementability review (2026-11-02)

- Orderings rows moved into the task that owns and tests each; AC10 became a review gate.
- The assist task split into Task 2a (host, input, wire, sources, slots) and Task 2b (client presentation). Sequencing: Task 1 and Task 2a, then Task 2b, then Task 3, whose manual proof needs 2b's bars. `input-contextual-priority` cites Task 2a and Task 2b by name.
- Progress counts held ticks against a required count, `ceil(holdMs × 1000 / TICK_DURATION µs)`, at least one, so host and client agree tick for tick; summed f32 fractions could complete a tick early or late.
- A pawn is never its own assist candidate.
- The received-progress stream holds rather than steps down when a sample lands between its predecessor and the extrapolated value, and catches up at no more than twice the slope.

## Impact policy

- `player.inLifeState:<name>`: `PlayerConditionScope::resolve_input` strips `LIFE_STATE_INPUT_PREFIX` (`ir/life_state_inputs.rs`) and resolves through `resolve_life_state_id`. `DispatchScope::resolve_input`, which `EntityScope` delegates to, refuses the prefix with an error.
- `EntityScope::resolve_owned_input` accepts only `IMPACT_SOURCE_TOKEN`, resolving `PlayerSlot` names (`OwnedEngine`, from the `owner_engine` snapshot) and per-owner store names. It has no life-state branch. `seed_owner_seat_from_registry` captures the seat and `PlayerSlot::ALL` values, not `PlayerMovementComponent.life_state`.
- `life_state_commands::set_life_state(ctx, registry, targets, state)` (`sim/src/life_state_commands.rs`) needs `group_commands_apply_here(ctx)`, `ctx.data_registry` for the descriptor's `life_states`, and a mutable `PlayerMovementComponent`. It calls `enter_life_state`, then `cancel_owned_weapon_activations` when the profile refuses `fire`. `group_commands_apply_here` reads `owner_slot_writes_enabled`, which defaults to true and is false only on client and teleporter or validation paths, so it holds in host impact context and in test `ScriptCtx`s.
- `BoundEffect`, `plan_effect` and `evaluate_dispatch`'s consequential lane are in `impact_policy.rs`; `ImpactEffect` and `apply_effect` are in `impact_effects.rs`.
- `ImpactPolicyRuntime::apply_planned` holds `ctx: ScriptCtx`, `&mut EntityRegistry` and the dispatch. It intercepts `SetOwnerSlot` and `AdjustSentiment` by matching `(CommandRecipients::One(_), effect)` before the generic arm; `apply_effect` marks both `unreachable!`.
- `source.grantHealth` exists. `grant_health` → `set_health_absolute` heals a pawn at HP ≤ 0, re-arms `death_handled` and clears kill credit. No life-state check.
- `validate_life_state_references` (`startup/life_state_validation.rs`), called from `validate_reaction_bodies_pass_a`, walks only `data.reactions`. An unknown impact-policy input or effect fails `bind_policy`; the rebuild loop logs `[Impact] policy … was skipped during bind` as a warning.
- Frame order after the fixed tick: `host_flush_pending_hit_declarations` (late remote hits, which run impact policies through `evaluate_pending_in_tick_impacts_with_overlay_sink`), the remote-hit death sweep, then `player_events.run_tick`.

## Player events and tick

- `PlayerEventTable::run_tick(&ScriptCtx, &mut Vec<PlayerEventResidual>)` → `evaluate` (seeds each player, evaluates the bound Bool program, edge-compares memory) → `apply`. It takes no `TickEvents`. Every event is an edge over a condition; `player_event_from_json` requires `edge` and `condition`. It runs on the authoritative branch, so single player too.
- `BoundPlayerEvent`: `source`, `condition`, `edge`, `program`, `inputs`, `commands`, `residual`, `memory`.
- `TickEvents.trigger_exit_fires` is filled in the Triggers stage and consumed by `scheduler.evaluate`.
- Tick stages: Movers → Movement → Triggers → Projectiles → Ai → Traps → Steering → Weapons, then `run_death_sweep`.
- `players()` order: `extend_with_player_pawns` (`group_resolution.rs`), registry slot order [M on slot order].
- Respawn: `ScriptCtx::pending_respawns`, drained by `sim::respawn::drain` from `begin_tick`. `drain` takes only `ctx` and the registry, so it cannot reach session-owned accumulator state. `enter_life_state` zeroes `life_state_elapsed_ticks`; the movement step and `advance_unstepped_life_state_time` advance it once per host tick for every pawn in a declared life state (a pawn whose descriptor declares none stays at zero), so a respawn or a same-state re-entry shows as elapsed failing to advance. `clear_for_level_unload` (`respawn_placements.rs`) clears the respawn queue at level unload. `touch_system` is session-owned tick state (`session/mod.rs`). `PlayerMovementComponent` holds `life_state: u8`, `life_state_elapsed_ticks`, `life_state_table_revision` and derived stamps (`life_use`, `life_touch`, …); no generation counter.
- Capability: `player_life::permits(registry, pawn, Capability)`.
- Disconnect: `host_handle_transport_disconnect` → `SeatTable::harvest_pawn`, from `net_poll_and_apply` before the tick loop.
- Two-player test: `player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick` (`startup/player_death_surface_tests.rs`).

## Input and wire

- `Command` (`input/commands.rs`) and `Action` (`input/types.rs`). A new command touches `Command::ALL`, `id`, `context`, `kind`, `accepts` (only sprint, movement and crouch restrict to `Press | Hold`; others accept all), `gameplay_target`, the `activator_acceptance_per_command` expectation, `engine_default_inputs`, `derived_relevance`, and the `CommandId` list in `sim/src/scripting/primitives/manifest.rs`. The panel and manifest parsing pick it up from `Command::ALL`.
- `legacy_actions` (`input/defaults_tests.rs`) is test-only, built from the pre-command legacy default tables; a command with no legacy default leaves it alone.
- `default_bindings_for` builds every engine default with `Activator::PRESS`.
- Defaults: Use is `KeyE` / `west`. Dash `KeyF` / `east`, drop `KeyG` / Select. The dev mod's `input.ts` binds sprint `hold` and dash `tap` (threshold 0.2) on `ShiftLeft`.
- `RelevanceFacts` (`dash`, `crouch`, `magazine`, `secondary`) from `registry_facts` and `tuning_facts` (`app/bindings.rs`).
- `WireMovementInput` field order: `wish_dir`, `jump_pressed`, `dash_pressed`, `running`, `crouch_intent`, `facing_yaw`, `use_pressed`, `drop_pressed`, `aim_pitch`, `firing_slot`. `use_pressed` is an edge. `held_gap_sim_command` clears fire, activation, secondary, use and drop and carries other levels. `neutral_sim_command` builds literals. `wire_convert.rs` converts both ways.
- `WIRE_VERSION` = 38 (`net/src/handshake.rs`). `WirePlayerMovementState::all_finite` (`net/src/wire/components.rs`) rejects non-finite floats before apply.
- Tick: `postretro_foundation::TICK_DURATION` is 16 667 µs. `wait` enrollment converts `ceil(durationMs × 1000 / 16_667)` (`startup/reaction_validation.rs`). The timed-slot module carries its own `TICKS_PER_SECOND` = 60.
- `SNAPSHOT_VERSION` = 24 (`net/src/wire.rs`); every `WirePlayerMovementState` field addition has bumped it. `TUNING_PAYLOAD_EPOCH` = 16. Rechecked at `48040a7`, unchanged.
- `SNAPSHOT_TICK_INTERVAL` = 2 (`netcode/src/host.rs`); the timed-slot module's `HOLD_CAP_TICKS` is 16.

## Client view of other pawns

- Interpolated remote Transforms: `ClientReplication::sample_into_registry`.
- `ClientReplication` keeps `map` (`NetworkId -> EntityId`) and `reverse_map` in lockstep; `network_id_for_entity` is `pub`, `entity_for_network_id` is `pub(crate)`. The app reads it through `NetEndpoint::Client { replication, .. }` (`entity_network_ids()` in `main.rs`).
- `WirePlayerMovementState` carries `life_state`, `life_state_elapsed_ticks`, `life_state_table_revision` for every pawn (`collect_payloads`). The client drops it for remote pawns: `IgnoredPayload::MovementWithoutLocalComponent`.
- `tuning_payload_for_pawn` (`netcode/src/lib.rs`) fills `life_state_profiles` from that pawn's descriptor only, cloning `LifeStateProfile` itself; `life_state_names` is shared. `LifeStateProfile` denies unknown fields; `validate_life_states` (`netcode/src/tuning_payload.rs`) checks received profiles.

## Slots and HUD

- `PlayerSlot::ALL` has 15 entries; `warned_owned_engine` is `Cell<u16>` with a `<= u16::BITS` assert.
- `LifeStateMs` touched: `player_slots.rs`, `engine_state_catalog.rs` (`OwnerPrivatePlayer`), `light_membership.rs` `MIRRORED_GAME_STATE_LEAVES`, `ui_proxy.rs`, `netcode/src/state_slots.rs`, generated `sdk/types/postretro.d.*`, typedef `expected.d.*` fixtures.
- Owner-private values reach clients through `owner_private_source_value` on the host and the state-slot wire channel. Every non-weapon `PlayerSlot` resolves through `player_slot_value`, which reads pawn components only; a value outside a component reads absent to every consumer.
- Timed-slot projection: `netcode/src/state_slots/timed_slot_targets.rs` `NAMES: [&str; 4]` (reloadProgress, weaponCooldownMs, heat, cell), fed by `ReplicatedWeaponProjection`; upward ramp per index via `Ramp::accept`. `Ramp` is `pub(super)` in that private module and ramps only once successive interval slopes agree, so it shows the first samples raw. `LifeStateMs` has its own `LifeStateDisplay` / `LifeStateClock`, applied in `App::build_ui_display_slot_snapshot`.
- `run_client_predicted_tick` (`runtime_teleporters/client_tick.rs`) takes a `predict` closure; `frame_loop` passes one that calls `predict_client_weapon_command` then movement prediction. Forward prediction in netcode takes no `EntityRegistry`. `capture_local_reconcile` surfaces `acked_tick`; the state batch applies only on `accepted_newest_snapshot`, and `accept_timed_slot_sample` skips a batch not applied and slots held for a baseline refresh.
- `reset_level_scoped_client_state` (called from `unload_level`) and `demote_client_state` (called from the host-hold path in `main.rs`) both run `prediction.reset_for_level_unload`. App-side state clears beside those calls, as `client_weapon` and `active_slot_resync` do.
- `ui_proxy.rs` writes `player.lifeStateMs` only off a connected client.
- `restamp_life_state` (`foundation/src/movement/life_state.rs`) sets every derived life-state stamp from the current profile, or clears them with none, after each state write. `carry_life_state_stamps` copies them when a table is unavailable, and `gate_merged`'s preserve path (`netcode/src/life_state_tables.rs`) uses it.
- `remote_player_locomotion` on `ClientReplication` is removed in `clear_identity_state` and local-pawn arming, and cleared in `reset_for_level_unload` and `reset_for_demotion`. `cache_remote_player_locomotion` also removes it for a meshless descriptor.
- `ClientApplyFrameOutcome` (`netcode/src/endpoint.rs`) is built by the `lib.rs` receive loop and reports `armed_local_pawn`. That loop applies the state batch and calls `accept_timed_slot_sample` only on `accepted_newest_snapshot`, and reads `acked_tick` from the snapshot's local reconcile.
- HUD: `bleedOutMeterTree` and `reloadMeterTree` in `content/dev/scripts/hud.ts`.
- Stale docs confirmed in `docs/scripting-reference.md` (per-owner + persist rejected; no source-scoped fact vocabulary) and `SourceHandle.grantHealth`'s doc comment.
