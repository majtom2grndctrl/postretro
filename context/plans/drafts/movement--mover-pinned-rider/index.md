# movement--mover-pinned-rider

Brief · compact · Epic 17 · reads: `context/lib/movement.md` §1, §6, `context/lib/networking.md` §Current contract (mover prediction, block reaction), `context/lib/entity_model.md` §5 · read at c1cad7e

## Problem
Raised by the `movement--static-overlap-invariant` review: its Non-goals name this softlock, and the roadmap queues it before "Enemy–mover interaction". Basis: a defect found by reading source; the first fixtures reproduce it. Cause: nothing stops a non-crush mover from carrying or pushing a player into geometry it cannot push them through. The carry is unswept, the `stop`/`reverse` rider skip has no pin test, and the default `displace` never yields when its push is refused. The player is left inside a ceiling, a wall or the mover, and on a one-way mover nothing frees them (`research.md` has every case). When done, no mover under `displace`, `stop` or `reverse` leaves a player pinned against static geometry or another mover. It holds or reverses until the body is clear, by one rule mappers can read in the docs and predict in TrenchBroom. Only `crush` keeps pressing.

## Decisions
- **Only `crush` may leave a body pinned. Every other policy yields to a pinned body.**
  - This diverges from `done/E17--doors-blocking-movers` AC4 (the default stays "byte-for-byte as today"), from `movement.md` §6 ("never yield"), and from `done/E17--rider-not-a-blocking-contact`'s "no static-pin logic" for stop/reverse riders.
  - Argument: a pin is a softlock, so no working map can rely on one. And every classic default pusher (Quake, Doom, Source) refuses to move into what it cannot push clear (`research.md` §Prior art).
  - Owner ratified, with the `crush` widening below.
- **Pinned has one definition, for every policy, crush included.**
  - A rider is the body the mover carries, keyed on carry ground: grounded on it, or vaulting onto it. A rider is pinned when its carry would sweep into static geometry or into another mover.
  - Any other body is pinned when the mover's push on it is blocked by static geometry or by another mover. This widens the blocking pass's pin test only; the movement substrate's displacement keeps ignoring movers. Without the widening, a default double door closing on a player leaves them inside a leaf.
  - Crush therefore also crushes a rider carried sideways into a wall, and a body squeezed between two movers. This changes authored crushers (`campaign-test`, the stress-warren maps); owner ratified.
- **The host decides one tick ahead.** The pin test runs on this tick's pose and on the preview of next tick's pose. So on the host the mover yields before its carry embeds a rider.
- **No course reaches the driver unpreviewed.** Whatever changed a mover's phase after the pass (the frame-end script drain, player-event fire lists, any later command source), every started mover is pin-tested again before the next mover tick, and yields there as the pass would. A command can move a held mover only along a course that pins nothing, so author-built responses to a pin are safe by construction.
- **A pin overrides the rider skip and the airborne rider grace**, as crush already ignores the grace. Unpinned contacts keep today's reactions exactly.
- **The engine guarantees no embed; authors decide what a pinned mover does next.** The engine's yield is the least prescriptive one, a hold: `displace` and `stop` hold. `reverse` reverses when its path heading drives into the pin and the reversed course pins nothing; otherwise it holds (pathless, as `effective_block_policy` already degrades it, a pin from spin or off the heading, or a reversed course that would pin another body). A hold is the replicated stop hold: it freezes translation and spin, fires the blocked event and sound at onset, and resumes its own course on the first tick the preview no longer pins. A hold keeps the mover's started state and any scripted `goToPathNode` target (unless an auto-close timer expires during the hold, pin Q1), so a `once` mover finishes its run once the body is clear. No wire field, no digest input, no FGD key or value.
  - Any other response is authored: `reverse` for a Quake-style bounce, a `blocked_event` reaction for anything a script can do, `crush` for a door that must close. Owner chose this over making `displace` reverse: how doors behave is the author's choice, not the engine's. A held lift in an enclosed shaft leaves its rider waiting, not embedded; the warning below names it, and the author's tools fix it.
