# coop--host-switch-timing

Brief · compact · reads: `context/lib/networking.md` §Combat authority (Inputs keep the client's order, Switches keep the client's order, A diverged active slot re-converges), §Version gates · read at c1c18993

## Problem
Found while planning `viewmodel-motion-layer`, by reading source: an observed defect class that no shipped content shows, because every shipped weapon authors `lowerMs` 0. For a remote co-op player, the host applies a weapon switch at the client tick the lower began and repoints the active weapon at once, while the client keeps the old weapon active until its lower ends. During that window a drop releases the new weapon instead of the one in hand. A reload pressed during the lower starts on the holstered old weapon. A reload the switch should have abandoned keeps running, then freezes half-done once the client's commands move on. Observers see the new weapon before the shooter does. When this is done, for any `lowerMs`, the host's view of a remote player's switch matches the client's in client-tick terms. The old weapon stays active and lowering until the client's repoint tick, drop and reload act on the weapon in hand, and observers swap once at the repoint. The non-zero `lowerMs` default in `viewmodel-motion-layer` waits on this.

## Decisions
- **Prerequisite: the shared release rule lands first, as its own direct build.** Releasing a wieldable never moves the active weapon except to replace the one released; releasing the active slot mid-switch moves to a live switch target and ends the switch. Single-player drop mid-lower is broken without it, and this brief copies the local rule onto the host, so the local rule must be right first.
- **Placement: host, client-authoritative model kept.** The client still owns its switch; the host follows the client's ticks rather than running its own lower timer, and nothing is predicted or reconciled (`plans/done/E16--wieldable-switching-inventory`). That plan accepted a held-versus-shown window "bounded by the equip duration"; this brief closes it.
- **The host begins the switch the client began.** At the declaration's client tick, ordered in the switch lane as today, the host applies the same switch begin the local machine does to the outgoing weapon: it enters `Lowering`, so fire and reload are refused, a non-blocking reload is abandoned, and a reload expiring on that tick completes first. The host records the switch target and origin and leaves the active slot alone. Begin-time refusals are unchanged: blocking reload, empty target, malformed lead.
- **The repoint tick is derived, never a host timer.** The host fixes it at begin from the outgoing weapon's `lowerMs`: begin + N − 1, where N is the tick count the client's lower timer takes to reach it, or begin itself for 0. A connected client never holds reload state (`networking.md` §Combat authority), so no declaration has the one-tick-later reload case (`research.md` Derivation). A host-side `Lowering` has no exit of its own; only the repoint ends it.
- **The repoint is an ordered lane event.** It applies, and is bounded, as a switch stamped at the repoint tick would be. At the repoint the host finishes the lower and moves the active slot; observers and owner-private samples change then. A target emptied before the repoint is refused there, and the outgoing weapon returns to `Idle`. The host's apply order on the repoint tick stays before touch, as it is today, so a drop stamped on that tick releases the incoming weapon (`research.md` Boundary-tick order).
- **The raise stays client-only.** A host raise lockout would run on host ticks and refuse on-time starts after a trim, the reason cadence is judged in client ticks (`networking.md` §Combat authority). Co-op is PvE; a stricter client costs nothing (`context/lib/index.md` §1.1, §4).
- **One outcome per declaration, in declaration order.** Accepted at the repoint; refused at begin or at the repoint. A declaration that a re-target supersedes before its repoint is refused when the re-target begins. The host never held its slot, so the client's rollback chain lands on the origin (`resolve_switch_outcome`).
- **A re-target replaces the target and keeps the repoint tick.** It mirrors the client, whose lower timer a third-slot select leaves running. A switch made during the raise is a fresh begin. A re-target that reaches the host after the repoint applies as a new switch: the shipped late-Control gap, which `coop--lossy-link-delivery` closes.
- **The declaration carries a kind: switch or resync.** A resync re-declaration starts no lower on the client, so the host repoints it at once, with no `Lowering` and no derivation. The two otherwise share one message (`client_weapon/slot_resync.rs`).
- **Drop needs no press-slot plumbing.** With the host's active slot following the client's ticks, every drop the lane orders before the repoint finds the client's weapon in hand, and the shared release rule lands both peers on the target. `networking.md`'s "a reload or drop applies to the slot it was pressed in" then holds. The lane's unordered overflow releases remain the shipped exception.
- **`lowerMs` 0 is unchanged in ticks and deliveries.** Begin and repoint fall on one tick, and the reply is sent there.
- **Non-goals.** Host raise lockout. Third-person lower and raise animation. The other shipped switch gaps (a late-Control switch, a reload squeezed by a stall, Input stalls past 2 s). The viewmodel presentation and the `lowerMs`/`raiseMs` default (`viewmodel-motion-layer` owns both).

## Acceptance
Host/client harness with fixture weapons authoring `lowerMs`/`raiseMs` > 0, at the conditioned-link profile unless a row says otherwise.

### Automated
The lower window agrees on both peers:
- [ ] Drop pressed during the lower releases the outgoing weapon on the host. Both peers then hold the switch target, agree on the active slot, and show no switch in progress.
- [ ] Drop stamped on the repoint tick releases the incoming weapon on the host, and both peers then agree.
- [ ] Reload pressed during the lower starts nothing on the host and reaches the client as no reload.
- [ ] Switching away from a non-blocking reload abandons it on the host: no completion, no reserve transfer, and switching back finds the weapon idle, not mid-reload.
- [ ] A reload that expires on the begin tick completes before the lower, on the host as in single-player.
- [ ] Switching away from a blocking reload is refused at begin, and the client rolls back with no second visible transition.
- [ ] A start bound to the outgoing weapon during the lower is refused on the host (regression guard: the client sends none).

