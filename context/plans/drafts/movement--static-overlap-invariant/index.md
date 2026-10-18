# movement--static-overlap-invariant

Brief · compact · reads: `context/lib/entity_model.md` §7 (Collision), `context/lib/movement.md` §4 (Vault), §6, §7, `context/lib/development_guide.md` §6.4 · read at b0a5c0f

## Problem
Owner co-op playtest under the dev lag proxy: after a vault, the player's pawn sank into the brush it had vaulted over and stayed embedded until the player walked out. Basis: observed defect for the player pawn. The enemy half was found by reading source: the agent controller copies the same moves. The lossy link triggered the embed, because the host ran held and then neutral input against the lip (`coop--lossy-link-delivery`). The defect itself lives in the movement code and can occur in any role. Cause: collision integration moves the capsule in places that bypass the swept cast, which can carry it into static geometry, and nothing ever pushes it back out. When done, a body that starts a tick clear of static geometry cannot end that tick intersecting it, whether it is a player pawn or an enemy, on any peer.

## Decisions
- **New invariant: own movement never ends a tick intersecting static geometry if it started clear.**
  - It covers player pawns and enemy agents.
  - Intersecting means the capsule overlaps a static surface by more than 0.5 mm. Sitting inside the skin band is resting contact, not intersection.
  - It extends the skin-cast contract in `entity_model.md` §7 from one cast to a whole tick.
  - Exempt moves are mover carry and mover displacement (mover policy), teleport relocation (which has its own fit check), and the sub-tolerance wall-contact nudge.
- **Own position changes go through a swept move.** This is a convention, enforced by a grep gate rather than the compiler. The unswept moves are converted at their source (`research.md` has the list and the likeliest culprit).
  - Capsule resizes (crouch, slide entry, stand-up and the swim seam) must pass the overlap test before they commit. Otherwise the body keeps its current size, as Quake and Source refuse a stand-up with no headroom.
  - Teleport arrival keeps today's floor classification. Only the ground-stick drop changes.
- **The check reports; it never reverts.**
  - Tests assert it on every movement tick. It is armed through the physics `test-support` feature, which dependent crates already enable, so agent, teleport and replay tests run armed.
  - Outside tests it compiles into every build (`development_guide.md` §6.4). It is on by default in debug-assertion builds and off by default in release, and a `POSTRETRO_*` variable turns it on in any build.
  - When it fires, it logs one warning per level load, shared by all bodies. The warning names the body, the position and the surface it intersected, so a modder can find the bad spot.
  - It stays off for a tick that starts intersecting, and resumes on the first tick that starts clear.
  - A revert would re-fire every tick under held input and freeze the body, a stall in place of an embed. The skin cast's guarantee has exceptions (`entity_model.md` §7), and modder geometry has hairline gaps (`drafts/geometry-integrity-validation`), so the warning is the net for both.
- **Host and client run identical code.** Pose and motion stay pure functions of input and geometry, so prediction, replay and host simulation stay deterministic. No wire or tuning change.
- **Players and enemies share geometry primitives, not controllers.** Swept step-up and swept snap-to-floor become pure functions that both the player substrate and the agent controller call. The agent's copies are deleted.
  - Each controller keeps its own policy: when to call a primitive, and with which tuning (step height, lift margin, walkable slope). Player feel stays in player code.
  - Controllers stay separate (`movement.md` §6, §7). An enemy can later move to a cheaper mode by not calling them.
- **Placement.** The movement substrate in `postretro-physics`, plus the agent controller in `postretro-sim`. This is neither co-op code nor vault code: the vault moves only by swept velocity, and its ledge probe already proves the full capsule (`research.md`).
- **Non-goals.**
  - Rescuing a body that starts a tick embedded, by nudge or by static depenetration. Own movement can no longer create one. A mover lifting a rider into a static ceiling under the default `displace` policy still can, and nothing frees it, because only `crush` handles a pinned rider (`movement.md` §6). That softlock is the next brief, `movement--mover-pinned-rider`, queued as a task.
  - Repairing gappy compiled geometry: `drafts/geometry-integrity-validation` owns it at bake time.
  - Clamping the vault exit pose: the probe already proves the vault's volume.

