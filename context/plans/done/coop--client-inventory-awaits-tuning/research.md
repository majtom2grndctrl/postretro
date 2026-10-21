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
- Not verified: that the host pawn is in `MovementOwners` before the client's baseline arms; that a weapon holstered after a carried selection is `Idle`. I: a held reload in the window ticks slot 0's weapon the same way. Out of scope here; on the roadmap as `coop--pre-arm-fire-resolves-slot-zero`.

Consequence: composing from the client's own descriptor only produces state that the late payload then has to correct. The correction is adoption. The reserve seeded from the client's own loadout is never corrected.

## Why adoption never skips on the join path (today)

V (review, fd8982c): `spawn_slot_adoption_open` closes on a pending declaration, a declaration ever made for the pawn, a non-idle selection latch, an active slot off the composed slot, a switch target or origin, or an active weapon that is not idle. In the join window none of these can hold:
- With no movement pawn, `local_wieldable_occupancy` names no active slot, and `WieldableSelection::advance_frame` clears the whole latch every frame. So `is_idle_on` reads idle when the payload lands.
- `client_declare_switch` returns without a movement pawn, so nothing is declared.
- The client wieldable tick gets no pawn, so the own-descriptor weapon stays `Idle`.
- Selection advances before the frame's snapshot apply and Control drain, so a slot key on the install frame is dropped too.

Adoption therefore always applies on a join, and today's late-payload join already ends on the host's slot with no re-sync. The re-sync's remaining trigger is a parity miss, such as P15. Adoption is still judged on every later install for the pawn's life, until a declaration closes it. `release_wieldable` repoints the active slot at the first populated one on any release, and adoption restores the spawn slot after such a repoint. On the host the only mid-life release is dropping the active weapon (`sim/touch.rs`).

## Carried state

- V: `restore_carried_reserve` and the carried-magazine loop run only when a carried record is passed, and only the host passes one.
- V: the client weapon prediction body (`advance_predicted_weapon_tick`) does not read `magazine` or `AmmoReserve` directly. The host weapon stage's reload gate does (`weapon_stage::machine`).
- V (review, fd8982c): no client reader of the composed magazine or reserve exists. The client HUD publishes no ammo. Predicted fire skips the resource check and the debit (`weapon_stage` execution). The client's equip pass never reaches the reload gate in `weapon_stage::machine`, and only that gate begins a reload. Reload sounds read the replicated values, and the projection reader of `AmmoReserve` (`state_slots`) is host-side.
- V (review): the late path leaves the client's own reserve behind. Own-descriptor composition seeds reserve from the client's loadout (`compose_wieldable_inventory` → `seed_weapon_reserve`). The payload merge adds host weapons through `acquire_wieldable_at`, which seeds none, and nothing removes the old reserve.

## Prior commitments

- `networking.md` §What gates: "the sites that resolve tuning keep no fallback to the local registry for a replicated value." The `None` branch is that fallback.
- `plans/done/E16--wieldable-switching-inventory` O41: a payload arriving mid-switch merges into the live inventory and does not rematerialize the pawn. O42: a decode failure clears tuning, and the inventory persists. With this brief a pre-composition decode failure leaves no inventory, which is equivalent in effect because movement is also inert.

## Shapes considered

