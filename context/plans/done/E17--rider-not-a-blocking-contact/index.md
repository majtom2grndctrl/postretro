# Rider Is Not a Blocking Contact

## Goal

A player riding on top of a kinematic mover must not be treated as an obstruction *by that same mover*. Today an upward mover with a passenger reports itself blocked (stop/reverse) and, under overhead geometry, crushes the passenger. Fix the host blocking pass so a rider's supporting contact with its own mover no longer counts as a block — including the brief moment after the rider jumps or steps off, while it is still airborne beside the mover — while a rider genuinely squeezed against the world above is still crushed.

## Scope

### In scope

- Host blocking pass exempts a player's supporting contact with the mover it is grounded on, for the **Stop** and **Reverse** policies.
- An airborne grace window: a player that has just left a mover it rode stays exempt from that mover's reactive contact for a short, bounded window while airborne, so jumping off a fast upward mover does not make the mover react to its departing passenger.
- Regression coverage that locks the preserved crush semantics: an unobstructed top-rider takes no crush damage; a rider carried into blocking static geometry above is still crushed; crush is unaffected by the grace window.

### Out of scope

- **Crush production code.** The crush arm already yields the supporting-contact-only outcome through its static-pin predicate (see Direction → Problem), and an airborne rider in open air never pins, so the grace never needs to touch crush. The crush arm changes zero lines. Applying the exemption or the grace to crush would delete ceiling-crush — do not touch the crush arm.
- **Agents (enemies) as riders.** Mover carry is player-only; agent grounding is tracked separately (`movement.md` §6). An agent standing on a mover is not carried, so its overlap is a genuine overrun and the policy applies unchanged.
- **Stop/reverse protection against world-crush.** A stop mover carrying a rider into a ceiling does not auto-stop to protect them; the reactive path has no static-pin logic and gains none here. Symmetric with the ratified crush decision — the world-crush is the world's concern, not the mover's rider bookkeeping.
- **Grace for a player on solid world ground.** The grace applies only while airborne. A player who steps off a mover onto adjacent `World` ground standing in the mover's path is a genuine obstruction and blocks/reverses normally.

## Direction

**Problem.** The host blocking pass (`run_mover_blocking_pass`, `crates/postretro/src/kinematic_mover/blocking.rs`) builds player capsules from `PlayerMovementComponent` + `Transform` and never reads the player's `GroundRef`. A passenger grounded on `Mover(id)` is fed into the same leading-face contact test as an obstruction standing in the mover's path. For an upward mover the leading face *is* the top face the rider stands on, so the rider always registers a contact. The **Stop/Reverse** arms react to any such contact via `note_reactive_contact` — the always-reproducing defect ("reacted as though the player was blocking the mover"). The **Crush** arm gates on `mover_push_is_blocked_by_static`: an open-air top-rider's relief push resolves upward and is unblocked, so no pin and no damage — but with geometry overhead the up-push is blocked, the rider pins, and crush lands ("in some instances I saw the player take damage"). The crush arm is therefore already correct; only the reactive path lacks the rider distinction. One tail remains after the grounded exemption: jumping clears ground to `Airborne` (`set_grounded(false)`), so a rider launched off a mover whose per-tick rise outruns the jump's separation is momentarily a non-rider in the leading sweep, and the reactive symptom partially returns for a fast upward mover. A short airborne grace, keyed on having just ridden that mover, closes it.

**Prior commitments.** `movement.md` §6: player grounded state is a `GroundRef` (`Airborne` | `World` | `Mover(u32)`); carry is predicted and reconciled with the pawn; the block decision is host-authoritative and reconciled, never phase-predicted; block policy applies to "players and enemies"; crush deals damage through the entity damage chokepoint. All preserved. This refines "applies to players" to "except the player riding that mover — or briefly airborne from it — on its supporting contact," a scoping of the existing rule, argued below, not a divergence. Because §6 states the unrefined rule, **at promotion** amend it to record the rider exemption (matching the `done/E17--doors-blocking-movers` precedent of updating §6 at promotion), so a later reader does not re-derive "riders block their own movers." No wire or reconciliation change: the fix reads `ground`, already populated host-side by host movement for every simulated pawn, and the grace timer lives in `MoverBlockingState` — explicitly host-only decision state ("intentionally not component state"), never replicated, exactly like the existing crush cadence — inside a pass that already runs host-authoritatively.

