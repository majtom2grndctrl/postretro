# coop--host-switch-timing

Brief · compact · reads: `context/lib/networking.md` §Combat authority (Pawn-state gates judge the stamped tick, Inputs keep the client's order, Switches keep the client's order, A diverged active slot re-converges), §Version gates · read at 08eb5c98

## Problem
Found while planning `viewmodel-motion-layer`, by reading source: an observed defect class that no shipped content shows, because every shipped weapon authors `lowerMs` 0. For a remote co-op player, the host applies a weapon switch at the client tick the lower began and repoints the active weapon at once, while the client keeps the old weapon active until its lower ends. During that window a drop releases the new weapon instead of the one in hand. A reload pressed during the lower starts on the holstered old weapon. A reload the switch should have abandoned keeps running, then freezes half-done once the client's commands move on. Observers see the new weapon before the shooter does. When this is done, for any `lowerMs`, the host's view of a remote player's switch matches the client's in client-tick terms. The old weapon stays active and lowering until the client's repoint tick, drop and reload pressed during the lower act on the weapon in hand, and observers swap once at the repoint. The non-zero `lowerMs` default in `viewmodel-motion-layer` waits on this.

## Decisions
- **Prerequisite: the shared release rule lands first, as its own direct build.** Releasing a wieldable never moves the active weapon except to replace the one released: releasing the active slot mid-switch moves to a live switch target and ends the switch, a dead target ends the switch on the origin, and a released weapon leaves the inventory idle. This brief copies the local rule onto the host, so the local rule must be right first.
- **Placement: host, client-authoritative model kept.** The client still owns its switch. The host follows the client's ticks, and nothing is predicted or reconciled (`plans/done/E16--wieldable-switching-inventory`). That plan accepted a held-versus-shown window "bounded by the equip duration"; this brief closes it.
- **The host begins the switch the client began.** At the declaration's begin, the host applies to the outgoing weapon the same switch begin the local machine applies: fire and reload are refused, and a non-blocking reload is abandoned. A reload expiring on the begin tick completes first, except when begin and repoint share a resolution, where it is abandoned like any other. The active slot stays put. Begin-time refusals are today's.
- **The repoint tick is the client's own lower-end tick,** fixed at begin from the outgoing weapon's `lowerMs` (`research.md` Derivation). A connected client never holds reload state (`networking.md` §Combat authority), so a reload the host completes first never moves it.
- **The repoint applies in client-tick order, where switch deliveries apply today.** It applies in the resolution of its tick, before touch, once nothing stamped before it is pending. Declarations stamped on or before its tick apply first in that resolution, exempt from the one-switch-per-resolution limit; later ones wait for it. A later declaration the lane releases early on host time, by its wait bound, its retained bound or a malformed lead, applies the parked repoint first. A drop stamped on the repoint tick therefore releases the incoming weapon (`research.md` Boundary-tick order).
- **How a host lower ends, and what each declaration gets.** Replies leave in declaration order, one per declaration.
  - At the repoint: on the target, accepted.
  - The outgoing weapon is released: the release rule lands it on a live target, accepted; with the target gone too, as an emptied target.
  - A declaration stamped on or before the repoint tick supersedes it, and the parked one is refused. An accepted re-target keeps the repoint tick; a refused one ends the lower on the origin, as the client's rollback does.
  - The target is emptied or re-tenanted before the repoint: on the origin, refused.
  - Level change, demotion or disconnect: discarded with the client's pending chain, which clears at the same points; no reply.
  - Nothing else ends it: no host timer, and not death, which leaves the client's lower running too. A silent client's lower holds until it resumes.
- **A re-target that reaches the host after its lower ended applies as a new switch** from the new active weapon, timed from its own stamp. This covers late Control (`coop--lossy-link-delivery` closes it), a re-target after a refused begin, and one after a drop the client has not yet learned of. Both peers end on the client's choice.
- **The declaration carries a kind: switch or resync.** A resync starts no lower on the client, so the host repoints it at once. The two otherwise share one message (`client_weapon/slot_resync.rs`).
- **The host runs no raise timer; it judges the raise in client ticks.** Reload presses and starts stamped before the client's raise-end tick, derived like the repoint, are refused, as pawn-state gates judge the stamped tick (`networking.md` §Combat authority). A host-tick lockout would refuse on-time starts after a trim.
- **`lowerMs` 0 keeps today's switch ticks, replies and observer changes;** begin and repoint share a resolution. The changes at 0 are intended and match single-player: a non-blocking reload switched away from is abandoned, and a reload during a non-zero raise is refused.
- **Non-goals.**
  - Press-slot plumbing for drop: with the host following the client's ticks, every drop ordered before the repoint finds the weapon in hand.
  - Third-person lower and raise animation: observers swap models at the repoint, and a lowering pose would need replicated equip state that `viewmodel-motion-layer` also leaves out.
  - The Control-versus-Input ordering gaps, a reload squeezed by a stall and an Input stall past the lane's lead bound: `coop--lossy-link-delivery` closes them by moving switches onto Input.
  - Lowers timed from the host's value: a diverged active slot, a `lowerMs` hot reload between the two begins, and the late re-targets above each time one lower from the host's side. They converge; the resync covers a slot divergence.
  - The viewmodel presentation and the `lowerMs`/`raiseMs` default belong to `viewmodel-motion-layer`.

