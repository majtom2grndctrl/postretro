# coop--client-inventory-awaits-tuning — research

Read at b0a5c0f; symbols rechecked at fd8982c (no crate change between). Symbols, not lines. **V** means read in source this session. **I** means inferred, or carried over from earlier research without being re-read.

## Client composition paths today

| Order | What composes | Active slot | Source |
|---|---|---|---|
| Payload installs, then baseline arms | `compose_wieldable_inventory_from_slots` from the payload's slot names | payload `spawn_active_slot`; if that slot is empty, the first populated slot | V: `materialize_net_local_wieldable_inventory_from_tuning`, `Some(tuning)` branch |
| Baseline arms, payload still in flight | `compose_wieldable_inventory` from the client's own pawn descriptor's loadout | first populated slot of the client's own loadout | V: same function, `None` branch |
| Payload lands after that | per-slot reconcile: same canonical name is retuned, a different name is replaced, an absent row is released; then adoption | adoption moves the active slot to `spawn_active_slot` only while `spawn_slot_adoption_open` holds | V: `NetEndpoint::install_tuning_payload` → `apply_installed_movement_tuning_to_armed_pawn` → `materialize_armed_local_pawn`; `adopt_payload_spawn_active_slot` |

- V: `install_tuning_payload` judges the adoption gate before rematerializing, because a release repoints the active slot at the first populated one and would read as a switch.
- V: both client paths pass no carried record. Reserve is seeded from weapon descriptors (`seed_weapon_reserve`), and magazines take descriptor defaults. Only the host restores `CarriedState` (`restore_carried_reserve`, the magazine loop in `compose_wieldable_inventory_slots`).
- V: the arm-time call is `materialize_armed_local_pawn`, from the client snapshot apply in `netcode/src/lib.rs`, run after `prediction.arm` and before reconcile.

## What the client presents between baseline and payload

The local pawn gets a `PlayerMovementComponent` only from the payload's movement block. With no payload, `materialize_armed_local_pawn` logs "local pawn has no host movement tuning; movement prediction stays inert". In production nothing else adds one: the descriptor-based `materialize_net_local_movement_component` has only test-harness callers (V).

`EntityRegistry::local_player_movement_pawn` requires `PlayerMovement` (V). On a client, remote pawns never carry it. So before the payload installs, every reader that goes through it finds no pawn:

| Consumer | Reads pawn via | Source |
|---|---|---|
| Viewmodel draw | `followed_player_pawn` → `local_player_movement_pawn` | V: frame loop viewmodel block; it also needs `tuning.placement_for_slot` |
| Camera follow (eye position) | `followed_player_pawn` and needs `PlayerMovementComponent` | V: `local_pawn_eye_position` |
| Weapon HUD slots | `local_player_movement_pawn` | V: `weapon_hud_values`, `weapon_state_values` in `ui_proxy` |
| Selection occupancy (cursor, dwell, commit) | `local_player_movement_pawn` | V: `local_wieldable_occupancy` |
| Switch declaration | `local_player_movement_pawn` | V: `client_declare_switch` |
| Weapon prediction | `local_active_wieldable` → `local_player_movement_pawn`; also needs host tuning for muzzle terms | V: `predict_client_weapon_command`, `client_fire_muzzle_terms` |
| Active-slot re-sync | `local_player_movement_pawn` | V: `client_observe_active_slot_resync` |
| Movement prediction | needs armed + host environment + `PlayerMovementComponent` | V: `client_predict_tick_with_sender` step 2–3 |

- I: the local world-body weapon prop comes from replicated archetype attachments (`local_weapon_attachments`), not from the composed inventory, so deferring composition does not change it.
- I: the crates hold several dozen other `Inventory` readers, and only the consumers above were traced. The executor confirms that none reaches the local pawn through `local_player_pawn()` before movement materializes.
- V: Input is still sent every tick before the baseline arms (`client_predict_tick_with_sender` step 1). Its `firing_slot` reads the local movement pawn and so defaults to 0 in the window. This brief does not change that.
- V (re-traced at fd8982c): the host fires slot 0 on a fresh press in the window. `ActivationInputCapture::command` emits an initiation for any `Pressed` edge with no pawn check. `predict_client_weapon_command` bails without a local pawn, so nothing clears the initiation or overwrites `firing_slot`. The host takes the slot through `observe_activation` → `deliver_due_start` → `prepare_remote_pawn_command`, and no step compares it to the active slot. `guard_initiation` and `run_remote_weapon_commands` check only that the weapon exists, the ledger, cadence, the fire gate and liveness. A level that was already held does not fire: `run_remote_weapon_commands` passes `controller_starts: false`. No test covers the pre-arm window.
- Not verified: that the host pawn is in `MovementOwners` before the client's baseline arms; that a weapon holstered after a carried selection is `Idle`. I: a held reload in the window ticks slot 0's weapon the same way. Out of scope here; it needs its own ticket, `coop--pre-arm-fire-resolves-slot-zero`.

Consequence: composing from the client's own descriptor only produces state that the late payload then has to correct. The correction is adoption, with the re-sync as its fallback.

## Why adoption could be skipped (today)

V: `spawn_slot_adoption_open` closes on any of these: a pending declaration, a declaration ever made for the pawn, a non-idle selection latch, an active slot off the composed slot, a switch target or origin, or an active weapon that is not idle. The latch input (`is_idle_on`) is read from `local_wieldable_occupancy`, which is empty in the window. When adoption is skipped, the re-sync declares the client's own-descriptor slot to the host after `ACTIVE_SLOT_RESYNC_WINDOW_SECS`. The host converges to a slot the host never composed.

## Carried state

- V: `restore_carried_reserve` and the carried-magazine loop run only when a carried record is passed, and only the host passes one.
- V: the client weapon prediction body (`advance_predicted_weapon_tick`) does not read `magazine` or `AmmoReserve` directly. The host weapon stage's reload gate does (`weapon_stage::machine`).
- I (carried from the #601 research, not fully re-traced): no client reader of the composed magazine or reserve exists. The owner-private projection (`state_slots`, magazine sample) and `client_reload` sound read the replicated values instead.

## Prior commitments

- `networking.md` §What gates: "the sites that resolve tuning keep no fallback to the local registry for a replicated value." The `None` branch is that fallback.
- `plans/done/E16--wieldable-switching-inventory` O41: a payload arriving mid-switch merges into the live inventory and does not rematerialize the pawn. O42: a decode failure clears tuning, and the inventory persists. With this brief a pre-composition decode failure leaves no inventory, which is equivalent in effect because movement is also inert.

## Shapes considered

| Shape | Verdict |
|---|---|
| Compose only from the payload; the armed pawn has no inventory until it installs | **Chosen**. Presentation is unchanged, and adoption is removed |
| Keep composing from own descriptor plus adoption plus re-sync (#601) | Status quo. It corrects a fallback the contract forbids |
| Defer the arm until the payload | Rejected for this brief. It moves reconcile seeding and the camera hand-off |
| Compose a placeholder (empty slots) then fill from payload | Rejected. Same as chosen, plus an extra empty `Inventory` that every consumer has to tolerate |