**Alternatives rejected.** *Blanket rider exemption* (exempt a rider from its own mover for all policies and all contact directions) — rejected: it deletes ceiling-crush, the mechanic the owner chose to keep. *Fix in carry* (lift the rider clear of the face so no contact forms) — rejected: fragile epsilon tuning at the resting contact, and the crush-into-ceiling case still needs that very contact to fire. *Unify the reactive and crush arms on the `mover_push_is_blocked_by_static` relief test* — rejected: stop/reverse must fire on ordinary *pushable* obstructions in the mover's path (AC 3), which the pin test would let through, regressing shipped `done/E17--doors-blocking-movers` behavior. *Grace as a one-tick "was grounded last tick" flag* — rejected: a mover rising faster than ~2× jump velocity still catches the rider on tick two, so the grace must be a bounded time window, not a single tick. *Grace as a velocity test* (exempt while the rider out-climbs the mover) — rejected: more state and edge cases than a time window buys, for a transient that a fixed window covers. The real discriminant is co-moving attachment — recent or current — which `GroundRef` plus a host-only timer encodes without touching predicted state.

## Acceptance criteria

- [ ] 1. A player grounded on an upward-moving **Stop** mover it rides does not set the mover's `blocked` flag and emits no `Blocked` event; the mover continues to advance across successive ticks.
- [ ] 2. A player grounded on a **Reverse** mover it rides does not reverse the mover's direction.
- [ ] 3. A player standing in the mover's path but **not grounded on it** and with no active grace (ground is `World`, or `Airborne` past the grace window) still blocks a Stop mover and reverses a Reverse mover.
- [ ] 4. A player grounded on mover A that is simultaneously contacted by a **different** mover B still blocks/reverses mover B — the exemption is per-mover.
- [ ] 5. A player riding an upward **Crush** mover with clear space above takes no crush damage and produces no `Crushed` event.
- [ ] 6. A player riding an upward **Crush** mover carried into blocking static geometry above is still crushed on the mover's cadence (ceiling-crush preserved).
- [ ] 7. The grounded exemption tracks `GroundRef` live: a player whose ground becomes `Mover(id)` is exempt from that mover that tick; a player whose ground becomes `World` is not grace-exempt and blocks/reverses if in the path that tick.
- [ ] 8. An enemy (agent) overlapping a Stop/Reverse/Crush mover still triggers the mover's policy — agents are unaffected.
- [ ] 9. A player that jumps off an upward **Stop**/**Reverse** mover moving fast enough that its leading sweep would otherwise reach them mid-air does not cause the mover to block or reverse during the grace window.
- [ ] 10. Once the grace window elapses while the player remains airborne in the mover's leading path, the mover blocks/reverses again — the grace is bounded, never a permanent airborne exemption.
- [ ] 11. The grace window is measured from the last tick the player was grounded on that mover (refreshed every grounded tick): a player who rides for many ticks then jumps still receives the full window. Grace exempts only the mover the player last rode, not other movers.
- [ ] 12. The grace does not suppress crush: a player pinned against a ceiling within the grace window is still crushed (grace is reactive-only).

## Tasks

### Task 1: Ground-keyed reactive exemption

**Exempt the rider's supporting contact in the reactive path.** In `run_mover_blocking_pass` (`crates/postretro/src/kinematic_mover/blocking.rs`), thread each player's `GroundRef` alongside its capsule so the Stop/Reverse arm can identify a passenger. `player_capsules` already reads `PlayerMovementComponent`; extend its returned tuple with `movement.ground` (`GroundRef`, from `postretro_foundation`, already the crate that supplies `PlayerMovementComponent`, and already `PartialEq` — compared with `==` in `movement/mover_carry.rs`). In the player loop's `BlockPolicy::Stop | BlockPolicy::Reverse` arm, before calling `note_reactive_contact`, skip the contact when the player's ground is `GroundRef::Mover(mover.mover_id)` — a co-moving rider is never an obstruction to any face of the platform it rides, so the whole self-mover contact is exempt, not merely the upward one. Leave the `BlockPolicy::Crush` arm and the agent loop untouched: crush's `mover_push_is_blocked_by_static` predicate already produces the supporting-contact-only outcome (no pin in open air, pin against a ceiling), and agents are never carried. Add unit tests to the module's existing `tests` block covering ACs 1–8, following the fixtures already there (`add_player`, `add_enemy`, `swept_wall`, `moving_contact_pose`, `blocking_static_wall`); set a test player's `ground` to `GroundRef::Mover(mover_id)` to model a rider and to `GroundRef::World` to model a path-blocker. The crush criteria (clear-above vs. blocked-above) assert against the existing crush behavior and add no production change — `blocking_static_wall` is the ceiling-analog that pins; its absence is the open-air case.

### Task 2: Airborne rider grace window

**Keep a departing rider exempt while airborne, briefly.** Add a host-only rider-grace timer to `MoverBlockingState`, a `HashMap<(EntityId, EntityId), f32>` keyed by `(mover_entity, player_entity)` holding remaining grace milliseconds, mirroring the existing `crush_elapsed_ms` field and its accessor style; it is host-only decision state and must not become component or wire state (see Invariants). Each pass, drive the timer from live grounding: for every `(mover_entity, player)` pair where the player's threaded `GroundRef` is `Mover(mover.mover_id)`, set the timer to the full window `RIDER_GRACE_MS` (a module-level const in `blocking.rs`); decrement every other entry by `tick_dt * 1000.0` and drop non-positive entries. Extend the Task 1 reactive-arm guard so the contact is skipped when the player is grounded on this mover **or** the player is `GroundRef::Airborne` and a positive grace remains for `(mover.entity, player_entity)`; a `GroundRef::World` player is never grace-exempt. At pass end, retain grace entries only for a live mover and a still-relevant player, the same way crush cadence is retained. Size `RIDER_GRACE_MS` from the constraint, not a guessed feel value: long enough that a rider rising at jump-plus-release velocity out-climbs a mover of up to roughly twice jump velocity before it expires (a handful of movement ticks; ~150 ms is a sound starting point), short enough that it does not shadow a genuine re-entry into the mover's path; tune within that band during implementation. The refresh-only-when-the-mover-is-in-the-pass limitation is harmless: a stopped mover has no leading sweep to react, and while grounded the player is exempt through the direct check regardless of the timer — the timer only governs the airborne tail, by which point the departed-from mover is moving and in the pass. Do not read the grace map in the crush arm or the agent loop. Add unit tests for ACs 9–12, driving `ground` from `Mover(mover_id)` to `Airborne` across successive passes and asserting the mover reacts again only after `RIDER_GRACE_MS` of airborne ticks elapse.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the threaded `GroundRef` in `player_capsules` and the reactive-arm guard site that Task 2 extends.
**Phase 2 (sequential):** Task 2 — consumes Task 1's threaded ground and guard; shares the same arm and `MoverBlockingState`, so it cannot run concurrently with Task 1.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A player is exempt from a mover's reactive (Stop/Reverse) contact iff it is grounded on that mover, or airborne within the grace window of last riding it — never for another mover, never on `World` ground. | Task 1 (ground-keyed exemption), Task 2 (airborne grace) | Reactive arm only; the exemption and grace map are read nowhere in the crush arm or the agent loop. | AC 1–4, 7, 9–11 |
| Crush outcomes are unchanged by this spec — open-air rider unharmed, ceiling pin still crushes — including within the airborne grace window. | Existing crush arm (untouched) | Grace lives in the reactive arm; the crush arm must not consult `GroundRef` or the grace map. | AC 5, 6, 12 |
| Rider grace is host-only decision state, never replicated or predicted. | Task 2 (grace map in `MoverBlockingState`) | No wire/reconcile field added; peers reconcile motion and health only, as with crush cadence. | Host-side unit tests (ACs 9–11); no snapshot/wire change to review |

## Rough sketch

- `player_capsules` → `Vec<(EntityId, Vec3, Capsule, GroundRef)>`; read `movement.ground`.
- `GroundRef` import from `postretro_foundation` (sibling of the existing `PlayerMovementComponent` import).
- Reactive-arm guard (both tasks), inside the `Stop | Reverse` match arm of the player loop only:
  - Task 1: `if player_ground == GroundRef::Mover(mover.mover_id) { continue; }`
  - Task 2 extends it: also `continue` when `player_ground == GroundRef::Airborne` and the `(mover.entity, player_entity)` grace entry is positive.
- `MoverBlockingState` gains `rider_grace_ms: HashMap<(EntityId, EntityId), f32>` plus accessors mirroring the crush-cadence ones; `clear()` clears it too.
- Grace lifecycle per pass: refresh grounded pairs to `RIDER_GRACE_MS`, decrement the rest by `tick_dt * 1000.0`, drop `<= 0`, retain live-mover/relevant-player at pass end.
- Do not add a ground parameter or the grace map to the crush pin checks or to `agent_capsules`.

## Open questions

- None blocking. The stop/reverse-into-ceiling non-protection (Out of scope) is a deliberate accepted edge, symmetric with the ratified crush decision; revisit only if playtesting surfaces a stop-elevator-into-ceiling scenario that feels wrong.

### Accepted edges (verified by depth review, no AC falsified)

- **Command-starved remote pawn.** A remote pawn with no command this tick is not simulated, so it keeps last tick's `ground` (possibly a stale `Mover(id)`) and is briefly exempt while frozen in the mover's path — but it is also not carried, so an upward mover simply rises through it until its next simulated tick re-resolves ground. Self-correcting and consistent with existing starvation behavior; the "every *simulated* pawn" reasoning already covers it. No change.
