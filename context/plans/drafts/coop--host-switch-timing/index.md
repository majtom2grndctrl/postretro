# coop--host-switch-timing

Brief · compact · reads: `context/lib/networking.md` §Combat authority (Inputs keep the client's order, Switches keep the client's order, A diverged active slot re-converges), §Version gates · read at bcde99af

## Problem
Found while planning `viewmodel-motion-layer`, by reading source: an observed defect class that no shipped content shows, because every shipped weapon authors `lowerMs` 0. For a remote co-op player, the host applies a weapon switch at the client tick the lower began and repoints the active weapon at once, while the client keeps the old weapon active until its lower ends. During that window a drop releases the new weapon instead of the one in hand. A reload pressed during the lower starts on the holstered old weapon. A reload the switch should have abandoned keeps running, then freezes half-done once the client's commands move on. Observers see the new weapon before the shooter does. When this is done, for any `lowerMs`, the host's view of a remote player's switch matches the client's in client-tick terms. The old weapon stays active and lowering until the client's repoint tick, drop and reload act on the weapon in hand, and observers swap once at the repoint. The non-zero `lowerMs` default in `viewmodel-motion-layer` waits on this.

## Decisions
- **Prerequisite: the shared release rule lands first, as its own direct build.** Releasing a wieldable never moves the active weapon except to replace the one released: releasing the active slot mid-switch moves to a live switch target and ends the switch, a dead target ends the switch on the origin, and a released weapon leaves the inventory idle. This brief copies the local rule onto the host, so the local rule must be right first.
- **Placement: host, client-authoritative model kept.** The client still owns its switch; the host follows the client's ticks rather than running its own lower timer, and nothing is predicted or reconciled (`plans/done/E16--wieldable-switching-inventory`). That plan accepted a held-versus-shown window "bounded by the equip duration"; this brief closes it.
- **The host begins the switch the client began.** At the declaration's begin, the host applies to the outgoing weapon the same switch begin the local machine applies. It enters `Lowering`, so fire and reload are refused and a non-blocking reload is abandoned; a reload expiring on that tick completes first. The active slot stays put. Begin-time refusals are unchanged: blocking reload, empty target, malformed lead.
- **The repoint tick is the client's own lower-end tick,** fixed at begin from the outgoing weapon's `lowerMs` (`research.md` Derivation). A connected client never holds reload state (`networking.md` §Combat authority), so a reload the host completes first never moves it.
- **The host lower is per-pawn state, never a host timer, and has named exits only.** It ends at the repoint; when a superseding declaration is refused (on the origin); when the outgoing weapon is released (on the target, by the release rule); when the target empties (on the origin); and at level change, demotion or disconnect. A silent client's lower holds until it resumes. Death does not end it, as death does not end the client's.
- **The repoint applies in client-tick order, outside the lane's arrival order.** It applies in the resolution of its tick once nothing stamped before it is pending. Declarations stamped at or before it apply first in that resolution; later ones wait for it. It is not a retained switch: it counts toward neither the one-switch-per-resolution limit nor the retained bound, and overflow never releases it. As today, the host applies it before touch, so a drop stamped on the repoint tick releases the incoming weapon (`research.md` Boundary-tick order).
- **One outcome per declaration, in declaration order.** A declaration stamped at or before a parked repoint supersedes it: the parked one is refused first, then the new one is decided. An accepted re-target keeps the repoint tick. A refused one ends the lower on the origin, as the client's rollback does. A switch is accepted when it lands on its target, at the repoint or by the release rule. Otherwise it is refused at begin or when the target empties.
- **A re-target that reaches the host after its repoint applies as a new switch** from the new active weapon: the shipped late-Control gap, which `coop--lossy-link-delivery` closes. Both peers still end on the client's choice.
- **The declaration carries a kind: switch or resync.** A resync starts no lower on the client, so the host repoints it at once, with no `Lowering`. The two otherwise share one message (`client_weapon/slot_resync.rs`).
- **The raise stays client-only.** A host raise lockout would run on host ticks and refuse on-time starts after a trim, the reason cadence is judged in client ticks (`networking.md` §Combat authority). Co-op is PvE; a stricter client costs nothing (`context/lib/index.md` §1.1, §4).
- **Drop needs no press-slot plumbing:** with the host following the client's ticks, every drop ordered before the repoint finds the weapon in hand.
- **`lowerMs` 0 keeps today's switch ticks, replies and observer changes;** begin and repoint share a resolution. The one change at 0 is intended: a non-blocking reload switched away from is now abandoned, as in single-player.
- **Non-goals.**
  - Third-person lower and raise animation: observers swap models at the repoint, and a lowering pose would need replicated equip state that `viewmodel-motion-layer` also leaves out.
  - The other shipped switch gaps are ordering gaps this brief does not touch: a reload pressed before a switch but squeezed by a stall, and an Input stall past the lane's lead bound.
  - A host whose active slot has diverged, or whose `lowerMs` changed by hot reload between the client's begin and its own, times that one lower from its own value. The resync converges the slot.
  - The viewmodel presentation and the `lowerMs`/`raiseMs` default belong to `viewmodel-motion-layer`.