- **Onset fires once per pin.** The host remembers, off the wire, which movers were held at the last pin test. A command that clears a hold the next test re-establishes is not a new onset: the blocked event and sound fire again only after a real release. A reaction on a held mover's own `blocked_event` therefore cannot loop on itself. The record keeps its entries across a pass with no movers in it, but drops a mover once it is released (a stop, a completion, a clear preview), so a later real onset still fires.
- **A reversed `once` mover can start again.** A `once` mover that a reversal (the `reverse` policy or a `moverReverse` command) brings back to its start ends unstarted, so a later start or its trigger runs it again. Today it stays started and every start no-ops, which would strand the authored responses this brief points to. Replicated phase only; no new wire field.
- **Unpinned motion is unchanged.** A `displace` mover still pushes what it can push clear, and a rider with headroom rides as before.
- **Carry stays unswept and exempt from the overlap check** (`movement.md` §1). The guarantee is the mover yielding, not the body.
- **The block decision stays host-only** (`networking.md` §Current contract). A client predicts the mover moving until the new phase arrives, and its pawn can embed for that window. The reconcile against the held or reversed mover clears it. Owner accepted this correction, the one a `stop` door already takes, over predicting the yield on the client, which would put block policy into the digest and on the wire.
- **Placement.** Mover policy, in the host blocking pass in `postretro-physics`, and the host's pin test before the mover tick. The movement substrate does not change.
- **Mapper surface.** The FGD `block_policy` help and the SDK `setBlockPolicy` doc comment state the pin rule. The `blocked_event` and `blocked_sound` help keeps "when a blocking mover reacts to contact" and adds that they also fire when a `displace` mover holds for a pinned body; help text only, no key or value change. `docs/level_design.md` gains a movers section. It says what each policy does to players and gives the pin rule: a default mover waits for a pinned player to step away, then carries on. It says how to author other responses: `reverse` for a mover that backs off, a `blocked_event` reaction for a scripted one. It says that `crush`, even at its default zero damage, is the one policy that never yields. It says that in co-op any player can hold a default door by standing where it would pin them, so a door that must close authors `crush`. It covers no agent behaviour yet.
- **A yield to a pinned rider warns once per mover per level load**, naming the mover and its TrenchBroom position. In an enclosed shaft a held lift is otherwise the mapper's only sign of too little headroom.
- **Non-goals.**
  - **Agents.** They are never carried or displaced (`movement.md` §6), so no mover pins one, and agent reactions stay as they are. "Enemy–mover interaction" adopts this predicate (`context/research/enemy-mover-interaction.md` §8).
  - **Static depenetration of an embedded body.** A pinned body has no free pose until the mover yields, and once movers yield they no longer create one.
  - **Crush beyond the shared predicate.** Multi-victim crush order belongs to "Enemy–mover interaction" (§4 there).

## Acceptance
### Automated
**Reproduce first** (at c1cad7e; each is committed as an expected failure naming the bug, and the fix makes it a plain test)
- [ ] A rider on a default lift rising to a stop with less headroom than the standing capsule ends ticks overlapping the ceiling by more than 0.5 mm. It is still overlapping 60 ticks after the lift stops. It replaces the fixture that asserts a rider rides through a ceiling.
- [ ] A pawn that a default door closes on against a static jamb, and one caught between two default door leaves meeting, each ends inside a door's hull once the doors close.
- [ ] Under `crush` with non-zero `crush_damage`, a rider carried sideways into a static wall takes no damage and ends inside the wall. A pawn squeezed between the crusher and another mover takes no damage either.

