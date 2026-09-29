# coop--spawn-placement-occupancy — research

Read at 3f070e32d (`feat/preferences-comfort-floor`). The placement code is unchanged since 05ab2d3da. The seat table lives in `crates/netcode` (`postretro-netcode`). Everything is cited by symbol. **V** means read in source. **I** means inferred from reading and not executed. Nothing here was run.

## Trace: two-placement map, host plus one client, fresh session (today)

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

## Prior commitments this brief touches

- **E15 AC-SEAT-1.** It bounds itself to "whenever a free one exists". The unseated install pawns are live, so today a free placement never exists. The doc comment on `assign_placement` says map installation "may leave live player pawns that are not currently represented by a remote connection binding", so counting them was deliberate. **V.**
- **E15 Task 6.** It says "occupancy is measured in pawns, not seats", because `spawn_from_player_starts` spawns one pawn per `player_spawn`, and a scan over seats alone would drop a joiner on a level-spawned pawn. Once the install spawns only seat 0's movement pawn, every live movement pawn is seat-bound, so the two measures agree. **V** for the text. **I** for the equivalence.
- **E15's "N player-start entities" edge.** Seat 0 seeds only the pawn `mark_local_player_pawn` selects. That still holds: the pawn is now the only movement pawn. **V.**
- **M7 acceptance.** It requires one entity per spawn entry, and one of each classname when `entity_class` values differ. The existing tests that pin this use non-movement stub descriptors, `multiple_spawn_points_spawn_one_entity_each` among them, so they survive. `carried_health_restores_only_the_first_local_player_start` is the only test that asserts a second movement pawn. **V.**

## Unseated install pawns: side effects today

- **V.** Enemy targeting: `target_candidates` in `crates/ai/src/targeting.rs` chains every `PlayerMovement` holder into the candidate set. Unseated pawns are therefore target candidates. Whether one wins selection depends on distance and faction; that part is not traced.
- **V.** Only the local pawn goes through `host_register_own_pawn`. The unseated pawns are therefore not in the replicable set, and clients never see them.
- **V.** `hold_disconnected_client` unbinds the held seat's pawn. `release_seat` removes the seat's placement assignment. Neither touches `level_spawn_placements`, so an unseated pawn's occupancy lasts for the whole level.

## Non-movement placements

- **V.** `capture_player_spawn_placements` walks only `PlayerMovement` holders, so a non-movement placement is never recorded as occupied.
- **V.** The `Participating` arm passes `host_spawn_points.len()`, which counts every placement, as the placement count. So a non-movement index is in the scan range.
- **V.** `host_handle_accept_descriptor_at_placement` spawns the placement's own `entity_class`. Assigning a non-movement index would hand the client a pawn it cannot drive.
- **I.** This is latent today, because every index is occupied. Once unseated pawns stop counting, the scan would reach non-movement indices first. Hence the movement-only assignable set.
- **V.** A movement descriptor is one with `movement` set. `attach_descriptor_components` attaches `PlayerMovementComponent` from it, so the check can run before spawn.

## Installs without a seat table

- **V.** `session.seat_table` is `None` only on a connected client, which suppresses the boot pawn.
- **V.** The observe driver and the lifecycle test fixtures call `install_world_cpu` with a no-op hook. Those that leave `suppress_boot_pawn` false still spawn a local pawn, so they need the pin rule without a seat table.

## Content survey

- **V.** Every `.map` under `content/` has exactly one `player_spawn`, and none sets `entity_class`. The only `entity_class` reference is its FGD definition in `sdk/TrenchBroom/postretro.fgd`.

## Ordering note for `coop--client-spawn-view-handoff`

That brief pre-assigns placements in the install hook for every seat already admitted. Its Path says to do so after `capture_player_spawn_placements`, and this brief deletes that helper. Seat 0 is now assigned before the install spawns. A remote assignment anywhere later in the install therefore sees the host's placement. The Regression-guard row pins this. **I.**