The repoint lands on the client's tick:
- [ ] The host repoints on the client's repoint tick for `lowerMs` of one tick, a whole number of ticks, and a non-multiple of the tick.
- [ ] Observers see exactly one weapon change, on the repoint tick. Owner-private samples name the outgoing slot for the whole lower.
- [ ] A→B then re-target to C mid-lower: one host repoint, to C, on the original repoint tick; one observer change. The client settles both declarations holding C. With C refused, the client rolls back to A, never B.
- [ ] A switch made during the raise starts a fresh lower on the host.
- [ ] A target emptied before the repoint is refused at the repoint, and both peers hold the origin.
- [ ] After a stall with catch-up trim, the repoint still applies in client-tick order, and a start stamped on the first tick after the client's raise is admitted.
- [ ] A host-side lower that outlives its own timer by many host ticks (a stalled client) stays lowering, refusing fire and reload, until the repoint.
- [ ] The 240-tick release bound and the 64-switch lane bound hold with a repoint parked.
- [ ] A level change, demotion or disconnect mid-lower leaves no parked repoint and no host-side `Lowering`.

Resync and zero duration:
- [ ] A resync re-declaration for a weapon with `lowerMs` > 0 repoints the host at once with no lower. No resync fires while a switch is in flight.
- [ ] With `lowerMs` 0, switch ticks, replies and observer changes match today's harness output exactly.
- [ ] A client that repointed before the host-side drop of its outgoing weapon reaches it keeps holding the incoming weapon.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe) at 45/60/5, then 10% loss, with `lowerMs`/`raiseMs` authored on a dev weapon: switch, drop mid-lower, reload mid-lower, rapid re-targets. The observer sees one swap, at about the shooter's swap.

## Path
- **Seams.** Begin: `apply_host_switch_declaration` / `host_apply_switch_deliveries` (netcode host_switch.rs). Mirror the begin from `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs), including the `complete_reload_before_lower` ordering; a shared begin helper beats a copy. Repoint: a parked entry in the switch lane (netcode command_queue/switch_lane.rs) keyed by the derived tick. Its wait bound counts from the begin's application plus its own lead, and it is exempt from the malformed-lead check. Derivation: step `advance_timer` (sim reload.rs) the way the client's machine does, never a closed form.
- **Reply timing.** `send_switch_accepted` moves to the repoint. Superseded refusals reuse `send_switch_refusal`. The client side (`resolve_switch_outcome`) needs no change; verify.
- **Readers of a parked target.** `normalize_inventory_liveness`, seat carry (netcode seat.rs) and the owner projection now meet a host switch target. Check that each repairs or ignores it.
- **Shape and rivals.** Chosen: derived repoint plus a host-side `Lowering` whose only exit is the repoint. That exit rests on `(Lowering, Expired)` leaving the state `Lowering` (`research.md` P7), so pin it with a comment and the stalled-client row. Rivals:
  - A client-declared repoint tick validated against host tuning. Its only use, the reload-expiring case, never reaches a connected client's declaration, so the field would carry nothing to check.
  - Running remote switches through the shared sim timer. Host ticks drift from client ticks under stalls and trims, the same reason the raise stays client-only.
  - A second declaration sent at the repoint. Control can lag Input, which reopens the drop gap until `coop--lossy-link-delivery` lands.
- **First slice.** A harness repro of drop-mid-lower on a remote pawn. It falsifies the riskiest premise: that a host-side `Lowering` with no exit plus the shared release rule lands both peers on the target.
- **Large files.** `crates/postretro/src/main.rs` and `crates/netcode/src/lib.rs` are far past 800 lines. Keep new logic in host_switch.rs and switch_lane.rs rather than extending them. `command_queue.rs` (past 800) gains only the lane hook.
- **At promotion.** `networking.md` §Switches keep the client's order gains the begin/repoint split and loses the drop-while-lowering gap. Record the ownership change from E16: the client runs the lower, and the host mirrors it in client ticks instead of letting equip timers differ. §Version gates records `WIRE_VERSION` 33.

## Open questions
- Order against `coop--lossy-link-delivery`, which moves this message onto Input. — **delegated**: build against whichever has landed and rebase. Two wire bumps are fine pre-stable.

## Boundary inventory

| Name | Rust | Wire | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Declaration kind | new field on `ClientSwitchDeclaration`, a two-variant enum: switch, resync | bitcode-derived enum, unit variants | n/a | n/a | n/a |

## Wire format
Bitcode owns the byte layout (`networking.md` §Wire/codec invariants); no manual layout. `ClientSwitchDeclaration` gains the kind as a native-`Encode`/`Decode` enum with two unit variants, switch and resync, alongside `declaration_id`, `slot` and `client_tick`. No other message changes. A field addition to a shipped message bumps `WIRE_VERSION` 32 → 33; it is not a new variant, so the app-protocol constant stays (`networking.md` §Version gates). The round-trip test covers both kinds.
