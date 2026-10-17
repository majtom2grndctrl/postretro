# movement--static-overlap-invariant

Brief · compact · reads: `context/lib/entity_model.md` §7 (Collision), `context/lib/movement.md` §4 (Vault), §7 · read at b0a5c0f

## Problem
Owner co-op playtest under the dev lag proxy: after a vault, the player's pawn sank into the brush it had vaulted over and stayed embedded until the player walked out. Basis: observed defect. The lossy link triggered it: the host ran held, then neutral, input against the lip (`coop--lossy-link-delivery`). The defect itself lives in the movement code and can occur in any role. Cause: collision integration moves the capsule in places that bypass the swept cast, which can carry it into static geometry, and nothing ever pushes it back out. The enemy controller copies two of those moves. When done, a body that starts a tick clear of static geometry cannot end that tick overlapping it, whether it is a player pawn or an enemy, on any peer.

## Decisions
- **New invariant: movement never ends a tick inside static geometry if it started clear.** It covers player pawns and enemy agents. It extends the skin-cast contract in `entity_model.md` §7 from a single cast to a whole tick, using the same 0.5 mm tolerance. Mover carry and mover displacement are exempt; mover block and crush policy owns those overlaps (`networking.md` §Current contract).
- **One swept move helper carries every position change.** A body moves only through the swept skin cast, so an unswept write cannot be added by accident. Today's unswept moves are converted at their source:
  - in `integrate_collision`: the second-TOI-0 free advance, the per-tick floor push, and the stuck-stop rollback (it restores the start X/Z but keeps the new Y);
  - `step_up_lift`'s rise;
  - the centre-ray drop in `floor_below`;
  - in the agent controller (`crates/sim/src/agent/mod.rs`), its `step_up_lift` and `ground_stick` copies.

  `research.md` ranks the free advance at a convex lip edge as the likeliest cause of the observed embed.
- **The guard is a test-build assertion, not a release revert.** In test builds, every movement tick asserts that a clear start ends clear, using the static half of `capsule_fits_combined` with the skin tolerance. Release builds carry no guard. A release revert would re-fire every tick under held input and freeze the body: a stall in place of an embed. Since every position change goes through the swept helper, release builds do not need one.
- **Host and client run identical code.** Pose and motion stay pure functions of input and geometry, so prediction, replay and host simulation stay deterministic. No wire or tuning change.
- **Placement.** The movement substrate in `postretro-physics`, plus the agent controller in `postretro-sim`. This is not co-op code and not vault code: the vault moves only by swept velocity, and its ledge probe already proves the full capsule (`research.md`).
- **Non-goals.**
  - Rescuing a body already embedded at tick start, by Quake-style nudge or static depenetration. Own movement can no longer create one. Mover carry under the `displace` policy still can, by lifting a rider into a static ceiling. That is mover policy's case, and the guard stays off for a tick that starts embedded.
  - Clamping the vault exit pose: the probe already proves the vault's volume.

## Acceptance
### Automated
**The invariant holds** (the test-build assertion is armed in every movement and agent test)
- [ ] Reproduces first: at b0a5c0f, a fixture pressing a pawn along a convex lip edge with a tilted walkable contact ends a tick overlapping the lip. The test is committed failing, before the fix.
- [ ] Vault fixtures never end a tick overlapping static geometry: on the path, aborted mid-path, oblique entry, held forward input after exit, and a lip at each walkable-normal extreme.
- [ ] A grounded pawn with its head within one skin width of a ceiling ends clear after the floor push.
- [ ] Walking into a low overhang with step-up available ends clear, for a pawn and for an agent.
- [ ] Ground-stick beside a wall above a ledge between one and two step heights high ends clear, for a pawn and for an agent.
- [ ] A stuck-stop corner wedge on a ramp ends clear.
- [ ] Two bypass conditions on one tick (free advance, then ground-stick) end clear.

**Legitimate motion still moves** (not over-tight)
- [ ] Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and running across a coplanar seam keep their current displacement within tolerance.
- [ ] A pawn carried by a mover into a static ceiling behaves as before. The assertion does not fire on mover-caused overlap.
- [ ] Existing movement, vault, slide, teleport and agent-navigation suites pass with the assertion armed.

**Determinism**
- [ ] Replaying a recorded command sequence through the client predictor reproduces the host's poses exactly.

### Manual
- [ ] Single player: vault the campaign-test ledges 20× at varied angles and speeds, and walk ramps and stairs. No embed and no felt hitch. Enemies chasing over the same ledges and stairs don't clip in.
- [ ] Co-op client at 45/60/5: the same course, with no embed after any correction.

## Path
- Unswept moves in movement/substrate.rs: the floor branch's free advance after the floor-push budget is spent, the floor push itself, the stuck-stop rollback, `step_up_lift`, and the `floor_below` centre-ray fallback.
- Agent copies: `step_up_lift` and `ground_stick` in crates/sim/src/agent/mod.rs, beside `collide_and_slide`.
- Overlap check: `capsule_fits_combined`, whose caller is teleport arrival in movement/teleport.rs. Add a static-only variant with the skin tolerance. The agent capsule needs the same check against `CollisionWorld`.
- Mover carry runs at the top of `integrate_collision` via `apply_mover_carry`. The assertion's start pose is taken after it.
- Existing coverage to extend: vault_tests.rs (its lip and headroom distance check covers only the nominal path), and the movement module's mover "without persistent overlap" tests.
- Chosen shape: convert every unswept move at its source behind one swept helper, and assert in tests. Rival weighed: keep the source fixes and add a release-build revert. Rejected because a revert re-fires under held input and freezes the body. The guard-alone rival is weaker than both.
- First slice: the failing lip-edge fixture. It confirms the free advance can embed before any fix lands.
- substrate.rs is past 800 lines. Introduce the swept helper in a sibling module first, in its own behaviour-preserving commit.
- At promotion: add the tick-level invariant to `entity_model.md` §7 beside the skin-cast contract. Update `movement.md` §4 Vault ("collision has no overlap query"), and note in §7 that enemies share the invariant.

## Open questions
- None.
