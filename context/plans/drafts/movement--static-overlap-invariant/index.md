# movement--static-overlap-invariant

Brief · compact · reads: `context/lib/entity_model.md` §7 (Collision), `context/lib/movement.md` §4 (Vault) · read at b0a5c0f

## Problem
Owner co-op playtest under the dev lag proxy: after a vault, the player's pawn sank into the brush it had vaulted over and stayed embedded until the player walked out. Basis: observed defect. The lossy link triggered it (the host ran held, then neutral, input against the lip; see `coop--lossy-link-delivery`), but the defect lives in the movement substrate and can occur in any role. Cause: collision integration has moves that bypass the swept cast and can carry the capsule into static geometry, and nothing restores a clear pose afterwards. A pawn that starts a tick clear of static geometry should never end it overlapping static geometry, on any peer, in any role.

## Decisions
- **New invariant: a pawn's own movement never ends a tick overlapping static geometry if it started clear.** The guard measures from the pose after mover carry. Carry and mover displacement belong to mover block and crush policy (`networking.md` §Current contract), so an overlap they cause is exempt. The invariant extends the skin-cast contract in `entity_model.md` §7 from one cast to a whole tick.
- **Close the known bypasses at their source.** The cases:
  - The second-TOI-0 free advance in `integrate_collision`, which moves `velocity * remaining_dt` with no sweep, becomes a swept move or stops without moving.
  - The `step_up_lift` rise is swept before it commits.
  - The ground-stick ray fallback in `floor_below` drops by a swept cast, never by ray distance.

  The research ranks the free advance at a convex lip edge as the likeliest cause of the observed embed (`research.md`).
- **A tick-end guard backs the invariant.** If the start pose was clear and the end pose is not, the tick reverts to the last clear pose. In test builds the guard fails the test instead. In release it reverts and warns once per session, so a missed bypass costs one stalled tick, never an embed. The overlap test is the static half of `capsule_fits_combined`: the exact trimesh intersection plus the core-in-solid-cell check. Teleport arrival already relies on it. Cost: one exact intersection test per moving pawn per tick.
- **Host and client run the same guard.** It is a pure function of pose and static geometry, so prediction, replay and host simulation stay deterministic. No wire or tuning change.
- **Placement.** The movement substrate in `postretro-physics`, engine-wide. Not co-op code, not vault code: the vault moves only by swept velocity, and its ledge probe already proves the full capsule (`research.md`).
- **Non-goals.**
  - Rescuing a pose that is already embedded at tick start, Quake-style nudge or static depenetration. Nothing creates one once the invariant holds. Reopen if the guard's warning shows up in playtests.
  - Mover-caused overlap: mover block and crush policy owns it.
  - Clamping the vault exit pose: the probe already proves the vault's volume.

## Acceptance
### Automated
**Invariant holds** (test-build guard armed for every movement test)
- [ ] Reproduces first: at b0a5c0f a fixture pressing a pawn along a convex lip edge with a tilted walkable contact ends a tick overlapping the lip. The test is committed failing, before the fix.
- [ ] Vault fixtures never end a tick overlapping static geometry: on-path, aborted mid-path, oblique entry, held forward input after exit, a lip at each walkable-normal extreme.
- [ ] Walking into a low overhang with step-up available ends clear.
- [ ] A pawn pressed against a wall above a ledge between one and two step heights high ends clear when ground-stick runs.
- [ ] Two bypass conditions on one tick (free advance then ground-stick) end clear.

**Legitimate motion still moves** (the guard is not over-tight)
- [ ] Ramps, stairs at step height, step-up onto a ledge, sliding along a wall and running across a coplanar seam keep their current displacement within tolerance, and the guard never fires.
- [ ] A pawn carried by a mover into a static ceiling is not reverted by the guard. Mover policy handles it.
- [ ] Existing movement, vault, slide and teleport suites pass with the guard armed.

**Determinism**
- [ ] Replaying a recorded command sequence through the client predictor reproduces the host's poses exactly, with the guard active on both.

### Manual
- [ ] Single-player: vault the campaign-test ledges 20× from varied angles and speeds, and walk ramps and stairs. No embed and no felt hitch.
- [ ] Co-op client at 45/60/5: the same course, with no embed after any correction.

## Path
- Bypasses: `integrate_collision` (floor branch, the free advance after the per-tick floor push budget runs out), `step_up_lift`, `floor_below` (centre-ray fallback). The floor push of one skin plus nudge is negligible and stays.
- Overlap test: `capsule_fits_combined`, whose teleport-arrival caller is in movement/teleport.rs. Split out a static-only variant rather than passing an empty mover set if that reads cleaner.
- Mover carry runs at the top of `integrate_collision` via `apply_mover_carry`. Take the guard's start pose after it.
- Existing coverage to extend: vault_tests.rs (the lip and headroom distance check covers only the nominal path), the movement module's mover "without persistent overlap" tests.
- Chosen shape: fix each bypass at its source, plus a guard that should never fire. Strongest rival: the guard alone, without fixing the sources. Rejected: every revert costs a stalled tick and hides which bypass fired.
- First slice: the failing lip-edge fixture. It confirms the free advance can embed before any fix lands.
- substrate.rs is past 800 lines. If the guard or a swept replacement adds substantially, move the bypass helpers into a sibling module first, in a behaviour-preserving commit.
- At promotion: add the tick-level invariant to `entity_model.md` §7 beside the skin-cast contract.

## Open questions
- Which pose the guard reverts to: the tick start after carry, or the pose before the last unswept step. — **delegated**: prefer the later one if it is tracked cheaply, and report the choice.
