# movement--mover-pinned-rider

Brief · compact · Epic 17 · reads: `context/lib/movement.md` §1, §6, `context/lib/networking.md` §Current contract (mover prediction, block reaction), `context/lib/entity_model.md` §5 · read at c1cad7e

## Problem
A non-crush mover can carry or push a player into geometry it cannot push them through. The carry is unswept, the `stop`/`reverse` rider skip has no pin test, and the default `displace` never yields when its push is refused. The player ends inside a ceiling, a wall or the mover; on a one-way mover nothing frees them (`research.md` lists every case). Found by reading source, named in `movement--static-overlap-invariant`'s Non-goals, queued before "Enemy–mover interaction". When done, no mover under `displace`, `stop` or `reverse` leaves a player pinned against static geometry or another mover, by one rule mappers can read in the docs. Only `crush` keeps pressing.

## Decisions
- **Only `crush` may leave a body pinned.** Every other policy yields to a pinned body.
  - Diverges from `done/E17--doors-blocking-movers` AC4 (default "byte-for-byte as today"), `movement.md` §6 ("never yield") and `done/E17--rider-not-a-blocking-contact` ("no static-pin logic"). Owner ratified.
  - Warrant: a pin is a softlock, so no working map relies on one. Every classic default pusher (Quake, Doom, Source) refuses to move into what it cannot push clear (`research.md` §Prior art).
- **One pin definition, every policy.**
  - A rider is the body the mover carries: grounded on it, or vaulting onto it (carry ground). It is pinned when its carry would sweep into static geometry or another mover.
  - Any other body is pinned when the mover's push on it is blocked by static geometry or another mover. Only the blocking pass's pin test widens; the substrate's displacement keeps ignoring movers. Without the widening, a default double door leaves a player inside a leaf.
  - So `crush` also crushes a rider carried sideways into a wall, and a body squeezed between two movers. This changes authored crushers (`campaign-test`, the stress-warren maps); owner ratified.
- **The engine guarantees no embed; authors decide the response.**
  - The engine's yield is a hold, the least prescriptive response. `displace` and `stop` hold.
  - `reverse` reverses when its heading drives into the pin and the reversed course pins nothing. Otherwise it holds: pathless (as `effective_block_policy` already degrades it), pinned by spin or off its heading, or reversing onto another body.
  - A hold is the replicated stop hold. It freezes translation and spin and resumes the mover's own course on the first tick the preview no longer pins. It keeps the started state and any scripted `goToPathNode` target (an auto-close expiry still overwrites the target, pin Q1), so a `once` mover finishes its run once the body is clear.
  - Every other response is authored: `reverse` to back off, a `blocked_event` reaction for anything a script can do, `crush` for a door that must close. Door behaviour is the author's choice, not the engine's. A held lift in an enclosed shaft leaves its rider waiting, not embedded; the warning names it.
- **No course reaches the driver unpreviewed.**
  - The host tests pins one tick ahead, on this tick's pose and the preview of the next, so it yields before a carry embeds a rider.
  - Commands applied after the pass (the frame-end script drain, player-event fire lists, any later source) cannot slip past it: every started mover is pin-tested again before the next mover tick. A command moves a held mover only along a course that pins nothing, so authored responses are safe by construction.
- **Onset fires once per pin.** The host remembers, off the wire, which movers were held at the last pin test. A command that clears a hold the next test re-establishes is not a new onset. The blocked event and sound fire again only after a real release (a stop, a completion, a clear preview), so a reaction on a held mover's own `blocked_event` cannot loop on itself.
- **A reversed `once` mover can start again.** A reversal (the `reverse` policy or `moverReverse`) that brings a `once` mover back to its start leaves it unstarted, so its trigger runs it again. Today it stays started and every start no-ops, which would strand the authored responses above. Replicated phase only.
- **A pin overrides the rider skip and the airborne rider grace**, as crush already ignores the grace. Unpinned contacts keep today's reactions, and unpinned motion is unchanged: `displace` still pushes what it can push clear.
- **Carry stays unswept and exempt from the overlap check** (`movement.md` §1). The guarantee is the mover yielding, not the body.
- **The block decision stays host-only** (`networking.md` §Current contract). A client predicts the mover moving until the new phase arrives, so its pawn can embed for that window; the reconcile clears it. This is the correction a `stop` door already takes. Predicting the yield instead would put block policy into the digest and on the wire. No wire field, digest input, or FGD key or value changes.
- **Placement.** Mover policy in `postretro-physics`: the host blocking pass and the pin test before the mover tick. The movement substrate does not change.
- **Mapper surface.**
  - FGD `block_policy` help and the SDK `setBlockPolicy` doc comment state the pin rule. `blocked_event` and `blocked_sound` help keeps "when a blocking mover reacts to contact" and adds the `displace` hold. Help text only.
  - `docs/level_design.md` gains a movers section: what each policy does to players; that a default mover waits for a pinned player, then carries on; how to author other responses (`reverse`, a `blocked_event` reaction); that `crush`, even at zero damage, never yields; that in co-op any player can hold a default door, so a door that must close authors `crush`. No agent behaviour yet.