**No non-crush mover leaves a body pinned** (driver, player movement and host blocking pass in tick order, under `displace`, `stop` and `reverse`. "Resumes" is a holding mover's outcome; a reversing one heads back along its path instead.)
- [ ] A rider lifted toward a too-low ceiling never ends a tick overlapping static geometry by more than 0.5 mm. A `displace` or `stop` lift holds; a `reverse` lift carries it back down.
- [ ] A rider overhanging a lift's edge, rising under an upper-floor slab edge: the lift yields. When the rider steps toward the platform's centre, the lift resumes the next tick and reaches its top stop with the rider clear.
- [ ] A rider on a platform translating under a low arch, and one on a spinning carousel whose rim passes under a static pillar, never end a tick inside static geometry. The carousel's spin holds under every policy, `reverse` included. Pin Q6.
- [ ] Each of these pawns is never left more than one tick's mover travel inside the mover: one standing against a jamb as a door closes, one under a descending lift, one against a wall a rotating door swings toward, and one between two closing door leaves. After it steps away, the movers resume the next tick and complete.
- [ ] A rider on a lift rising toward a closed hatch mover never ends a tick overlapping the hatch's hull. The lift yields, and resumes the tick after the hatch opens clear of the rider's carry.
- [ ] A rider that jumped off a rising lift and is caught against a ceiling within the grace window counts as pinned.
- [ ] A vault onto a rising mover under a ceiling never ends a tick inside the ceiling.
- [ ] Two pawns on one tick, one pinned and one not: the mover yields, whatever order the pawns are visited in.
- [ ] The tick after the preview first pins, a holding mover moves nothing and its rider ends where it ended the tick before. A reversing one carries its rider away from the pin. Pin Q5.
- [ ] A held mover released by `moverStop`, then started again into the pin, fires the blocked event again.
- [ ] The blocked event and sound fire once per onset, not per held tick. A reaction on a held mover's own `blocked_event` that runs `moverStart`, or `moverReverse` where both courses pin, fires the event once for the whole pin, and the mover never changes direction on successive ticks.
- [ ] A yield to a pinned rider logs one warning naming the mover and a TrenchBroom position. Later yields of that mover in the same level load are silent, and after a reload the next one warns.

**Orderings** (`research.md` pins)
- [ ] An auto-close timer that expires while a pawn pins the door never moves the door into the pawn. It yields if its closing course drives into the pin. Pin Q1.
- [ ] A `start` or `reverse` command on a held mover never moves it into the pin. A command whose course leaves the pin moves it, and one that drives into the pin yields again. Pin Q2.
- [ ] A default `once` door that holds for a pawn at its jamb closes the tick after the pawn steps away and completes its run; a default lift sent by `moverGoToPathNode` that holds for a pinned rider keeps the target and reaches it once the rider is clear.
- [ ] A `blocked_event` reaction that runs `moverReverse` on a held default door backs it off the pawn, never into it, and a later start or its trigger closes it again. Pin Q7.
- [ ] A `once` door under `reverse` that backs off a pawn to its start ends unstarted: its trigger's next start runs it to completion. Before the fix, that start does nothing.
- [ ] A pinned pawn that despawns or disconnects releases the hold on the next tick. Pin Q3.
- [ ] `setBlockPolicy("crush")` on a held mover resumes it the next tick and crushes. A crusher switched to `displace` mid-pin yields the next tick. Pin Q4.
- [ ] A `moverStart`, `moverReverse`, `moverGoToPathNode` or `moverSetSpinRate` reaction run from the frame-end script drain on a mover holding for a pinned rider never carries the rider into static geometry on the next tick. Pin Q7.
- [ ] A player event whose fire list starts or reverses a mover holding for a pinned rider, in a frame that runs two fixed ticks, never carries the rider into static geometry on the next tick. Pin Q9.
- [ ] A `reverse` lift whose rider is pinned against a ceiling while a pawn stands pinned under it holds. It never moves onto the pawn, never changes direction on successive ticks, and fires one blocked event. Pin Q8.

**Unpinned motion unchanged** (regression guards)
- [ ] A default slider pushing a pawn across open floor, and a rider on a default lift with headroom, reproduce their c1cad7e positions exactly.
- [ ] A grounded rider in open air never stops or reverses its own `stop`/`reverse` mover, and a pawn standing in the path still does.
- [ ] Existing E17 blocking and rider tests pass unchanged, except the default's pinned-push case, which this brief restates.
- [ ] Agents: a default door still ignores an enemy, and `stop`/`reverse`/`crush` treat enemies as at c1cad7e.
- [ ] Under `crush`, a rider lifted into a ceiling is still damaged on its cadence, the mover keeps moving, and the armed overlap assertion stays silent.
- [ ] Under `crush` with non-zero `crush_damage`, a rider carried sideways into a static wall, and a pawn squeezed between the crusher and another mover, each take damage on the crusher's cadence while it keeps moving.

**Co-op and gates**
- [ ] Loopback harness with the host blocking pass running, under the mandated link profile: a client riding a default lift toward a too-low ceiling converges to the host's pose. Its pawn overlaps static geometry on no tick after the first reconcile carrying the yield. The largest correction is reported in the plan of record.
- [ ] Diff gate: no wire field, content-digest input, or FGD key or value is added.
- [ ] `docs/` gains no `cargo run -p xtask`.

### Manual
- [ ] Single player, on a new dev map holding every case above: no embed, no camera inside a ceiling, and each yield reads as the mover waiting for, or backing off from, the player. The blocked sound plays where authored.
- [ ] Co-op client at 45/60/5 on the same map: no lasting embed, and the correction when a lift yields is acceptable feel.
- [ ] A mapper reading only the FGD help and `docs/level_design.md` predicts, for each case on the map, whether the mover pushes, yields or crushes.

## Path
- **Seams.**
  - `run_mover_blocking_pass`: the displace snapshot filter, the `Stop | Reverse` rider skip, and the stale-hold sweep, which clears `blocked` on every non-`stop` mover.
  - `preview_next_mover_pose`; `mover_push_is_blocked_by_static`, with the rider test beside it, sweeping to `apply_mover_carry`'s end pose. The other-mover blocker can use other movers' current poses.
  - `player_capsules` returns `movement.ground`; rider identity needs `carry_ground` (movement/mod.rs).
  - Reverse's heading test needs a normal: use the push or carry direction.
  - `apply_mover_command` (`Start`, `Reverse`, `GoToPathNode`) and `travel_toward_closed_terminus` clear `blocked`. Trigger-stage commands run before the pass; the frame-end script drain and player events run after it, and no host record says which movers they changed, so the pin test sits at the top of `simulate_tick`, before the mover tick, over every started mover. Since nothing moves a player between ticks, the pass's pin decision may move there whole.
  - A reversal needs its own preview of the reversed course before it commits (pin Q8).
- **Shape:** the mover holds for a pinned body, reusing the stop hold; `reverse` keeps its reversal.
- **Strongest rival:** swept carry. Rejected: the platform then rises into the rider and must still yield, and it changes `movement.md` §1's carry exemption.
- **First slice:** the reproduce-first rider fixture under driver, movement and pass. It falsifies the riskiest premise: that the one-tick preview yields before the carry embeds.
- **Tests.** The overlap_tests.rs mover fixtures place poses by hand and never run the pass. The netcode loopback `host_simulate_tick` skips the pass too.
  - A reversed `once` mover: `Start`'s early return (`completed || started && wait_remaining_ms <= 0`, kinematic_mover/command.rs) and the driver's backward arrival at waypoint 0 (kinematic_mover.rs), which never ends the run.
- **Files past 800 lines:** kinematic_mover/blocking.rs, mostly tests. Move its test module to a sibling file first, in its own behaviour-preserving commit.
- **At landing:** in `movement.md`, replace the §1 Not covered bullet and restate §6's displace and rider-exemption text. In `networking.md` §Current contract, the stop hold now also serves `displace`.

## Open questions
- None. The owner settled the divergence (with the `crush` widening), hold for `displace` with every other response authored, the client reconcile correction, and the pin test for every command after the pass; each is recorded in Decisions.
