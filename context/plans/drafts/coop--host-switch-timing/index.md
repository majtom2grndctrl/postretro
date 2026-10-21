# coop--host-switch-timing

Parked `/draft-session` handoff — not yet a brief. Resume with `/draft-brief` from this handoff. Recheck the verified facts if source has moved past `Read at`.

**Unblocks:** `viewmodel-motion-layer`. Its non-zero `lowerMs`/`raiseMs` default lands only after this item.

## Handoff
```markdown
Problem: For a remote co-op player, the host applies a weapon switch at the client tick the lower *began*, repointing `active_slot` instantly. The client keeps the old weapon active until its lower ends. While that lasts the two disagree about the old weapon, which causes these defects for any weapon with `lowerMs` > 0:
  (1) A drop pressed during the lower drops the new weapon (networking.md §Combat authority lists this as a known gap).
  (2) A reload pressed during the lower starts a reload the client refused, on the holstered old weapon.
  (3) A non-blocking reload the client abandoned at lower begin keeps running on the host.
  (4) Observers see the new weapon before the shooter's own viewmodel swaps.
Every shipped weapon authors 0 today, so content never shows it, but any mod that sets `lowerMs` hits it. It also blocks the non-zero default planned in `viewmodel-motion-layer`.
Outcome: For a remote pawn, the host's view of a switch matches the client's in client-tick terms. The old weapon stays active and "lowering" (no fire, no reload, a non-blocking reload abandoned) from the begin tick to the client's exact repoint tick. The host repoints at that tick, and observers swap then. Drop and reload pressed during the lower act exactly as on the client. `lowerMs` 0 is tick- and delivery-identical to today (the declaration itself grows with the wire bump).
Read at: 3197d201 (research); rechecked against a038e5bc (merged main) for every symbol below. 2026-10-21: main (d6f3471c) adds nothing to the switch, weapon or wire paths; WIRE_VERSION 32 and epoch 14 still hold. Self-reviewed 2026-10-21 against 339b0c48.
Verified facts: (by symbol)
  - Client declaration: `simulate_client_wieldable_tick` (sim/src/sim/mod.rs) reports `accepted` on the lower-begin tick, and the frame loop (frame_loop/mod.rs) calls `client_declare_switch(slot, input_tick)` (defined in main.rs). The active-slot resync (`client_weapon/slot_resync.rs`) returns a slot that goes out through the same call, so host-side a resync is indistinguishable from a switch. The payload is `ClientSwitchDeclaration { declaration_id, slot, client_tick }` (net/src/wire/control.rs), sent as `ClientControlMessage::SwitchDeclaration`. It carries no repoint tick.
  - Declarations go out on lower begin, on a (Raising, BeginLower) restart (full lower, carry 0) and on a third-slot retarget mid-lower (timer unchanged, `switch_origin` stays the original slot). Re-selecting the origin slot sends nothing. With lower_ms 0 the repoint is on the declared tick.
  - Repoint tick: t + N − 1, where N = the ticks until `advance_timer`'s accumulated elapsed time reaches `lower_ms` (`reaches_millisecond_boundary`, sim/src/sim/reload.rs). Deterministic from (lower_ms, tick_dt), and the safe way to reproduce it is to step `advance_timer`, not a closed form. `TICK_DURATION` = 16_667 µs on both sides (frame_timing.rs).
  - Exception: the `complete_reload_before_lower` binding in `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs) applies BeginLower after the machine pass, so the repoint is t + N for `lower_ms` > 0, and t for `lower_ms` 0 (the lower completes the same tick). The host cannot derive this from (client_tick, lower_ms).
  - Host apply: `apply_host_switch_declaration` (netcode/src/host_switch.rs) repoints instantly and calls only `cancel_activation`, which clears executions, not reloads. It refuses on a failed normalize, an empty target, or an active weapon with `block_during_reload` and `is_reload_activity()`. Refusal reaches the client as `ServerSwitchRefused` → `refuse_local_wieldable_switch` → `refuse_local_switch` (rollback, Lowering/Raising → Idle).
  - Switch lane (netcode/src/command_queue/switch_lane.rs): ordered by `client_tick`. `SWITCH_LEAD_TICKS` (malformed lead, 120) and `SWITCH_WAIT_TICKS` (240 release bound) key on `client_tick`. At most one switch applies per resolution.
  - Commands name the client's local `active_slot` (`predict_client_weapon_command` / `predict_with_liveness`), so they name the old slot during the lower.
  - Host binds fire, cadence, holster freeze and reload by `command.firing_slot`: `prepare_remote_pawn_command` (main.rs), `guard_initiation` (host_activations.rs), `ActivationCadence` (activation_cadence.rs), `PressEdges::deliver_presses` (press_edges.rs).
  - Drop is the exception: `TouchSystem::drop_wieldables` (sim/src/sim/touch.rs) releases `inventory.active_slot` and ignores the press slot. That contradicts networking.md's "a reload or drop applies to the slot it was pressed in".
  - Reload during the lower: the client no-ops (Lowering, BeginReload) but sends the raw reload level. The host turns the edge into a press on the old slot, whose weapon is Idle on the host, so a reload starts. Traced, not run.
  - Abandoned reload: on the client, BeginLower from Reloading abandons it (weapon_stage/state.rs). The host only calls `cancel_activation`.
  - Host fire start is gated by `can_start` (`weapon.state.allows_fire()`, cooldown, real command) in `advance_weapon_activation_with_shell_preemption` (sim/src/weapon/execution.rs), before `start_activation` (combat-model/src/activation.rs) runs. A host-tick raise timer would refuse on-time starts under catch-up trims. This is the same reason cadence measures in client ticks (networking.md §Combat authority: "Measuring in host ticks alone would refuse on-time starts after a trim").
  - Readers of the host's `active_slot`: owner-private projection (`WeaponSlotProjection::for_pawn`, state_slots.rs; the client drops other-slot samples via `sample_for_slot`, client_pull.rs); replicated archetype and third-person attachment (`active_weapon_archetype`, replication.rs; `WeaponOwners::take_attachment_changes`; `update_active_weapon_attachment`, remote_materialize.rs).
  - Resync (new since the research): a connected client re-declares its current slot through the same lane when the host's projected slot differs for `ACTIVE_SLOT_RESYNC_WINDOW_SECS`, with nothing pending and no switch in flight. That declaration starts no local lower or raise (networking.md §A diverged active slot re-converges).
  - `WIRE_VERSION` = 32 (net/src/handshake.rs). `TUNING_PAYLOAD_EPOCH` = 14.
  - Adjacent draft: `coop--lossy-link-delivery` moves switch declarations and outcomes from Control onto the Input stream, closing the Control-versus-Input ordering gaps. It touches the same message and lane.
