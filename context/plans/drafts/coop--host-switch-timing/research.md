# coop--host-switch-timing — research

Read at c1c18993. Findings that inform the brief but do not decide it. Symbols, not line numbers.

## Pins

| # | Claim | Source |
|---|---|---|
| P1 | The declaration carries `declaration_id`, `slot`, `client_tick`; no repoint tick, no kind. | `ClientSwitchDeclaration` (net wire/control.rs) |
| P2 | Host apply repoints `active_slot` at once, clears `switch_target`/`switch_origin`, calls only `cancel_activation` on the outgoing weapon, and replies at once. Refuses on failed normalize, empty target, or `block_during_reload && is_reload_activity()`. | `apply_host_switch_declaration`, `host_apply_switch_deliveries` (netcode host_switch.rs) |
| P3 | Host switch apply runs after command resolution and before the simulation, so on a switch's own tick it precedes touch (drop) and the weapon stage. | `host_apply_switch_deliveries` doc |
| P4 | The client settles exactly one outcome per declaration, front-first; an outcome for any other id is ignored. A refused front with a newer pending declaration passes its rollback slot to that declaration and changes nothing visible. | `resolve_switch_outcome` (netcode lib.rs) |
| P5 | Local switch: `begin_lower` needs an occupied non-active slot that is not already the target and no blocking reload. A third-slot select during a lower re-targets: `(Lowering, BeginLower)` is a no-op on the timer. The repoint happens when `machine.lowered`; the raise begins the same tick. `lower_ms` 0 repoints on the select tick. | `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs), `transition_wieldable_state` (weapon_stage/state.rs) |
| P6 | `complete_reload_before_lower` finishes a reload that expires on the select tick before lowering, so the repoint is one tick later. A connected client never holds reload state (reload is projected, `networking.md` §Combat authority), so the case never reaches a declaration. | local.rs; networking.md "Client-side ammo, heat, cell, and reload prediction/reconciliation remain out of scope" |
| P7 | `(Lowering, Expired)` reports `Lowered` but leaves the state `Lowering`; only local.rs `finish_lowering` leaves it. The remote stage (`run_remote_weapon_commands`, commands/remote.rs) never reads `machine.lowered`, so a host-side `Lowering` holds until something ends it. | state.rs, remote.rs |
| P8 | Reload during the lower: the client sends the raw reload level (`build_sim_command`), `firing_slot` names its active slot A (`predict_with_liveness`), and host intake records `Press { slot: A }` (`observe_reload_level`). `deliver_presses` binds the press to A; `tick_weapon_machine_phases` starts a reload when `allows_reload()`, which `Idle` allows. Nothing checks `active_slot`. | command_queue.rs, press_edges.rs, machine.rs, `prepare_remote_pawn_command` (main.rs) |
| P9 | A reload running when the switch applies: `cancel_activation` returns unless an activation cursor exists, so the reload continues while commands name A, can complete with reserve transfer, then freezes mid-reload once commands name B (`tick_weapon_resources` advances only heat and cell on holstered instances). Single-player abandons it: `(Reloading | ShellLoading, BeginLower)`. | weapon.rs `cancel_activation`, resource.rs, state.rs |
| P10 | Host fire gate is `can_start` in `advance_weapon_activation_with_shell_preemption` (`allows_fire()`, cooldown, real command); `guard_initiation` does not check the active slot. A holstered `Idle` weapon would admit a start; only the client's own refusal to send one protects it today. | sim weapon/execution.rs, host_activations.rs |
| P11 | Drop: `drop_wieldables` releases `inventory.active_slot`; `drop_pressed` is a per-player bool with no slot. A connected client predicts no drop; it learns of one through the tuning resend, whose materialize calls `release_wieldable`. | sim touch.rs, frame_loop/mod.rs, `materialize_net_local_wieldable_inventory_from_tuning` |
| P12 | `release_wieldable` sets `active_slot` to the lowest occupied slot whatever slot it released, clears `switch_target`/`switch_origin` only when they name the released slot, and starts no raise. | sim scripting/builtins/wieldable_inventory.rs |
| P13 | Single-player drop mid-lower: A released, active becomes the lowest occupied slot F in `Idle`, `switch_target` stays B. `normalize_inventory_liveness` repairs nothing (both live); the switch never completes; selecting B is refused (`switch_target == Some(B)`), and the UI reads a switch in progress. No test covers it. | traced: touch.rs, local.rs, wieldable_inventory.rs |
| P14 | Client whose dropped weapon was still lowering when the tuning resend lands: target cleared, active moves to F, A stays `Lowering` at 0 ms indefinitely; the resync treats it as a switch in flight and never steps in. Inferred from P12 and P5. | traced |
| P15 | Owner projection and observer archetype read the host's `active_slot`: `WeaponSlotProjection::for_pawn` (netcode state_slots.rs), `active_weapon_archetype` (netcode replication.rs), `WeaponOwners::take_attachment_changes`. | source |
| P16 | Resync sends through `client_declare_switch` with the client's current slot; host-side it is indistinguishable from a switch. | client_weapon/slot_resync.rs, main.rs |
| P17 | Switch lane: released in client-tick order once nothing older is pending; at most one per resolution; malformed beyond `SWITCH_LEAD_TICKS`; released unordered after `SWITCH_WAIT_TICKS` from intake or at `MAX_RETAINED_SWITCHES`. | netcode command_queue/switch_lane.rs |
| P18 | `E16--wieldable-switching-inventory` accepted that remote players "may briefly see the held and firing weapons disagree during an equip window, bounded by the equip duration", and rejected predict-and-reconcile for switches. This brief removes that window and keeps the no-reconcile model. | plans/done |

## Derivation: repoint tick

With P6, every connected-client declaration with `lower_ms` > 0 repoints at begin + N − 1, where N is the tick count `advance_timer` takes to reach `lower_ms` at `TICK_DURATION`; with 0 it repoints at begin. The host holds the same `lower_ms` (the client composes from host tuning) and the same tick, so it can derive the tick. A hot reload changing `lowerMs` mid-lower is the only way the two could differ; both peers fixed their lower at begin, so deriving R at begin agrees with the client.

## Boundary-tick order

On a switch's own tick the host applies the switch before touch (P3), so a drop stamped on that tick releases the new active slot. Single-player runs touch before the weapon stage, so its drop on the repoint tick releases the outgoing weapon. Today the same split exists for `lower_ms` 0. Keeping the host's order preserves `lower_ms` 0 behavior exactly; the one-tick boundary is a documented edge, not a divergence this brief adds.

## Late Control

A re-target declaration rides Control; the parked repoint is lane-ordered against retained Input. If the re-target arrives after the host released the repoint, the host reads it as a new switch from the new active weapon. Both peers still end on the client's choice. This is the shipped gap "a switch arriving after input stamped later than it cannot reorder that input", which `coop--lossy-link-delivery` closes by moving switches onto Input.
