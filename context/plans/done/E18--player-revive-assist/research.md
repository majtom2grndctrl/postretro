# Player revive and assist — grounding

Read at `b6fc0c4`, after `E18--player-life-states` landed. Cited by symbol. `[M]` marks a claim inferred rather than read.

## Owner decisions (2026-11-02)

- The client learns a teammate's assistability from a per-pawn `assistable` stamp replicated on the movement state, not from `TuningPayload`. The payload carries the pawn's own profiles, `assistable` included: it serializes `LifeStateProfile` verbatim, so `TUNING_PAYLOAD_EPOCH` bumps (review round 1).
- The shared-key press-time resolution (old AC8 / I4 Phase 3) moves to `input-contextual-priority` (its open question 2). This spec exposes `assist_candidate` for it.
- Engine default for `assist`: `hold` on `KeyE` / gamepad `west`, through a per-command default activator (`press` for every other command), gated by relevance so a mod without `assistable` binds nothing and keeps Use on press. Reason: defaults for novices, controls for experts. The dev mod's `input.ts` keeps only a label row for `assist` and inherits the default. `input-contextual-priority` replaces this default with its grouped one.
- `input-contextual-priority` is reconciled in the same session: its open question 2 is decided, and its Default task replaces revive's engine default.

## Owner decisions: pair-valued `assistable` (2026-11-02)

- `radius` and `holdMs` are literals or Number expressions over helper and target, through a reusable two-role pair scope built in this spec.
- No per-entity state (`@state.*`) side reads here; a roadmap entry tracks them behind `scripting.md` §11's cross-entity write path.
- Caps 16 m and 60 s: literals outside are rejected at load, expression results clamp. A radius at or below 0 is a gate (this helper cannot assist this target).
- Mid-hold value change keeps the progress fraction; the new rate applies from the next pass. The effective rate is engine-internal: no script slot exposes it.
- Engine-owned defaults when a field is omitted: 1.5 m and 3000 ms. `assistable: {}` is valid. The dev mod's `down` uses `{}`; the healer recipe lives in the docs only, since the one dev-mod expression that needs no new store would tie revive time to XP.
- The per-pawn stamp is a bool. Numbers reach the helper's client through a host-evaluated pair table and the target's client through `received_required_ticks`; the client never evaluates pair IR.
- Progress is a Q32 fixed-point accumulator: increment `ceil(2³² / n)`. For n ≤ 3600 (60 s), n(n − 1) < 2³², so a constant value completes on exactly the n-th advancing tick. Client tolerance and rebase work in the same units.
- Rejected: a target part times helper factor split, a callback form, client-side evaluation, summed f32 fractions.

## Pair scope grounding (read at `154e36b`)