Not verified:
  - Host reload on the holstered weapon (fact above): traced, not run.
  - Whether reconcile ever corrects the host-continued abandoned reload.
  - Seat carry (netcode/src/seat.rs), `normalize_inventory_liveness` and scripting builtins as readers of a parked switch target. Expected to repair through `Inventory::switch_target`, as `release_wieldable` already does.
Decisions (owner-ruled 2026-10-21 unless noted: approach, raise lockout and validation accepted as recommended; review fixes noted):
  - Two-tick declaration: the client sends its lower-begin tick (existing `client_tick`) and its exact repoint tick, computed locally, including the `complete_reload_before_lower` case. (Review fix) It also carries a kind, switch or resync, since the two otherwise share one message. `WIRE_VERSION` 32 → 33.
  - Host-side lowering marker: at the begin tick, in client-tick order, the host marks the old weapon lowering. Fire and reload are refused, and a non-blocking reload is abandoned exactly as the client's state.rs does. The marker ends on the repoint *event* at the repoint tick, never on a host-tick timer. The host then repoints, and observers swap.
  - Raise stays unenforced on the host. The client is stricter, which costs nothing in PvE co-op: PvP is a non-goal (index.md §1.1, §4). A host-tick raise reintroduces trim refusals.
  - Validation: the repoint tick must equal begin + N − 1, or begin + N for the reload-completes-same-tick case (begin for `lowerMs` 0 in both), with N stepped from the host's `lowerMs` for the outgoing active weapon. A resync-kind declaration skips this check. Otherwise it is refused as malformed, in order, and the client rolls back. The malformed-lead check stays on the begin tick, so a long `lowerMs` cannot trip it. This is a desync check, not a security gate: anti-cheat is a non-goal, and the client predicts with the host's tuning, so a mismatch can only be a bug. It fails loudly (a refusal plus a diagnostic naming the expected and declared ticks) rather than silently trusting the client.
  - `block_during_reload` refusal moves to the begin tick, where the client checked. At the repoint tick the host only re-checks that the target is still occupied (drop or hand-over), refusing otherwise.
  - Retarget mid-lower: the new declaration carries the same repoint tick, and the host replaces the pending target. One repoint, with no A→B→C flicker for observers.
  - A restart mid-raise is a fresh begin/repoint pair.
  - A resync re-declaration is sent with the resync kind, has begin == repoint, skips repoint validation and sets no lowering marker.
  - `drop_wieldables` honours the press slot, making networking.md's sentence true. Remove the known gaps this closes from §Switches keep the client's order.
  - (not ruled; open question 1) Sequence with `coop--lossy-link-delivery` (same message, same lane): whichever lands second rebases. Two wire bumps are fine pre-stable.