## Acceptance
Host/client harness with fixture weapons authoring `lowerMs`/`raiseMs` > 0. Its client settles switch outcomes and installs the host's inventory resends through the production paths. An observer is a second client's replicated view of the shooter. The conditioned-link profile applies unless a row says otherwise.

### Automated
The lower window agrees on both peers:
- [ ] A drop stamped on any tick from the begin through the tick before the repoint releases the outgoing weapon, and a reload stamped on those ticks starts nothing. A drop stamped on the repoint tick releases the incoming one. (P23)
- [ ] Drop pressed during the lower: both peers then hold the switch target and agree on the active slot, with no switch in progress. The declaration settles accepted, and observers see one change. (P23)
- [ ] The outgoing weapon, dropped mid-lower, lies in the world idle, not lowering. Whoever picks it up, the host player or a remote one, can fire it, and the host admits the start. (P23)
- [ ] A re-target and a drop stamped on the same tick mid-lower: the first declaration is refused, the drop lands the re-target, and both peers hold it with no switch in progress. Observers see one change, never the first target. (P33)
- [ ] Switching away from a non-blocking reload abandons it on the host: no completion, no reserve transfer after the begin, and switching back finds the weapon idle, not mid-reload. With `lowerMs` 0 too.
- [ ] A reload that expires on the begin tick of a lower longer than one tick completes before the lower on the host, as in single-player, and the repoint still lands on the client's repoint tick. (P25)
- [ ] Switching away from a blocking reload is refused at begin. With a lower longer than the round trip, the client rolls back before its repoint and never shows the target.
- [ ] A reload stamped on the repoint tick or during the client's raise starts nothing on the host; the first reload stamped after the raise starts. (P38)
- [ ] A start bound to the outgoing weapon during the lower is refused on the host (regression guard: the client sends none).

The repoint lands on the client's tick:
- [ ] The host repoints on the client's repoint tick for a `lowerMs` shorter than one tick (begin and repoint share a tick), one ending exactly on a tick boundary, and one between boundaries. (P21)
- [ ] Observers see exactly one weapon change, on the repoint tick. Owner-private samples name the outgoing slot for the whole lower.
- [ ] A→B then re-target to C mid-lower: one host repoint, to C, on the original repoint tick, and one observer change. The client settles both declarations holding C. With C refused at its begin, both declarations settle in order and both peers hold A, idle, with no switch in progress. (P22)
- [ ] Three re-targets in one lower: the host refuses each superseded declaration in order and repoints once, to the last, on the original repoint tick. Re-selecting the origin or the target sends nothing. (P29)
- [ ] A re-target made on the repoint tick repoints the host straight to it on that tick; the host never holds the first target. (P20)
- [ ] A switch made during the raise starts a fresh lower on the host.
- [ ] Two switches inside one stall with no catch-up trim, the second made after the first's repoint: the host repoints to the first target on its tick, then begins the second. Both are accepted, and observers see two changes. Under a catch-up trim the host keeps that order, and observers may see only the second. (P19)
- [ ] A re-target that reaches the host after its repoint begins a fresh lower from the first target, repointing on the re-target's stamp plus the lower, at once if that tick has resolved. Both peers end on the re-target, with both declarations accepted. (P31)
- [ ] A target emptied before the repoint, by release or by removal, or released and refilled in one host tick, is refused. Both peers hold the origin, idle, with no switch in progress, and the client can fire it. (P24, P34)
- [ ] After a stall with catch-up trim, the repoint still applies in client-tick order, and a start stamped on the first tick after the client's raise is admitted.
- [ ] A host-side lower that outlives its own timer by many host ticks (a stalled or silent client) stays lowering, refusing fire and reload, until the repoint. (P26)
- [ ] With a lower parked and Input stalled while Control flows, a later declaration the lane releases early, by its wait bound, its retained bound or a malformed lead, never gets its reply before the parked one's. The client settles every declaration in order, and once Input resumes both peers agree with nothing pending. (P26, P32)
- [ ] Death mid-lower keeps the repoint on its tick, matching the client. A level change, demotion or disconnect mid-lower leaves no host lower behind and no reply owed. (P30)

