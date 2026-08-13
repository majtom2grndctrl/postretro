# Rider Is Not a Blocking Contact

## Goal

A player riding on top of a kinematic mover must not be treated as an obstruction *by that same mover*. Today an upward mover with a passenger reports itself blocked (stop/reverse) and, under overhead geometry, crushes the passenger. Fix the host blocking pass so a rider's supporting contact with its own mover no longer counts as a block — while a rider genuinely squeezed against the world above is still crushed.

## Scope

### In scope

- Host blocking pass exempts a player's supporting contact with the mover it is grounded on, for the **Stop** and **Reverse** policies.
- Regression coverage that locks the preserved crush semantics: an unobstructed top-rider takes no crush damage; a rider carried into blocking static geometry above is still crushed.

### Out of scope

- **Crush production code.** The crush arm already yields the supporting-contact-only outcome through its static-pin predicate (see Direction → Problem). It changes zero lines here. Applying the exemption to crush would delete ceiling-crush — do not touch the crush arm.
- **Agents (enemies) as riders.** Mover carry is player-only; agent grounding is tracked separately (`movement.md` §6). An agent standing on a mover is not carried, so its overlap is a genuine overrun and the policy applies unchanged.
- **Stop/reverse protection against world-crush.** A stop mover carrying a rider into a ceiling does not auto-stop to protect them; the reactive path has no static-pin logic and gains none here. Symmetric with the ratified crush decision — the world-crush is the world's concern, not the mover's rider bookkeeping.

## Direction

**Problem.** The host blocking pass (`run_mover_blocking_pass`, `crates/postretro/src/kinematic_mover/blocking.rs`) builds player capsules from `PlayerMovementComponent` + `Transform` and never reads the player's `GroundRef`. A passenger grounded on `Mover(id)` is fed into the same leading-face contact test as an obstruction standing in the mover's path. For an upward mover the leading face *is* the top face the rider stands on, so the rider always registers a contact. The **Stop/Reverse** arms react to any such contact via `note_reactive_contact` — this is the always-reproducing defect ("reacted as though the player was blocking the mover"). The **Crush** arm gates on `mover_push_is_blocked_by_static`: an open-air top-rider's relief push resolves upward and is unblocked, so no pin and no damage — but with geometry overhead the up-push is blocked, the rider pins, and crush lands ("in some instances I saw the player take damage"). The crush arm is therefore already correct; only the reactive path lacks the rider distinction.

**Prior commitments.** `movement.md` §6: player grounded state is a `GroundRef` (`Airborne` | `World` | `Mover(u32)`); carry is predicted and reconciled with the pawn; the block decision is host-authoritative and reconciled, never phase-predicted; block policy applies to "players and enemies"; crush deals damage through the entity damage chokepoint. All preserved. This refines "applies to players" to "except the player riding that mover, on its supporting contact" — a scoping of the existing rule, argued below, not a divergence from it. No wire or reconciliation change: the fix reads `ground`, already populated host-side by host movement for every simulated pawn, inside a pass that already runs host-authoritatively.

**Alternatives rejected.** *Blanket rider exemption* (exempt a rider from its own mover for all policies and all contact directions) — rejected: it deletes ceiling-crush, the mechanic the owner chose to keep. *Fix in carry* (lift the rider clear of the face so no contact forms) — rejected: fragile epsilon tuning at the resting contact, and the crush-into-ceiling case still needs that very contact to fire. Scoping the exemption to the reactive path, keyed on `GroundRef`, is the narrowest shape that fixes the defect and preserves the decision.

## Acceptance criteria

- [ ] A player grounded on an upward-moving **Stop** mover it rides does not set the mover's `blocked` flag and emits no `Blocked` event; the mover continues to advance across successive ticks.
- [ ] A player grounded on a **Reverse** mover it rides does not reverse the mover's direction.
- [ ] A player standing in the mover's path but **not grounded on it** (ground is `World`/`Airborne`) still blocks a Stop mover and reverses a Reverse mover — the exemption is scoped to the rider's own ground-mover, not to any overlap.
- [ ] A player grounded on mover A that is simultaneously contacted by a **different** mover B still blocks/reverses mover B — the exemption is per-mover.
- [ ] A player riding an upward **Crush** mover with clear space above takes no crush damage and produces no `Crushed` event.
- [ ] A player riding an upward **Crush** mover carried into blocking static geometry above is still crushed on the mover's cadence (ceiling-crush preserved).
- [ ] The exemption tracks `GroundRef` live: a rider whose ground becomes `World`/`Airborne` (steps off) is no longer exempt and blocks/reverses if in the path on the tick it leaves; a player whose ground becomes `Mover(id)` (steps on) becomes exempt that tick.
- [ ] An enemy (agent) overlapping a Stop/Reverse/Crush mover still triggers the mover's policy — agents are unaffected.

## Task

**Exempt the rider's supporting contact in the reactive path.** In `run_mover_blocking_pass` (`crates/postretro/src/kinematic_mover/blocking.rs`), thread each player's `GroundRef` alongside its capsule so the Stop/Reverse arm can identify a passenger. `player_capsules` already reads `PlayerMovementComponent`; extend its returned tuple with `movement.ground` (`GroundRef`, from `postretro_foundation`, already the crate that supplies `PlayerMovementComponent`). In the player loop's `BlockPolicy::Stop | BlockPolicy::Reverse` arm, before calling `note_reactive_contact`, skip the contact when the player's ground is `GroundRef::Mover(mover.mover_id)` — a co-moving rider is never an obstruction to any face of the platform it rides, so the whole self-mover contact is exempt, not merely the upward one. Leave the `BlockPolicy::Crush` arm and the agent loop untouched: crush's `mover_push_is_blocked_by_static` predicate already produces the supporting-contact-only outcome (no pin in open air, pin against a ceiling), and agents are never carried. Add unit tests to the module's existing `tests` block covering every acceptance criterion, following the fixtures already there (`add_player`, `add_enemy`, `swept_wall`, `moving_contact_pose`, `blocking_static_wall`); set a test player's `ground` to `GroundRef::Mover(mover_id)` to model a rider, and to `GroundRef::World` to model a path-blocker. The crush criteria (clear-above vs. blocked-above) assert against the existing crush behavior and add no production change — `blocking_static_wall` is the ceiling-analog that pins; its absence is the open-air case.

## Rough sketch

- `player_capsules` → `Vec<(EntityId, Vec3, Capsule, GroundRef)>`; read `movement.ground`.
- Reactive arm guard: `if player_ground == GroundRef::Mover(mover.mover_id) { continue; }` placed before `note_reactive_contact`, inside the `Stop | Reverse` match arm of the player loop only.
- `GroundRef` import from `postretro_foundation` (sibling of the existing `PlayerMovementComponent` import). `GroundRef` already derives `PartialEq` (compared with `==` in `movement/mover_carry.rs`).
- Do not add a ground parameter to the crush pin checks or to `agent_capsules`.

## Open questions

- None blocking. The stop/reverse-into-ceiling non-protection (Out of scope) is a deliberate accepted edge, symmetric with the ratified crush decision; revisit only if playtesting surfaces a stop-elevator-into-ceiling scenario that feels wrong.
