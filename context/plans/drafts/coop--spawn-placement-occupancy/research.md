# coop--spawn-placement-occupancy — research

Read at 05ab2d3da (`feat/preferences-comfort-floor`). The placement code is identical on `main` (0170b20b9). Everything is cited by symbol. **V** means read in source this session. **I** means inferred from reading and not executed. Nothing here was run.

## Trace: two-placement map, host plus one client, fresh session

| Step | Effect | Mark |
|---|---|---|
| Host install spawn | `spawn_from_player_starts_with_carried_loadout` loops over every start and spawns a descriptor pawn at each one. It marks only the first pawn carrying `PlayerMovement` as local: P0 at s0 is local, P1 at s1 is not. | V |
| Spawn transform | `spawn_descriptor_instance` places the pawn at the origin plus `ai_capsule_center_from_feet_offset`. That offset is zero for a descriptor without a brain, so a player pawn sits exactly at the authored origin. | V |
| Capture hook | The `before_level_load` hook in `install_world_cpu` runs after pawns are materialized. `capture_player_spawn_placements` matches every `PlayerMovement` pawn to a placement by exact origin equality: P0→0 and P1→1. | V |
| Seat 0 bind | `SeatTable::bind_pawn(Seat(0), P0)` copies `level_spawn_placements[P0]` into `placement_assignments[Seat(0)]`, which is 0. | V |
| Client promotion | In the host net frame's `Participating` arm, `occupied_live_placements` returns seat-bound {0} ∪ level-spawn {0, 1} = {0, 1}. | V |
| Assign | `assign_placement` finds no sticky entry. Occupied is {0, 1} plus the unbound seat 0's assignment 0. No index is free, so it takes the fallback `next_placement_cursor % 2 = 0`, logs "every player_spawn placement is occupied", and sets the cursor to 1. | V |
| Spawn | `host_handle_accept_descriptor_at_placement` spawns a fresh pawn at s0, where the host spawned. P1 stays at s1 with no seat. | V |
| Second client | The fallback gives cursor 1, which is where P1 stands. | I |

Confidence: high. Every step was read. The composed outcome is inferred, because no existing test covers it. `live_placement_occupancy_survives_pawn_movement` binds a single placement, and `placement_assignment_persists_by_seat_and_skips_live_and_held_occupants` passes occupancy in by hand.

## Why E15's letter holds

AC-SEAT-1 bounds itself to "whenever a free one exists". The unseated install pawns are live, so there is never a free one. The doc comment on `assign_placement` says map installation "may leave live player pawns that are not currently represented by a remote connection binding", so counting them was deliberate. E15's "Live pawns outnumber placements" edge frames the overflow as seats promoting in, not as leftovers from install.

## Unseated install pawns: side effects

- **V.** `carried_health_restores_only_the_first_local_player_start` asserts that a second pawn exists at the second start, with descriptor health. The extra pawns are therefore tested behavior, not an accident of the loop.
- **I.** Enemy targeting in `crates/ai/src/targeting.rs` builds its candidates from every holder of `PlayerMovement`, so unseated pawns are probably target candidates. Not confirmed downstream.
- **V.** Only the local pawn goes through `host_register_own_pawn`. The unseated pawns are therefore not in the replicable set, and clients never see them.
- **V.** `hold_disconnected_client` unbinds the held seat's pawn. `release_seat` removes the seat's placement assignment. Neither touches `level_spawn_placements`, so an unseated pawn's occupancy lasts for the whole level.

## Ordering note for `coop--client-spawn-view-handoff`

That brief assigns placements at host install for every seat already admitted. If an assignment runs in the install hook before `capture_player_spawn_placements` records seat 0's placement, the assignment cannot see the host's occupancy. The same-frame Acceptance row pins this. **I.**
