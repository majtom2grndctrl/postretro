# coop--spawn-placement-occupancy

Brief · compact · reads: `context/lib/networking.md` §Session-state ledger, §Slot lifecycle; `plans/done/E15--seat-session-identity-roster` AC-SEAT-1, AC-REJOIN-1 · read at 05ab2d3da (`feat/preferences-comfort-floor`)

Sequenced before `coop--client-spawn-view-handoff`. That brief's manual proof cannot pass for the first client until this lands.

## Problem
Found by review while drafting `coop--client-spawn-view-handoff`. Basis: a defect found by reading source, not by running it. It is latent on current content because every map under `content/` has at most one `player_spawn`. Cause: the host's install spawn creates a movement pawn at every `player_spawn`, and placement capture records each one as a live occupant, so on an N-placement map every index is occupied before any remote seat is assigned. The first joining client therefore falls back to the cursor, which is index 0 (the host's own spawn) on a fresh session. E15 AC-SEAT-1 holds only to the letter: the occupants are unseated pawns that no player controls. When this is done, a joining seat's pawn materializes at a placement that no other player's pawn occupies whenever one is free, and it never lands on top of an unseated pawn.

## Decisions
- **Occupancy means players.** A placement is occupied only by a live pawn bound to a seat, or reserved by a held seat. Promotion assigns the first free placement, scanning from the cursor. The reuse-and-log fallback fires only when seats alone fill every placement. This follows E15 AC-SEAT-1 and its "Live pawns outnumber placements" edge, which counts seats promoting in. It diverges from the premise on `SeatTable::assign_placement` that unseated install pawns count as occupants. Counting them makes the free case unreachable on any map with more than one spawn.
- **No shared placement with an unseated pawn.** A joiner's pawn never materializes at a placement where an unseated movement pawn stands. The fate of those pawns is the one blocking question below.
- **E15 lifecycle unchanged.**
  - A seat keeps its placement within a level.
  - A held seat reserves its placement until release.
  - Level unload clears every assignment.
  - The cursor survives level changes.
- **Layer placement.**
  - Occupancy policy stays in the seat table: host-only, in the binary's netcode layer.
  - The descriptor spawn layer stays seat-blind. Its contract is that "the caller owns seat lookup".
  - Whatever decides which install placements get a pawn lives in the engine's install caller, not in `data_archetype`.
  - Connected clients are untouched, since they already suppress the boot pawn.
- **Non-goals.**
  - Overflow policy. When there are more seats than placements, reuse-and-log stays.
  - Resolving overlap in the overflow case.
  - Sending the spawn pose to the client. `coop--client-spawn-view-handoff` owns that.
  - Adding placements to `level_content_digest`.
  - Team spawns, per-seat spawn preference, or spawn selection by distance or line of sight.

## Acceptance
### Automated
Free placement first:
- [ ] Two-placement map, host plus one client, fresh session: the client's pawn materializes at the placement the host's pawn does not occupy. Today it gets the host's placement and logs the all-occupied warning.
- [ ] One-placement map, host plus one client: the client reuses the host's placement and the collision warning logs. The overflow path is unchanged.
- [ ] Three-placement map, host plus two clients: the clients get two distinct placements, and neither is the host's.
- [ ] More seats than placements: three clients promote into a two-placement map with the host. The first client gets the free placement. Each later client reuses an index and logs the collision, and no client is refused a pawn.
- [ ] After a client's pawn materializes, no other movement pawn stands at that placement's authored origin.

Lifecycle:
- [ ] A client disconnects and reclaims its seat within the hold window, in the same level: it returns to its previous placement. A client that joins during the hold gets a different free placement, not the held one.
- [ ] After the hold expires and the seat is released, the next joiner can receive the released placement. Before expiry it cannot.
- [ ] Host restarts the same level with one client connected: after re-promotion the host's pawn and the client's pawn occupy distinct placements.
- [ ] A client assigned in the same frame as the host's install never receives the host's placement, whichever of the two is recorded first. This is the ordering `coop--client-spawn-view-handoff` introduces.

Regression guard:
- [ ] Single-player on a multi-spawn map: one controllable pawn at the first movement placement, and the camera starts at the first spawn.

### Manual
- [ ] Host a dev map with two `player_spawn` placements and connect one client. The client spawns at the second placement, not inside or beside the host at the first.

## Path
- **Seams:**
  - `spawn_from_player_starts_with_carried_loadout` spawns at every start and marks the first movement pawn local.
  - `capture_player_spawn_placements` (in `main.rs`) binds every movement pawn found at an authored origin.
  - `SeatTable::bind_level_spawn_placement` and `SeatTable::bind_pawn` hand placement 0 to seat 0.
  - `SeatTable::occupied_live_placements` unions the seat-bound occupants with the level-spawn occupants.
  - `SeatTable::assign_placement` runs the scan and the fallback.
  - The `Participating` arm of the host net frame drives assignment.
  - `host_handle_accept_descriptor_at_placement` always spawns a fresh pawn.
- **Shape (recommended, pending the owner):** the install materializes a movement pawn for the local seat only. Other movement placements stay empty until a seat claims one. `level_spawn_placements` may then reduce to seat 0's provenance. Whether it survives is the executor's call.
- **Rival:** drop unseated install pawns from occupancy and leave spawning as it is. Rejected, because the joiner then materializes inside an inert pawn.
- **First slice:** a failing seat-level test. It runs the install spawn over two placements, then capture, then binds seat 0, then admits a remote seat, then computes occupancy, then assigns. It asserts index 1. Today it returns 0 with "every player_spawn placement is occupied".
  - The test needs `capture_player_spawn_placements` reachable outside `main.rs`. Move it beside the seat table first, behavior-preserving, in its own commit.
  - Precedents: `seat/roster_harness_test.rs` and `boot_spawn_gate_test.rs`.
- **Existing test:** `carried_health_restores_only_the_first_local_player_start` asserts that a second pawn exists at the second start. It changes meaning under the recommended shape, so update it in the same pass.
- **Files past ~800 lines:**
  - `main.rs`: do not extend it.
  - `seat.rs`: production code is under the limit, so put new tests in a sibling test module.
  - `data_archetype.rs` and `startup/lifecycle.rs`.
- The trace and the side effects are in `research.md`.

## Open questions
- What happens to the unseated movement pawns the install creates at the extra `player_spawn` placements? Options:
  - **(A)** Do not materialize them. Non-movement `entity_class` placements keep spawning. Recommended: it matches Quake's co-op starts, and it removes inert bodies that enemy targeting can pick today (see `research.md`).
  - **(C)** The first joiner at that placement adopts the pawn. This keeps the single-player world unchanged, but adoption has to register replication, ownership and carried loadout on an entity that already exists.

  Owner: Dan. **Blocks build.**
- Add a two-spawn map under `content/dev/maps/` for the manual row, or reuse a temporary map. **Delegated.**
- Where the moved capture helper lands. **Delegated.**