## Acceptance
### Automated
**The invariant holds** (the test-build assertion is armed in every movement and agent test)
- [ ] Reproduces first: at b0a5c0f, a fixture pressing a pawn along a convex lip edge with a tilted walkable contact ends a tick overlapping the lip. It is committed as an expected failure naming the bug, and the fix makes it a plain passing test.
- [ ] Every per-bypass fixture below ends a tick overlapping static geometry at b0a5c0f. A bypass that no fixture can drive into overlap is named in its test with the reason, and the swept-move grep gate covers it.
- [ ] Vault fixtures never end a tick overlapping static geometry: on the path, aborted mid-path, oblique entry, held then neutral forward input after exit, and a lip whose top is flat and one tilted to exactly the walkable limit.
- [ ] A grounded pawn with step height zero and its head within one skin width of a ceiling ends clear after the floor push. With a positive step height it ends clear after ground-stick. Pin P7.
- [ ] Walking into a step whose top has less headroom than the standing capsule needs leaves the pawn, or agent, below the step and clear.
- [ ] Ground-stick beside a wall, over a drop taller than one step height but within ground-stick reach, ends clear, for a pawn and for an agent. At b0a5c0f the same fixture sinks into the ledge.
- [ ] A corner wedge on the steepest walkable ramp, whose blocked tick slid downhill, ends clear after the stuck-stop rollback.
- [ ] Two bypass conditions on one tick (free advance, then ground-stick) end clear. Pin P5.
- [ ] A step-up followed by a floor push on one tick, under an overhang, ends clear. Pin P4.
- [ ] A pawn standing still with its capsule over a ledge edge ends clear after ground-stick. Pin P7.
- [ ] An idle agent with no destination, settling with its capsule over a ledge edge, ends clear. Pin P6.
- [ ] A pawn riding a moving platform that walks into a convex lip on the same tick ends clear. The carry is exempt; the movement after it is not. Pin P2.
- [ ] A pawn pushed out of a mover toward a static wall does not trip the assertion: the push happens before the tick's checked start. Pin P1.
- [ ] After a mover-caused embed, the first tick that starts clear is checked again: a forced overlap on that tick is caught. Pin P8.
- [ ] With the check on and the test assertion off, an overlapping end pose handed to the check through a test-only hook logs one warning naming the body, position and surface, and moves nothing. A second overlap in the same level load is silent; after a level reload the next one logs.

**Legitimate motion still moves** (not over-tight)
- [ ] Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and running across a coplanar seam keep the displacement recorded at b0a5c0f, within 1 mm per tick. The baseline is a committed test that runs before the fix. A seam slide that loses free-advance distance is noted in the plan of record.
- [ ] A step whose top has headroom for the standing capsule, but not for the step-up lift margin, still steps up, for a pawn and for an agent.
- [ ] With step height zero, neither a pawn nor an agent steps up or snaps to the floor, as today. Pin P7.
- [ ] A teleport arrival over walkable floor, with a wall pressed against the capsule, still lands grounded, as today.
- [ ] A pawn steps up onto, and snaps to, a mover's top. An agent beside the same mover still ignores it, as today.
- [ ] Each controller keeps its own call rules. A falling agent beside a step still steps up, and an airborne pawn still does not. A pawn tuned to a non-default walkable slope still classifies floors by its own slope.
- [ ] A pawn carried by a mover into a static ceiling behaves as before. The assertion does not fire on mover-caused overlap.
- [ ] Existing movement, vault, slide, teleport and agent-navigation suites pass with the assertion armed.

**Determinism**
- [ ] Replaying a recorded command sequence through the client predictor reproduces the host's poses exactly. The sequence includes a vault across a convex lip with held then neutral input, a step-up, and a ground-stick beside a wall.