- **A yield to a pinned rider warns once per mover per level load**, naming the mover and its TrenchBroom position. In an enclosed shaft a held lift is otherwise the mapper's only sign of too little headroom.
- **Non-goals.**
  - **Agents.** Never carried or displaced (`movement.md` §6), so no mover pins one; their reactions stay as they are. "Enemy–mover interaction" adopts this predicate (`context/research/enemy-mover-interaction.md` §8).
  - **Static depenetration of an embedded body.** A pinned body has no free pose until the mover yields, and yielding movers no longer create one.
  - **Multi-victim crush order.** Belongs to "Enemy–mover interaction" (§4 there).

## Acceptance
### Automated
**Reproduce first** (at c1cad7e; each committed as an expected failure naming the bug; the fix makes it a plain test)
- [ ] A rider on a default lift rising to a stop with less headroom than the standing capsule overlaps the ceiling by more than 0.5 mm, and still does 60 ticks after the lift stops. Replaces the fixture asserting a rider rides through a ceiling.
- [ ] A pawn a default door closes on against a static jamb, and one caught between two default door leaves, each ends inside a door's hull.
- [ ] Under `crush` with non-zero `crush_damage`, a rider carried sideways into a static wall takes no damage and ends inside the wall; a pawn squeezed between the crusher and another mover takes no damage.
- [ ] A `once` door under `reverse` that backs off a pawn to its start ignores its trigger's next start.