Resync, wire and zero duration:
- [ ] A resync for a weapon with `lowerMs` > 0 repoints the host at once with no lower. No resync fires while a switch is in flight. (P27)
- [ ] Both declaration kinds round-trip through the wire codec, and the wire version advances.
- [ ] With `lowerMs` 0, switch ticks, replies and observer changes match a baseline recorded with the extended harness before the host change lands.
- [ ] A client that repointed before the host-side drop of its outgoing weapon reaches it keeps holding the incoming weapon.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe) at 45/60/5, then 10% loss, with `lowerMs`/`raiseMs` authored on a dev weapon: switch, drop mid-lower, reload mid-lower and mid-raise, rapid re-targets. The observer sees one swap, at about the shooter's swap.

## Path
- **Seams.** Begin: `apply_host_switch_declaration` / `host_apply_switch_deliveries` (netcode host_switch.rs). Mirror the begin from `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs) through a shared helper, not a copy. For a lower longer than one tick, applying the begin after the outgoing weapon's machine pass on the begin tick keeps a reload expiring there, as `complete_reload_before_lower` does locally. The host records the switch target and origin at begin so the shared release and liveness rules see the switch. Hold the lower as per-pawn netcode state beside the switch lane (command_queue/switch_lane.rs); `switch_ready` already keeps later-stamped entries behind a pending older tick, so the parked repoint can feed it. Derive repoint and raise-end ticks by stepping `advance_timer` (sim reload.rs) as the client's machine does, never by closed form, with N ≥ 1 for every non-zero duration.
- **Replies.** `send_switch_accepted` moves to wherever the lower lands on its target. Superseded refusals reuse `send_switch_refusal`. The client side (`resolve_switch_outcome`) needs no change; verify.
- **Liveness repair on the host.** `normalize_inventory_liveness` runs on every pawn each tick and ends a lower; with the prerequisite it ends on the origin when only the target died. Seat carry (netcode seat.rs) and the owner projection now meet a host switch target; check that each repairs or ignores it. Detect a re-tenanted target by weapon instance, not occupancy.
- **Harness.** The conditioned fixture records outcomes only and sends declarations raw (`host_activations/conditioned_tests.rs`). Extend it to settle through the endpoint's pending chain, install resends (`conditioned_tests/receive_alternate.rs` opt-in) and carry a second, observing client. `presentation_conditioned_fixture.rs` has two clients but resolves no commands or switches. Record the `lowerMs` 0 baseline before the host change.
- **Shape.** Chosen: derived ticks held as per-pawn host state with named endings. Strongest rival: running remote switches through the shared sim timer, rejected because host ticks drift from client ticks under stalls and trims. Others are in `research.md` Rivals. The repoint-only exit of a host `Lowering` rests on `(Lowering, Expired)` leaving the state `Lowering` (P7); pin it with a comment and the stalled-client row.
- **First slice.** A harness repro of drop-mid-lower on a remote pawn. It falsifies the riskiest premise: that a host lower with named endings plus the shared release rule lands both peers on the target.
- **Large files.** `crates/postretro/src/main.rs`, `crates/netcode/src/lib.rs` and `crates/netcode/src/command_queue.rs` are past 800 lines. Keep new logic in host_switch.rs and switch_lane.rs, or a new sibling module.
- **At promotion.** `networking.md` §Switches keep the client's order gains the begin/repoint split and the raise gate, and loses the drop-while-lowering gap. Record the ownership change from E16: the client runs the lower, and the host mirrors it in client ticks instead of letting equip timers differ. §Version gates records `WIRE_VERSION` 33.

## Open questions
- Order against `coop--lossy-link-delivery`, which moves this message onto Input. — **delegated**: build against whichever has landed and rebase. Two wire bumps are fine pre-stable.

## Boundary inventory

| Name | Rust | Wire | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Declaration kind | new field on `ClientSwitchDeclaration`, a two-variant enum: switch, resync | bitcode-derived enum, unit variants | n/a | n/a | n/a |

## Wire format
Bitcode owns the byte layout (`networking.md` §Wire/codec invariants); no manual layout. `ClientSwitchDeclaration` gains the kind as a native-`Encode`/`Decode` enum with two unit variants, switch and resync, alongside `declaration_id`, `slot` and `client_tick`. No other message changes. A field addition to a shipped message bumps `WIRE_VERSION` 32 → 33; it is not a new variant, so the app-protocol constant stays (`networking.md` §Version gates).
