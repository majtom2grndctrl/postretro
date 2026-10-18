# movement--static-overlap-invariant

Brief · compact · reads: `context/lib/entity_model.md` §7 (Collision), `context/lib/movement.md` §4 (Vault), §7 · read at b0a5c0f

## Problem
Owner co-op playtest under the dev lag proxy: after a vault, the player's pawn sank into the brush it had vaulted over and stayed embedded until the player walked out. Basis: observed defect. The lossy link triggered it: the host ran held, then neutral, input against the lip (`coop--lossy-link-delivery`). The defect itself lives in the movement code and can occur in any role. Cause: collision integration moves the capsule in places that bypass the swept cast, which can carry it into static geometry, and nothing ever pushes it back out. The enemy controller copies two of those moves. When done, a body that starts a tick clear of static geometry cannot end that tick overlapping it, whether it is a player pawn or an enemy, on any peer.

## Decisions
- **New invariant: movement never ends a tick inside static geometry if it started clear.** It covers player pawns and enemy agents. It extends the skin-cast contract in `entity_model.md` §7 from a single cast to a whole tick, using the same 0.5 mm tolerance. Mover carry and mover displacement are exempt (see Non-goals).
- **One swept move helper carries every position change.** A body moves only through the swept skin cast, so an unswept write cannot be added by accident. Today's unswept moves are converted at their source:
  - in `integrate_collision`: the second-TOI-0 free advance, the per-tick floor push, and the stuck-stop rollback (it restores the start X/Z but keeps the new Y);
  - `step_up_lift`'s rise;
  - the centre-ray drop in `floor_below`;
  - in the agent controller (`crates/sim/src/agent/mod.rs`): its near-verbatim copies of `step_up_lift` and `ground_stick`.

  `research.md` ranks the free advance at a convex lip edge as the likeliest cause of the observed embed.
- **The guard checks and reports; it never reverts.** It uses the static half of `capsule_fits_combined` with the skin tolerance.
  - Test builds assert on every movement tick.
  - Builds with debug assertions, including the dev profile the owner playtested on, log one warning per session.
  - Release builds carry no check.

  A revert would re-fire every tick under held input and freeze the body, a stall in place of an embed. The swept helper is a convention the compiler does not enforce, and the skin cast's guarantee has exceptions (`entity_model.md` §7: non-capsule pairs, degenerate triangles, a core piercing a triangle). The dev warning is the net for those.
- **Host and client run identical code.** Pose and motion stay pure functions of input and geometry, so prediction, replay and host simulation stay deterministic. No wire or tuning change.
- **Players and enemies share geometry primitives, not controllers.** Swept step-up and swept snap-to-floor become pure functions of collision query, capsule, step height and walkable slope. The player substrate and the agent controller both call them, and the agent's copies are deleted.
  - Each controller keeps its own policy: when to call a primitive, and with what tuning. Player feel stays in player code.
  - An enemy can later move to a cheaper mode, such as navmesh projection, by not calling them.
  - One step-up keeps the navmesh bake's step height and the agents' real step reach as one contract, not two copies that drift.
  - The separate-controller commitment holds (`movement.md` §7; agent grounding stays its own).
- **Placement.** The movement substrate in `postretro-physics`, plus the agent controller in `postretro-sim`. This is not co-op code and not vault code: the vault moves only by swept velocity, and its ledge probe already proves the full capsule (`research.md`).
- **Non-goals.**
  - Rescuing a body already embedded at tick start, by Quake-style nudge or static depenetration. Own movement can no longer create one. Mover carry under the `displace` policy still can, by lifting a rider into a static ceiling. Under the default `displace` policy nothing resolves that: movers never yield, and only `crush` handles a pinned rider (`movement.md` §6). It is an unowned leftover. Raise a follow-up ticket for it, not this brief. The guard stays off for a tick that starts embedded.
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
- [ ] In a debug-assertion build, a forced overlap logs exactly one warning per session and moves nothing.

**Legitimate motion still moves** (not over-tight)
- [ ] Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and running across a coplanar seam keep their current displacement within tolerance.
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
- [ ] Grep gate: outside the swept move helper, the movement substrate and the agent controller write no body position, except the exemptions Decisions names.
- [ ] Grep gate: the agent controller holds no step-up or snap-to-floor logic of its own. It reaches them only through the shared primitives.
- [ ] Grep gate: the tick-end overlap check, its start sample and its warning all sit behind the debug-assertions gate.
- [ ] Diff gate: the change adds no wire field and no movement-descriptor field.

### Manual
- [ ] Single player: vault the campaign-test ledges 20× at varied angles and speeds, and walk ramps and stairs. No embed and no felt hitch. Enemies chasing over the same ledges and stairs don't clip in.
- [ ] Co-op client at 45/60/5: the same course, with no embed after any correction.

## Path
- Unswept moves in movement/substrate.rs: the floor branch's free advance after the floor-push budget is spent, the floor push itself, the stuck-stop rollback, `step_up_lift`, and the `floor_below` centre-ray fallback.
- Agent copies to delete: `step_up_lift` and `ground_stick` in crates/sim/src/agent/mod.rs, beside `collide_and_slide`. The agent queries static-only `CollisionWorld` and the player queries `CombinedCollisionWorld`, so the primitives take the query as a parameter. `postretro-sim` already depends on `postretro-physics`.
- Overlap check: `capsule_fits_combined`, whose caller is teleport arrival in movement/teleport.rs. Add a static-only variant with the skin tolerance. The agent capsule needs the same check against `CollisionWorld`.
- Mover carry runs at the top of `integrate_collision` via `apply_mover_carry`. The assertion's start pose is taken after it.
- Existing coverage to extend: vault_tests.rs (its lip and headroom distance check covers only the nominal path), and the movement module's mover "without persistent overlap" tests.
- Chosen shape: convert every unswept move at its source behind one swept helper, and assert in tests. Rival weighed: keep the source fixes and add a release-build revert. Rejected because a revert re-fires under held input and freezes the body. The guard-alone rival is weaker than both. For enemies, fixing each copy in place was rejected, because copying is how the defect reached them; sharing the whole controller was rejected, because it couples player feel to AI movement and per-tick cost.
- First slice: the failing lip-edge fixture. It confirms the free advance can embed before any fix lands.
- substrate.rs is past 800 lines. Introduce the swept helper in a sibling module first, in its own behaviour-preserving commit.
- At promotion: add the tick-level invariant to `entity_model.md` §7 beside the skin-cast contract. Update `movement.md` §4 Vault ("collision has no overlap query"), and note in §7 that enemies share the invariant.

## Open questions
- None.