**No non-crush mover leaves a body pinned** (driver, player movement and host blocking pass in tick order, under `displace`, `stop` and `reverse`; "resumes" is a holding mover's outcome, a reversing one heads back along its path)
- [ ] A rider lifted toward a too-low ceiling never ends a tick overlapping static geometry by more than 0.5 mm. A `displace` or `stop` lift holds; a `reverse` lift carries it back down.
- [ ] A rider overhanging a lift's edge under an upper-floor slab edge: the lift yields; when the rider steps inward it resumes the next tick and reaches its top stop with the rider clear.
- [ ] A rider on a platform under a low arch, and one on a carousel whose rim passes under a static pillar, never end a tick inside static geometry. The carousel's spin holds under every policy, `reverse` included. Pin Q6.
- [ ] A rider on a lift rising toward a closed hatch mover never overlaps the hatch's hull; the lift resumes the tick after the hatch opens clear of the rider's carry.
- [ ] Pawns against a jamb as a door closes, under a descending lift, against a wall a rotating door swings toward, and between two closing door leaves are never left more than one tick's mover travel inside the mover. After each steps away, the movers resume the next tick and complete.
- [ ] A rider that jumped off a rising lift and is caught against a ceiling within the grace window counts as pinned.
- [ ] A vault onto a rising mover under a ceiling never ends a tick inside the ceiling.
- [ ] Two pawns on one tick, one pinned and one not: the mover yields, whatever order the pawns are visited in.
- [ ] The tick after the preview first pins, a holding mover moves nothing and its rider ends where it ended the tick before; a reversing one carries its rider away. Pin Q5.
- [ ] A `reverse` lift whose rider is pinned against a ceiling while a pawn stands pinned under it holds: it never moves onto the pawn, never changes direction on successive ticks, and fires one blocked event. Pin Q8.
- [ ] A yield to a pinned rider logs one warning naming the mover and a TrenchBroom position; later yields of that mover in the same level load are silent, and after a reload the next one warns.

**Hold, onset and restart**
- [ ] A default `once` door holding for a pawn at its jamb closes the tick after the pawn steps away and completes its run. A default lift sent by `moverGoToPathNode` that holds for a pinned rider keeps the target and reaches it once the rider is clear (auto-close off).
- [ ] The blocked event and sound fire once per onset, not per held tick. A reaction on a held mover's own `blocked_event` that runs `moverStart`, or `moverReverse` where both courses pin, fires the event once for the whole pin, and the mover never changes direction on successive ticks.
- [ ] A held mover released by `moverStop`, then started again into the pin, fires the blocked event again.
- [ ] A `once` door under `reverse` that backs off a pawn to its start ends unstarted; its trigger's next start runs it to completion.

**Orderings** (`research.md` pins)
- [ ] An auto-close timer that expires while a pawn pins the door never moves the door into the pawn; a closing course into the pin yields. Pin Q1.
- [ ] A trigger-stage `start` or `reverse` on a held mover moves it only along a course that leaves the pin; a course into the pin yields again. Pin Q2.
- [ ] A pinned pawn that despawns or disconnects releases the hold on the next tick. Pin Q3.
- [ ] `setBlockPolicy("crush")` on a held mover resumes it the next tick and crushes; a crusher switched to `displace` mid-pin yields the next tick. Pin Q4.
- [ ] A `moverStart`, `moverReverse`, `moverGoToPathNode` or `moverSetSpinRate` reaction from the frame-end script drain on a mover holding for a pinned rider never carries the rider into static geometry on the next tick. A `blocked_event` reaction running `moverReverse` on a held default door backs it off the pawn, and a later start or its trigger closes it again. Pin Q7.
- [ ] A player event whose fire list starts or reverses a mover holding for a pinned rider, in a frame running two fixed ticks, never carries the rider into static geometry on the next tick. Pin Q9.

**Unpinned motion unchanged** (regression guards)
- [ ] A default slider pushing a pawn across open floor, and a rider on a default lift with headroom, reproduce their c1cad7e positions exactly.
- [ ] A grounded rider in open air never stops or reverses its own `stop`/`reverse` mover; a pawn standing in the path still does.
- [ ] Existing E17 blocking and rider tests pass unchanged, except the default's pinned-push case, which this brief restates.
- [ ] Agents: a default door still ignores an enemy, and `stop`/`reverse`/`crush` treat enemies as at c1cad7e.
- [ ] Under `crush`, a rider lifted into a ceiling is still damaged on its cadence, the mover keeps moving, and the armed overlap assertion stays silent.
- [ ] Under `crush` with non-zero `crush_damage`, a rider carried sideways into a static wall and a pawn squeezed between the crusher and another mover each take damage on its cadence while it keeps moving.

**Co-op and gates**
- [ ] Loopback harness with the host blocking pass running, under the mandated link profile: a client riding a default lift toward a too-low ceiling converges to the host's pose and overlaps static geometry on no tick after the first reconcile carrying the hold. The largest correction is reported in the plan of record.
- [ ] Diff gate: no wire field, content-digest input, or FGD key or value is added.
- [ ] `docs/` gains no `cargo run -p xtask`.

### Manual
- [ ] Single player, on a new dev map holding every case above: no embed, no camera inside a ceiling, and each yield reads as the mover waiting for, or backing off from, the player. The blocked sound plays where authored.
- [ ] Co-op client under the mandated link profile on the same map: no lasting embed, and the correction when a lift yields is acceptable feel.
- [ ] A mapper reading only the FGD help and `docs/level_design.md` predicts, for each case on the map, whether the mover pushes, yields or crushes.

## Path
- **Shape:** the mover holds for a pinned body, reusing the stop hold; `reverse` keeps its reversal.
- **Seams.**
  - `run_mover_blocking_pass`: the displace snapshot filter, the `Stop | Reverse` rider skip, and the stale-hold sweep.
  - `preview_next_mover_pose`; `mover_push_is_blocked_by_static`, with the rider sweep beside it to `apply_mover_carry`'s end pose. Other movers can be tested at their current poses.
  - `player_capsules` returns `movement.ground`; rider identity needs `carry_ground` (movement/mod.rs).
  - Reverse's heading test needs a normal: the push or carry direction. A reversal previews its reversed course before it commits (pin Q8).
  - `apply_mover_command` (`Start`, `Reverse`, `GoToPathNode`) and `travel_toward_closed_terminus` clear `blocked`. No host record says which movers a command changed, so the pre-tick pin test covers every started mover at the top of `simulate_tick`. Nothing moves a player between ticks, so the pass's pin decision may move there whole.
  - Reversed `once` restart: `Start`'s early return (kinematic_mover/command.rs) and the driver's backward arrival at waypoint 0 (kinematic_mover.rs), which never ends the run.
- **Strongest rival:** swept carry. Rejected: the platform then rises into the rider and must still yield, and it breaks `movement.md` §1's carry exemption.
- **First slice:** the reproduce-first rider fixture under driver, movement and pass. It falsifies the riskiest premise: that the one-tick preview yields before the carry embeds.
- **Tests.** The overlap_tests.rs mover fixtures place poses by hand and never run the pass; the netcode loopback `host_simulate_tick` skips it too.
- **Files past 800 lines:** kinematic_mover/blocking.rs, mostly tests. Move its test module to a sibling file first, in its own behaviour-preserving commit.
- **At landing:** `movement.md` §1 Not covered bullet and §6's displace and rider-exemption text; `networking.md` §Current contract, where the stop hold now also serves `displace`.

## Open questions
- None.