## Acceptance
Host/client harness with fixture weapons authoring `lowerMs`/`raiseMs` > 0. Its client settles switch outcomes and installs the host's inventory resends through the production paths. An observer is a second client's replicated view of the shooter. The conditioned-link profile applies unless a row says otherwise.

### Automated
The lower window agrees on both peers:
- [ ] A drop or reload stamped on any tick from the begin through the tick before the repoint acts on the outgoing weapon. A drop stamped on the repoint tick releases the incoming one. (P23)
- [ ] Drop pressed during the lower releases the outgoing weapon on the host. Both peers then hold the switch target and agree on the active slot, with no switch in progress. The declaration settles accepted, and observers see one change. (P23)
- [ ] A weapon dropped mid-lower lies in the world idle. Whoever picks it up, the host player or a remote one, can fire it, and the host admits the start. (P23)
- [ ] Reload pressed during the lower starts nothing on the host and reaches the client as no reload.
- [ ] Switching away from a non-blocking reload abandons it on the host: no completion, no reserve transfer, and switching back finds the weapon idle, not mid-reload.
- [ ] A reload that expires on the begin tick completes before the lower on the host, as in single-player, and the repoint still lands on the client's repoint tick. (P25)
- [ ] Switching away from a blocking reload is refused at begin, and the client rolls back with no second visible transition.
- [ ] A start bound to the outgoing weapon during the lower is refused on the host (regression guard: the client sends none).

The repoint lands on the client's tick:
- [ ] The host repoints on the client's repoint tick for a `lowerMs` shorter than one tick (begin and repoint share a tick), one ending exactly on a tick boundary, and one between boundaries. (P21)
- [ ] Observers see exactly one weapon change, on the repoint tick. Owner-private samples name the outgoing slot for the whole lower.
- [ ] A→B then re-target to C mid-lower: one host repoint, to C, on the original repoint tick, and one observer change. The client settles both declarations holding C. With C refused at its begin, both declarations settle in order and both peers hold A, idle, with no switch in progress. (P22)
- [ ] Three re-targets in one lower: the host refuses each superseded declaration in order and repoints once, to the last, on the original repoint tick. Re-selecting the origin or the target sends nothing. (P29)
- [ ] A re-target made on the repoint tick repoints the host straight to it on that tick; the host never holds the first target. (P20)
- [ ] A switch made during the raise starts a fresh lower on the host.
- [ ] Two switches inside one stall, the second made after the first's repoint: the host repoints to the first target on its tick, then begins the second. Both are accepted, and observers see two changes. (P19)
- [ ] A re-target that reaches the host after its repoint begins a fresh lower from the first target. Both peers end on the re-target, with both declarations accepted. (P31)
- [ ] A target emptied before the repoint, by release or by removal, is refused. Both peers hold the origin, idle, with no switch in progress, and the client can fire it. (P24)
- [ ] After a stall with catch-up trim, the repoint still applies in client-tick order, and a start stamped on the first tick after the client's raise is admitted.
- [ ] A host-side lower that outlives its own timer by many host ticks (a stalled or silent client) stays lowering, refusing fire and reload, until the repoint. (P26)
- [ ] With a lower parked, the lane's retained bound releases the oldest retained declaration, never the repoint. (P26)
- [ ] Death mid-lower keeps the repoint on its tick, matching the client. A level change, demotion or disconnect mid-lower leaves no host lower behind. (P30)

