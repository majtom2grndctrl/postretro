# coop--spawn-placement-occupancy

Brief · compact · reads: `context/lib/networking.md` §Slot lifecycle, §Session-state ledger, §Role model; `plans/done/E15--seat-session-identity-roster` AC-SEAT-1, AC-REJOIN-1, Task 6; `plans/done/M7--player-spawn` §Acceptance criteria · read at 3f070e32d (`feat/preferences-comfort-floor`)

**Sequence: 2 of 4.** Depends on `E23--preferences-comfort-floor` landing: that branch splits `main.rs` and `startup/lifecycle.rs`, where this brief's install and host-net seams live. Review it against `main` after that merge. No dependency on `animated-lightmap-compact-atlas` (1 of 4). `coop--client-spawn-view-handoff` (4 of 4) depends on this one: its manual proof cannot pass for the first client until this lands.

## Problem
Found by review while drafting `coop--client-spawn-view-handoff`. Basis: a defect found by reading source, not by running it. It is latent on current content because every map under `content/` has at most one `player_spawn`. Cause: the host's install spawn creates a movement pawn at every `player_spawn`, and placement capture records each one as a live occupant, so on an N-placement map every index is occupied before any remote seat is assigned. The first joining client therefore falls back to the cursor, which is index 0 (the host's own spawn) on a fresh session. E15 AC-SEAT-1 holds only to the letter: the occupants are unseated pawns that no player controls. When this is done, the install creates a movement pawn for the local player only, and a joining seat's pawn materializes at a movement placement no other player's pawn occupies whenever one is free.

## Decisions
- **Install materializes seat 0's movement pawn only.** Every other movement placement stays empty until a seat claims it, as Quake's co-op starts do. This knowingly diverges from M7's acceptance (one entity per spawn entry) for movement placements. M7's goal names co-op as the reason for multiple spawns, and co-op needs free starts, not pre-placed bodies.
- **Occupancy means players.** A placement is occupied by a live pawn bound to a seat, or reserved by a held seat. With no unseated movement pawns, pawn occupancy equals seat occupancy. So E15 Task 6's rule, "occupancy is measured in pawns", stands unchanged; the reason it gives, one pawn per `player_spawn`, no longer holds. Promotion assigns the first free placement, scanning from the cursor. Reuse-and-log fires only when seats fill every placement, per E15's "Live pawns outnumber placements" edge.
- **Seat 0 is assigned through the seat table.** Seat 0 takes its placement from `SeatTable::assign_placement`, the chokepoint remote seats use, pinned to the first movement placement, before the install spawns anything. The install spawns the movement pawn at that placement only. The origin-equality capture (`capture_player_spawn_placements`, `level_spawn_placements`) is deleted: placement is assigned, never inferred from transforms. Seat 0's placement is therefore recorded before any same-frame remote assignment can run.
- **Only movement placements are assignable.** A movement placement's `entity_class` resolves to a descriptor that carries movement. Seats are assigned only from these, the overflow fallback included, because `host_handle_accept_descriptor_at_placement` spawns the placement's own `entity_class`. Non-movement placements still spawn their entity at install, per M7. A map with placements but none of them movement takes the no-placement branch (`host_handle_accept`), as a map with no `player_spawn` does.
- **Forecloses:** `player_spawn` can no longer pre-place an extra movement-bearing body in single-player. No content uses it: no map sets `entity_class`, and none has a second `player_spawn`. Undo cost: restore the spawn loop and a provenance record beside it.
- **E15 lifecycle unchanged.**
  - A seat keeps its placement within a level.
  - A held seat reserves its placement until release.
  - Level unload clears every assignment, seat 0's included. Every install re-assigns seat 0.
  - The cursor survives level changes.
- **Layer placement.**
  - Occupancy and the seat-0 pin live in the seat table, in `postretro-netcode`, host-only.
  - The descriptor spawn layer stays seat-blind. It receives the resolved placement index, just as it receives the carried record, because "the caller owns seat lookup".
  - The install rule does not branch on role. Single-player and host install identically (`networking.md` §Role model: nothing branches on player count).
  - Installs without a seat table apply the same pin rule: the observe driver and the fixtures. Connected clients are untouched, since they already suppress the boot pawn.
- **Non-goals.**
  - Overflow policy. When there are more seats than movement placements, reuse-and-log stays.
  - Resolving overlap in the overflow case.
  - Sending the spawn pose to the client. `coop--client-spawn-view-handoff` owns that.
  - Adding placements to `level_content_digest`.
  - Team spawns, per-seat spawn preference, or spawn selection by distance or line of sight.