- `NumberOrIr` (`foundation/src/data_descriptors/types/movement.rs`): untagged serde, `Literal(f32)` or `Ir(IrNode)`; dash precedent. `LifeStateProfile` is in the same crate.
- `eval_value_checked` (`foundation/src/ir/eval.rs`) returns `Err(IrEvalError)` on non-finite arithmetic or division by zero; `eval_value` totalizes to a value.
- Activation caps: `MAX_ACTIVATION_IR_DEPTH` = 32, `MAX_ACTIVATION_IR_NODES` = 256 (`foundation/src/weapon_activation/compiled.rs`).
- `CandidateValidationScope` (`foundation/src/candidate.rs`): value-free declaration-time twin of the runtime candidate scope.
- `is_local_player_slot` (`scripting-core/src/player_event_scope.rs`): `player.*` catalog entries with no replication.
- `PlayerConditionScope::seed_player` reads per-owner store values only with a seat; a seatless pawn reads them as unobserved. The single-player pawn holds `Seat(0)`: `SeatTable::new` reserves it and `startup/lifecycle_world_handles.rs` binds the local pawn to it, so per-owner side reads resolve in single player.
- `MAX_CLIENTS` = 8 (`netcode/src/lib.rs`, private const).
- Owner-token recognition sites: `storeRef` in `sdk/lib/data_script.ts`, the store `byPlayer` in `data_script.luau`, `QUICKJS_INSTALL_BY_PLAYER` / `LUAU_INSTALL_BY_PLAYER` (`scripting-core/src/game_state_refs.rs`), and the mirrors `JS_INSTALL_BY_PLAYER` / `LUAU_INSTALL_BY_PLAYER` in `script-compiler/src/light_membership.rs`, compared by a drift guard in `postretro-sim`. `OwnerToken` is declared in both SDK runtimes. Read at `f9a68dd`: the installers compare `__wire` strings; both `storeRef` sites compare the handle by identity against `IMPACT_SOURCE` and `PLAYER_TARGET`, and `PlayerOwner` is `SourceHandle | PlayerTarget`. No generator writes into `sdk/lib/`.
- Life-state raw adapters (`raw_js`, `raw_lua` in `data_descriptors/life_states.rs`) reject nesting past a fixed shallow depth below `components.lifeStates`, far under the activation IR depth cap; `validate_profile` ends with `serde_json::from_value` of the whole profile (read at `f9a68dd`).
- Missing values: `BindingScope::read` is total; `eval_value_checked` flags only non-finite Number leaves and arithmetic. `PlayerConditionScope` seeds optional values and reports absence through `observes` before evaluation (read at `f9a68dd`).
- SDK lowering: a fluent `NumberRef` keeps its IR in a module `WeakMap`, so a ref inside a descriptor must be replaced before serialization; `loweredValue` (`sdk/lib/util/expression_refs.ts` / `.luau`) does it. `defineEntity` already walks the descriptor through `lowerLoadoutReferences`. `activation.charge` is a frozen `numberRef` constant; `activation.shot` lowers its scale values with `numberNode`. `[M]` the walk shape for `assistable`.
- Load-time hooks for `PairValidationScope` (read at `f9a68dd`): the descriptor validator in `scripting-core/src/data_descriptors/life_states.rs` sees the profile JSON but not store schemas, and `startup/life_state_validation.rs` drops reactions rather than failing load, so neither fits. The commit gates fit. On reload, `commit_staged_manifest_result` (`runtime/core.rs`) holds the built manifest's descriptors and `next_store_declarations` beside `plan_reconcile` and `staged_preflight::validate_life_state_snapshots`; a failure returns `StagedManifestCommitOutcome::Rejected` before commit. On first load, `run_mod_init` (`runtime/mod_init.rs`) holds the manifest's descriptors and store declarations and maps a `plan_reconcile` failure to a mod-init error before `apply_reconcile_plan`. The boot manifest drain in `session/mod.rs` runs after that commit and only logs on failure. The mod-wide life-state names derive from the descriptor list, as `first_life_state_name_overflow` walks them. `[M]` whether that derived order matches the committed `data_registry.life_state_names`; validation needs only membership.
- Pair-table binding moment: `App::rebuild_player_events` (`startup/lifecycle_reaction_rebuild.rs`) runs at level install (`lifecycle_install_world.rs`) and at recompose, over the active composed set.
- `host_replicate` callers (read at `f9a68dd`): production in `main.rs`; fixtures and tests in `host_activations/presentation_conditioned_fixture.rs`, `host_activations/conditioned_tests/receive_alternate.rs`, `ghost_links/replication_test_fixtures.rs`, `presentation_budget_measurement_tests.rs`, `netcode/src/predict_reconcile_relay_test_fixtures.rs` and `netcode/src/lib.rs` tests.
- Snapshot records: `EntitySnapshot.last_processed_client_tick` and the per-recipient `local_player` flag (`net/src/replication.rs`, `net/src/wire.rs`); a record carrying movement metadata without a movement payload fails `MovementMetadataWithoutMovement`. Record metadata has no per-field omit rule (read at `f9a68dd`): `ingest_tick` refreshes authority metadata without minting a baseline, and the encoder resends an acked owner's pawn at its held baseline only when `own_cursor_advanced`; the client re-applies no component on that resend. Wire types are bitcode, and `last_processed_client_tick` sits behind `has_last_processed_client_tick`. `postretro-net` depends on no engine crate, so engine constants cannot reach its record validation.
- `ClientReplication::map` resolves `NetworkId` to `EntityId` (`entity_for_network_id`); `reverse_map` goes the other way.

## Engine default grounding (read at `154e36b`)

- Engine defaults reach bindings at these sites, each hard-coding `Activator::PRESS`: `default_bindings_for` (`input/defaults.rs`) and `engine_slots` (`input/binding_table.rs`), which `EffectiveTable::build` layers from. Both take the per-command default activator.
- `EffectiveTable::build` skips `Relevance::Irrelevant` commands, so relevance gates engine defaults.
- `default_bindings()` is relevance-blind and seeds the boot `InputSystem` (`session/mod.rs`) before `BindingState` installs the effective table.
- `engine_defaults_bind_every_gameplay_command_but_pad_wieldable_slots` (`input/defaults_tests.rs`) requires every gameplay command to have keyboard and gamepad defaults; a defaultless `assist` would fail it. `engine_defaults_keep_every_legacy_key_mouse_and_button_read` counts "no extra key or button reads" and must exclude `assist`.
- `Activator::new(ActivatorKind::Hold)` takes `DEFAULT_ACTIVATOR_THRESHOLD` (0.2 s).

