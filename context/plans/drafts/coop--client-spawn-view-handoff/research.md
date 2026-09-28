# coop--client-spawn-view-handoff — research

Read at 3f070e32d (`feat/preferences-comfort-floor`). Symbols, not lines. **V** means read in source this session. **I** means inferred from reading and not executed.

## Client join sequence today

| Step | Client renders | Host slot | Source |
|---|---|---|---|
| Connect, admission sent | splash or frontend | pending → admitted, seat minted | V: `NetClient::queue_control_messages`; host `admit_or_reclaim` in both host polls |
| Host names the map | same | admitted; Relevel sent on admission or at host install | V: `NetServer::set_relevel_catalog_id`, `send_relevel`, `relevel_recipients` |
| Client loads | splash (Loading) | admitted, held | V: `follow_relevel_catalog` enqueues a catalog load |
| Client install | splash | same | V: `install_level_payload` calls `set_level_parity` (the client's declaration) and teleports the camera to `first_spawn`; `install_world_cpu` suppresses the boot pawn for a connected client |
| Client reveal (today: same frame as install) | world from spawn 0, with spawn 0's yaw, at spawn origin | admitted | V: `finish_level_payload` → `clear_splash` → Running |
| Declaration arrives, parity matches | same | participating: placement assigned, pawn spawned and ticked, tuning sent | V: the host net frame's `Participating` arm → `assign_placement` → `host_handle_accept_descriptor_at_placement` |
| Epoch marker, tuning, first snapshot | same | same | V: `client_drain_control` (Tuning); `NetClient::is_participating` |
| `local_player` baseline arms the pawn | camera position jumps to the pawn eye; yaw keeps its prior value | same | V: `maybe_arm_local_pawn`; `follow_camera_to_local_pawn` writes position only; on level entry only `install_level_payload` and `--start-pose` write `camera.yaw` |

Before the pawn arms, no local pawn carries `PlayerMovement`. Remote pawns do not get one on the client. As a result, `local_player_movement_pawn` returns none and the camera position does not move (V). Input is not sent without a participation epoch (V: `NetClient::send_input`).

## Join sequence after this brief and the SH brief

| Step | Client | Host slot |
|---|---|---|
| Host install | — | placement pre-assigned per admitted seat; Relevel, then hint |
| Client install | splash; parity declared; stand-in at hinted pose | parity evaluated; divergence cause sent now if content differs; else held until revealed (SH brief) |
| Client Settling | splash; settles at hinted pose | held (client not revealed) |
| Client reveal | world at hinted pose; revealed declared (SH brief) | — |
| Revealed drained | same | participating: pawn at pre-assigned placement, epoch, tuning |
| First baseline arms | no position or yaw jump | same |

## Placement facts

- **V.** Every `.map` under `content/` has at most one `player_spawn`. `campaign-test` has one.
- **V.** On the host, `spawn_from_player_starts_with_carried_loadout` spawns a pawn at every `player_spawn` and marks the first one local. `capture_player_spawn_placements`, run from the `install_world_cpu` hook, binds every such pawn to its placement. `occupied_live_placements` then reports all of them as occupied.
- **V.** When every index is occupied, `assign_placement` falls back to `next_placement_cursor % count` and logs "every player_spawn placement is occupied". On a single-spawn map the client therefore reuses index 0, the host's spawn, which is also the client's stand-in view. On current content, the jump never happens.
- **I (defect, own brief).** On an N-spawn host map the host's own spawn pass occupies all N placements, so the first joiner falls back to the cursor. `coop--spawn-placement-occupancy` fixes it and counts seated pawns and held seats. It assigns at promotion, so it does not count a placement pre-assigned to a seat that has no pawn yet; this brief extends the rule to cover that.
- **V.** `assign_placement` also counts other seats' assignments as occupied, but only for seats with no client binding (held rejoin seats). A pre-assigned seat that is client-bound but has no pawn yet is not counted.
- **V.** The placement is sticky per seat within a level (`assign_placement`'s early return), cleared at level unload (E15), and scoped by `placement_count`.
- **V.** `level_content_digest` hashes movers, waypoints and static collision inputs. It does not hash `player_spawn` placements.
- **V.** On a host `restartLevel`, the client does not reload: `relevel_is_already_selected` returns early for the active catalog id. It is demoted (the `Holding` arm of `client_drain_control` calls `demote_client_state`) and then re-promoted in place.

## Facing at arm

- **V.** The host spawns a slot pawn through `spawn_net_slot_pawn_with_carried_loadout` → `spawn_descriptor_instance`, which sets the pawn's `Transform` rotation from the placement's `rotation_quat`. Snapshots carry it as `WireTransform.rotation`.
- **V.** `follow_camera_to_local_pawn` writes camera position only. Without a hint, an armed client keeps the first spawn's yaw from `install_level_payload`.
- **V.** The Running frame applies mouse look to the camera once per render frame (`Camera::rotate` in main.rs's gameplay block); the call read is not gated on an armed pawn.
- **I.** So an unarmed client can already turn its view, and adopting the pawn's facing at arm may snap it. Delegated in the brief.

## Candidate spawn-view shapes

| Shape | Wire change | Verdict |
|---|---|---|
| Host sends the spawn pose before promotion; the client settles there | One appended server Control message | **Chosen** |
| Reveal at the stand-in, then re-settle after arming | None | Rejected. Two reveals, or the pop stays |
| Keep the splash until arm | None | Rejected. Needs snapshot apply during Settling |
| Client derives the placement from its seat | None | Rejected. Assignment depends on occupancy (E15) |
| Settle every spawn view | None | Rejected. Fixes the SH miss at whichever spawn is assigned, not the position and yaw jump; cost grows with spawn count |
| Assign at promotion, send the pose with the epoch | One message | Rejected. Arrives after reveal; the settle view is already fixed |

## Premises in the SH brief

- **V.** Today, the camera the SH brief settles from is the spawn origin with no eye height. The first Running frame lifts it by `capsule.eye_height` (`follow_camera_to_local_pawn`). The SH brief's eye-pose chokepoint is where this brief's hint enters.