**Gates**
- [ ] Grep gate: outside the swept move, the movement substrate and the agent controller write no body position, except the exemptions Decisions names.
- [ ] A stand-up or slide exit under a ceiling too low for the standing capsule leaves the body crouched and clear.
- [ ] An airborne stand-up just above a floor, where the head-anchored resize would drop the feet into it, leaves the body crouched and clear. At b0a5c0f the same fixture ends intersecting the floor. Pin P11.
- [ ] Grep gate: the agent controller holds no step-up or snap-to-floor logic of its own. It reaches them only through the shared primitives.
- [ ] The check is off by default in release and on by default in debug-assertion builds, and its `POSTRETRO_*` variable turns it on in a release build.
- [ ] Diff gate: the change adds no wire field and no movement-descriptor field.

### Manual
- [ ] Single player: vault the campaign-test ledges 20× at varied angles and speeds, and walk ramps and stairs. No embed and no felt hitch. Enemies chasing over the same ledges and stairs don't clip in.
- [ ] Co-op client at 45/60/5: the same course, with no embed after any correction.
- [ ] Seam feel: run and slide across coplanar seams on `movement-feel`, before and after the change. No felt speed loss.
- [ ] Release build with the check's variable set, on `campaign-test`: any warning names a position a modder can find in the map.

## Path
- Unswept moves in movement/substrate.rs: the floor branch's free advance after the floor-push budget is spent, the floor push, the stuck-stop rollback (it keeps the new Y), `step_up_lift`'s rise, and the ground-stick drop through the `floor_below` centre-ray fallback. Convert the fallback at the ground-stick call site, not inside `floor_below`, because teleport arrival calls `floor_below` for grounding only. Wall-contact nudge: exempt. Resize writes: intents/normal.rs, intents/slide.rs, dispatch.rs, swim transition.
- Agent copies to delete: `step_up_lift` and `ground_stick` in crates/sim/src/agent/mod.rs, beside `collide_and_slide`. The agent queries static-only `CollisionWorld` and the player queries `CombinedCollisionWorld`, so the primitives take the query as a parameter. `postretro-sim` already depends on `postretro-physics`.
- Overlap check: the static half of `capsule_fits_combined` (exact intersection, teleport arrival's check), with the 0.5 mm tolerance added. `CombinedCollisionWorld::static_only` wraps the agent's `CollisionWorld`, and a movement collision-source trait already spans both. Substrate helpers take parry's capsule type, and `postretro-sim` has no parry dependency, so the shared primitives take engine capsule params.
- Primitive inputs: collision query, capsule, step height, lift margin, walkable slope. Lift margins match today (0.05 m), and walkable slope differs. Each controller passes its own values.
- Mover carry and mover displacement run at the top of `integrate_collision`. Take the check's start sample after both. Teleport relocation runs after the substrate, so the end sample precedes it. Agent call sites: idle settle and steering in agent_steering.rs.
- Existing coverage to extend: vault_tests.rs (its lip and headroom distance check covers only the nominal path), and the movement module's mover "without persistent overlap" tests.
- Chosen shape: convert every unswept move at its source behind one swept helper, and assert in tests. Rival weighed: keep the source fixes and add a release-build revert. Rejected because a revert re-fires under held input and freezes the body. The guard-alone rival is weaker than both. For enemies, fixing each copy in place was rejected, because copying is how the defect reached them; sharing the whole controller was rejected, because it couples player feel to AI movement and per-tick cost.
- First slice: the failing lip-edge fixture. It confirms the free advance can embed before any fix lands.
- substrate.rs is past 800 lines. Introduce the swept helper in a sibling module first, in its own behaviour-preserving commit.
- At promotion: add the tick-level invariant to `entity_model.md` §7 beside the skin-cast contract. Update `movement.md` §4 Vault ("collision has no overlap query"), note in §6 and §7 that enemies share the primitives and the invariant, and list the check's `POSTRETRO_*` variable beside the other diagnostics.

## Open questions
- The check's variable name, and whether the warning names the surface by face index or by brush. — **delegated**.