## Implementability review (2026-11-02)

- Orderings rows moved into the task that owns and tests each; AC10 became a review gate.
- The assist task split into Task 2a (host, input, wire, sources, slots) and Task 2b (client presentation). Sequencing: Task 1 and Task 2a, then Task 2b, then Task 3, whose manual proof needs 2b's bars. `input-contextual-priority` cites Task 2a and Task 2b by name.
- Required count n = `ceil(holdMs × 1000 / TICK_DURATION µs)`, at least one, so host and client agree tick for tick; summed f32 fractions could complete a tick early or late. The Q32 accumulator (owner decisions above) keeps that guarantee for pair values that change mid-hold.
- A pawn is never its own assist candidate.
- The received-progress stream holds rather than steps down when a sample lands between its predecessor and the extrapolated value, and catches up at no more than twice the slope.

## Review round 3 (2026-11-02)

- The pair table and `received_required_ticks` ride as owner authority metadata behind one presence flag; a change forces a held-baseline resend to the owner. The record had no per-field omit rule to follow.
- The client's drop row sets the latch: a host sample of 0 cannot tell a reset from a confirmation, and re-predicting on a second target after a confirmation produced a rise-and-drop bar.
- Each client record keeps its own increment, and the tolerance uses it; a lowered required count arriving with a reset sample had widened the tolerance and masked the drop. The rebase keeps the current increment.
- Host-side readers outside the pass (`input-contextual-priority`) take the pass's stored values through `pair_values`, so pair IR has exactly the accumulator and snapshot evaluation sites.
- The bound pair table rebinds wherever player events rebuild; a level unload clears only progress state.

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
- Two-player test: `player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick` (`startup/player_death_surface_tests.rs`). Its `Consumer::tick_then` calls `simulate_tick_with_presentation_aim` with a fresh `TouchSystem` and no remote pawn commands each tick, then `PlayerEventTable::run_tick`.

## Input and wire

- `Command` (`input/commands.rs`) and `Action` (`input/types.rs`). A new command touches `Command::ALL`, `id`, `context`, `kind`, `accepts` (only sprint, movement and crouch restrict to `Press | Hold`; others accept all), `gameplay_target`, the `activator_acceptance_per_command` expectation, `engine_default_inputs`, `derived_relevance`, and the `CommandId` list in `sim/src/scripting/primitives/manifest.rs`. The panel and manifest parsing pick it up from `Command::ALL`.
- `legacy_actions` (`input/defaults_tests.rs`) is test-only, built from the pre-command legacy default tables; a command with no legacy default leaves it alone.
- Defaults: Use is `KeyE` / `west`. Dash `KeyF` / `east`, drop `KeyG` / Select. The dev mod's `input.ts` binds sprint `hold` and dash `tap` (threshold 0.2) on `ShiftLeft`.
- `RelevanceFacts` (`dash`, `crouch`, `magazine`, `secondary`) from `registry_facts` and `tuning_facts` (`app/bindings.rs`).
- `WireMovementInput` field order: `wish_dir`, `jump_pressed`, `dash_pressed`, `running`, `crouch_intent`, `facing_yaw`, `use_pressed`, `drop_pressed`, `aim_pitch`, `firing_slot`. `use_pressed` is an edge. `held_gap_sim_command` clears fire, activation, secondary, use and drop and carries other levels. `neutral_sim_command` builds literals. `wire_convert.rs` converts both ways.
- `WIRE_VERSION` = 38 (`net/src/handshake.rs`). `WirePlayerMovementState::all_finite` (`net/src/wire/components.rs`) rejects non-finite floats before apply.
- Tick: `postretro_foundation::TICK_DURATION` is 16 667 µs. `wait` enrollment converts `ceil(durationMs × 1000 / 16_667)` in f64 (`sim/src/scripting_systems/reaction_scheduler/enroll.rs`); `wait_duration_is_valid` (`startup/reaction_validation.rs`) mirrors it. The timed-slot module carries its own `TICKS_PER_SECOND` = 60.
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