Resync, wire and zero duration:
- [ ] A resync for a weapon with `lowerMs` > 0 repoints the host at once with no lower. No resync fires while a switch is in flight. (P27)
- [ ] Both declaration kinds round-trip through the wire codec, and the wire version advances.
- [ ] With `lowerMs` 0, switch ticks, replies and observer changes match today's harness output exactly.
- [ ] A client that repointed before the host-side drop of its outgoing weapon reaches it keeps holding the incoming weapon.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe) at 45/60/5, then 10% loss, with `lowerMs`/`raiseMs` authored on a dev weapon: switch, drop mid-lower, reload mid-lower, rapid re-targets. The observer sees one swap, at about the shooter's swap.

## Path
- **Seams.** Begin: `apply_host_switch_declaration` / `host_apply_switch_deliveries` (netcode host_switch.rs). Mirror the begin from `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs); a shared begin helper beats a copy. The host records the switch target and origin at begin so the shared release and liveness rules see the switch. The host lower is per-pawn netcode state beside the switch lane (command_queue/switch_lane.rs), checked with the lane's readiness rule (`switch_ready`). Derive the repoint tick by stepping `advance_timer` (sim reload.rs) as the client's machine does, never by closed form, with at least one step: N ≥ 1 for every `lowerMs`.
- **Reply timing.** `send_switch_accepted` moves to the repoint or the release that lands the switch. Superseded refusals reuse `send_switch_refusal`. The client side (`resolve_switch_outcome`) needs no change; verify.
- **Liveness repair on the host.** `normalize_inventory_liveness` runs on every pawn each tick and ends a lower; with the prerequisite it ends on the origin when only the target died. Seat carry (netcode seat.rs) and the owner projection now meet a host switch target; check that each repairs or ignores it.
- **Harness.** The conditioned fixture records outcomes only and sends declarations raw (`host_activations/conditioned_tests.rs`). Extend it to settle through the endpoint's pending chain and to install resends (`conditioned_tests/receive_alternate.rs` opt-in). Observer rows can reuse the two-client `presentation_conditioned_fixture.rs`.
- **Shape and rivals.** Chosen: a derived repoint tick held as per-pawn host state whose exits are named. The repoint-only exit of a host `Lowering` rests on `(Lowering, Expired)` leaving the state `Lowering` (`research.md` P7); pin it with a comment and the stalled-client row. Rivals:
  - A client-declared repoint tick validated against host tuning. Its only use, the reload-expiring case, never reaches a connected client's declaration.
  - Running remote switches through the shared sim timer. Host ticks drift from client ticks under stalls and trims.
  - A second declaration sent at the repoint. Control can lag Input, which reopens the drop gap until `coop--lossy-link-delivery` lands.
  - The repoint as an entry in the switch lane. The lane is arrival-ordered and front-blocked, so it would hold back an earlier-stamped re-target, and its bounds would release the repoint on host time.
- **First slice.** A harness repro of drop-mid-lower on a remote pawn. It falsifies the riskiest premise: that a host `Lowering` with named exits plus the shared release rule lands both peers on the target.
- **Large files.** `crates/postretro/src/main.rs`, `crates/netcode/src/lib.rs` and `crates/netcode/src/command_queue.rs` are past 800 lines. Keep new logic in host_switch.rs and switch_lane.rs, or a new sibling module.
- **At promotion.** `networking.md` §Switches keep the client's order gains the begin/repoint split and loses the drop-while-lowering gap. Record the ownership change from E16: the client runs the lower, and the host mirrors it in client ticks instead of letting equip timers differ. §Version gates records `WIRE_VERSION` 33.

## Open questions
- Order against `coop--lossy-link-delivery`, which moves this message onto Input. — **delegated**: build against whichever has landed and rebase. Two wire bumps are fine pre-stable.

## Boundary inventory

| Name | Rust | Wire | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Declaration kind | new field on `ClientSwitchDeclaration`, a two-variant enum: switch, resync | bitcode-derived enum, unit variants | n/a | n/a | n/a |

## Wire format
Bitcode owns the byte layout (`networking.md` §Wire/codec invariants); no manual layout. `ClientSwitchDeclaration` gains the kind as a native-`Encode`/`Decode` enum with two unit variants, switch and resync, alongside `declaration_id`, `slot` and `client_tick`. No other message changes. A field addition to a shipped message bumps `WIRE_VERSION` 32 → 33; it is not a new variant, so the app-protocol constant stays (`networking.md` §Version gates).