Non-goals:
  - Host-side raise lockout.
  - Third-person lower/raise animation.
  - The other listed switch gaps (a switch arriving after later-stamped input; a reload squeezed by a stall; the Input-stall malformed refusal), unless one falls out for free.
  - The viewmodel presentation (`viewmodel-motion-layer`).
  - Changing the `lowerMs`/`raiseMs` default (also `viewmodel-motion-layer`).
Proof:
  Automated (host/client harness, fixture weapons with lowerMs/raiseMs > 0):
    - Drop pressed during the lower drops the old weapon on host and client. Drop on the repoint tick drops the new one.
    - Reload pressed during the lower starts nothing on the host. Fire pressed during the lower starts nothing on the host.
    - Switching away from a non-blocking reload abandons it on both sides. A blocking reload refuses at the begin tick.
    - The reload-completes-same-tick case repoints at t + N on both sides.
    - Retarget mid-lower: one host repoint, one observer attachment change, at the shared repoint tick.
    - A restart mid-raise gets a fresh pair.
    - lowerMs 0 is tick- and delivery-identical to today, including the reload-completes-same-tick case (repoint on the begin tick).
    - Under a catch-up trim, a start stamped on the first tick after the client's raise is admitted.
    - A bad repoint tick (off by one either way, or beyond the host's N) is refused in order with a diagnostic naming both ticks, and the client rolls back.
    - A target emptied before the repoint tick is refused, and the client rolls back.
    - Owner-private samples name the old slot throughout the lower. No resync re-declaration fires during an in-flight switch.
    - The 240-tick release bound and the 64-switch lane bound still hold with parked switches.
    - A resync re-declaration applies immediately with no marker and passes for a weapon with lowerMs > 0; a switch-kind declaration with begin == repoint for that weapon is refused.
  Manual:
    - Loopback recipe (networking.md §Manual loopback recipe) at 45/60/5 and 10% loss, with lowerMs/raiseMs authored on a dev weapon: switch, drop mid-lower, reload mid-lower, rapid retargets. The observer sees one swap at roughly the shooter's swap.
Open owner questions:
  1. Ordering against `coop--lossy-link-delivery`: which lands first? A scheduling call; product goals don't decide it.
Resolved 2026-10-21 (formerly Q1–Q3): the two-tick declaration plus host lowering marker (index.md §1.1 "Smooth PvE co-op ... done well" rules out the cheaper options, which leave defects 2 and 3); raise unenforced on the host; validate the repoint tick as a desync check.
Route: brief (compact)
Why this route: It bumps the wire version and changes the networking.md combat-authority contract, which should be reviewed before code. The decisions are few, one executor owns sim, netcode and client, and there are no phases.
Next action: draft (`/draft-brief` from this handoff; settle the ordering question before the build)
```