## Acceptance
### Automated
Install:
- [ ] Single-player and host, on a map with three movement placements: exactly one movement pawn exists after install. It is at the first movement placement, it is the local pawn, and the camera starts at the first spawn.
- [ ] A map where a non-movement placement precedes two movement placements: the non-movement entity spawns at install, and the one movement pawn stands at the first movement placement.

Free placement first:
- [ ] Two-placement map, host plus one client, fresh session: the client's pawn materializes at the placement the host's pawn does not occupy. Today it gets the host's placement and logs the all-occupied warning.
- [ ] One-placement map, host plus one client: the client reuses the host's placement and the collision warning logs. The overflow path is unchanged.
- [ ] Three-placement map, host plus two clients: the clients get two distinct placements, and neither is the host's.
- [ ] More seats than placements: three clients promote into a two-placement map with the host. The first client gets the free placement. Each later client reuses an index and logs the collision, and no client is refused a pawn.

Movement placements only:
- [ ] A map with one non-movement placement and two movement placements, host plus one client: the client gets the free movement placement. Its pawn carries movement.
- [ ] A map with one non-movement placement and one movement placement, host plus one client: the client reuses the host's placement and the collision logs. It is never given the non-movement placement.
- [ ] A map whose only placements are non-movement, host plus one client: the client takes the same path as on a map with no `player_spawn`.

Lifecycle:
- [ ] A client disconnects and reclaims its seat within the hold window, in the same level: it returns to its previous placement. A client that joins during the hold gets a different free placement, not the held one.
- [ ] After the hold expires and the seat is released, the next joiner can receive the released placement. Before expiry it cannot.
- [ ] Host restarts the same level with one client connected: after re-promotion the host's pawn and the client's pawn occupy distinct placements.

Regression guard:
- [ ] A remote seat assigned in the same frame as the host's install, at any point after the install begins, never receives the host's placement. This is the ordering `coop--client-spawn-view-handoff` introduces.

### Manual
- [ ] Host a dev map with two `player_spawn` placements and connect one client. The client spawns at the second placement, not inside or beside the host at the first.

## Path
- **Seams:**
  - `spawn_from_player_starts_with_carried_loadout` loops over every start. It marks the first `PlayerMovement` pawn local.
  - `install_world_cpu` resolves spawn points internally and exposes them only through the post-spawn `before_level_load` hook. Seat 0's assignment needs a pre-spawn seam. `local_carried_loadout` on `WorldInstallHandles` is the precedent for a caller-resolved value.
  - `SeatTable::bind_pawn` copies from `level_spawn_placements`. `SeatTable::occupied_live_placements` unions in the level-spawn set. Both lose that input.
  - `SeatTable::assign_placement`'s doc premise, "map installation may leave live player pawns", goes with it.
  - The `Participating` arm of the host net frame passes `host_spawn_points.len()` as the placement count, and that count includes non-movement placements.
- **Shape:** assign seat 0 through the seat table, then spawn only its movement pawn. The rivals, rejected:
  - Keep spawning and drop unseated pawns from occupancy. The joiner then materializes inside an inert pawn.
  - A joiner adopts the unseated pawn at its placement. That makes two entry paths to a participating pawn, against `networking.md` §Slot lifecycle ("any entry to participating … spawns its pawn"). It bypasses the carried loadout applied at spawn. Until adoption, inert bodies stay in enemy target selection.
  - Merge into `coop--client-spawn-view-handoff`. That brief is larger and gated behind the SH brief. This fix lands first, and the sibling extends its chokepoint.
- **First slice:** a failing test that installs a two-movement-placement fixture as host, promotes one remote seat, and asserts placement 1. Today it gets 0 with "every player_spawn placement is occupied". Precedents: `seat/roster_harness_test.rs` and `boot_spawn_gate_test.rs`.
- **Existing tests:**
  - `carried_health_restores_only_the_first_local_player_start` asserts that a second pawn exists at the second start, with descriptor health. It changes meaning: no second movement pawn exists now. Keep the carried-health half, and invert the second-pawn half.
  - `multiple_spawn_points_spawn_one_entity_each` and `player_spawn_marks_first_successful_movement_pawn_when_earlier_spawn_has_no_movement` use non-movement placements. They stay green as M7's guard.
- **Files past ~800 lines:**
  - `main.rs`: this work only deletes from it.
  - `seat.rs`: production code is under the limit, so put new tests in a sibling test module.
  - `data_archetype.rs` and `startup/lifecycle.rs`.
- The trace and the side effects are in `research.md`.

## Open questions
- Add a two-spawn map under `content/dev/maps/` for the manual row, or reuse a temporary map. **Delegated.**
- The shape of the pre-spawn seam that hands seat 0's placement into the install. **Delegated.**