| Shape | Verdict |
|---|---|
| Compose only from the payload; the armed pawn has no inventory until it installs | **Chosen**. Presentation is unchanged, and adoption is removed |
| Keep composing from own descriptor plus adoption plus re-sync (#601) | Status quo. It corrects a fallback the contract forbids |
| Defer the arm until the payload | Rejected for this brief. It moves reconcile seeding and the camera hand-off |
| Host holds the pawn's first baseline until the client acknowledges the payload | Rejected. It adds a round trip to every join, ties snapshot sending to Control acknowledgements, and fixes a client defect on the host |
| Compose a placeholder (empty slots) then fill from payload | Rejected. Same as chosen, plus an extra empty `Inventory` that every consumer has to tolerate |

## Test harnesses

- V (review): `switch_order.rs`'s `Fixture::new` builds the local pawn directly and never arms it through replication.
- V (review): `host_activations/conditioned_tests/receive_alternate.rs` arms through replication, but it stores the payload in place of `NetEndpoint::install_tuning_payload`, unwraps an inventory on every arm, and gives host and client one descriptor table with no carried state. A late-payload test with differing loadouts needs it extended.
- V (review): the merge tests in `net_descriptor.rs` (for example `tuning_merge_grows_an_existing_inventory_at_the_host_named_slot`, `tuning_merge_same_named_slot_preserves_the_existing_instance`) seed their pre-state through the no-payload branch this brief removes.
- V (review): the client applies Control before draining Snapshot in one frame (`client_drain_control` runs ahead of the snapshot drain in main.rs), so a same-frame arrival is the payload-first path.
- V (review): every applied `local_player` record re-arms (`maybe_arm_local_pawn` → `materialize_armed_local_pawn`). Only the `has_inventory` early-out in `materialize_net_local_wieldable_inventory_from_tuning` stops a second composition.
- V (review): demotion despawns the mapped local pawn, and level unload clears the registry, so a re-armed pawn after either is a fresh entity and takes the compose branch.

## Ordering pins

| id | scenario | ordering | expected outcome | source |
|---|---|---|---|---|
| P1 | Baseline and payload in one frame | Control drains before Snapshot | The payload is stored with no armed pawn. The arm composes once from it, active on the spawn slot | `client_drain_control`; `install_tuning_payload` |
| P2 | Late payload | Arm, then the payload frames later | No inventory while waiting. The install composes from the payload's slots, with reserve and magazines seeded as on the payload-first path | `materialize_net_local_wieldable_inventory_from_tuning`; `compose_wieldable_inventory_from_slots` |
| P3 | Re-arm after composition | Every applied local record re-arms; the player switched first | No recompose, merge only. The active slot stays where the player put it | `maybe_arm_local_pawn`; `has_inventory` early-out |
| P4 | Second payload, same pawn | Composed, then a payload with new weapon tuning and another spawn slot | Instances, magazines and timers kept. Active slot unmoved | `install_tuning_payload`; `same_pawn_tuning` ignores the spawn slot |
| P5 | Repeat payload | The same pawn tuning arrives twice | Generation unchanged, nothing rematerializes | `apply_installed_movement_tuning_to_armed_pawn` |
| P6 | Decode failure before composition | Arm, then a bad payload | No inventory. Movement removed. Error logged | `install_tuning_payload`; `materialize_armed_local_pawn` |
| P7 | Decode failure after composition | Composed, then a bad payload | Inventory kept (E16 O42) | `has_inventory` early-out |
| P8 | Failure, then a good payload | Bad, then good, with no inventory | The good payload composes: its previous is `None`, so the generation bumps | `apply_installed_movement_tuning_to_armed_pawn` |
| P9 | Payload with no weapons | Arm, then an all-empty payload, later a fill | Empty inventory, active 0. The fill acquires the slot and makes it active | `compose_wieldable_inventory_slots`; `acquire_wieldable_at` |
| P10 | Slot key in the window | Press before or on the install frame | Dropped: the latch clears with no movement pawn, and selection runs before snapshot apply | `WieldableSelection::advance_frame`; `local_wieldable_occupancy` |
| P11 | Re-sync timer across the window | Window open longer than `ACTIVE_SLOT_RESYNC_WINDOW_SECS` | The timer resets each tick with no pawn, and does not fire once the host's projection names the composed slot | `client_observe_active_slot_resync`; `slot_resync` |
| P12 | Demotion between arm and payload | Arm, a holding divergence, re-entry | Pawn despawned, tuning dropped. The host resends after the slot closes. The new pawn composes only from the new payload | `install_tuning_payload`'s demotion clear; host admission |
| P13 | Level unload between arm and payload | Arm, relevel, new level | Tuning dropped. No old-level payload can follow: Control is reliable-ordered, and the next payload needs re-entry | endpoint unload; `networking.md` §Channel model |
| P14 | Local pawn re-identity (stale-mapping repair) | Composed, then the client respawns the entity for the same id | The new entity composes from the installed payload, active on the spawn slot. Unchanged by this brief | client stale-mapping repair; `materialize_net_local_wieldable_inventory_from_tuning` |
| P15 | Spawn slot names a weapon the client cannot compose | Late or early payload | Warns and falls back to the first populated slot. The host stays on its slot until the re-sync converges it. This is the parity miss the re-sync stays for | `compose_wieldable_inventory_slots`; `slot_resync` |
