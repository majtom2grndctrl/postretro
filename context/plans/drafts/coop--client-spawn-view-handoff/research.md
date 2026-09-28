# coop--client-spawn-view-handoff — research

Read at 695763db4 (`feat/preferences-comfort-floor`). Symbols, not lines. **V** means read in source this session. **I** means inferred from reading and not executed.

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

Before the pawn arms, no local pawn carries `PlayerMovement`. Remote pawns do not get one on the client. As a result, `local_player_movement_pawn` returns none and the camera does not move (V). Input is not sent without a participation epoch (V: `NetClient::send_input`).

## Placement facts

- **V.** Every `.map` under `content/` has at most one `player_spawn`. `campaign-test` has one.
- **V.** On the host, `spawn_from_player_starts_with_carried_loadout` spawns a pawn at every `player_spawn` and marks the first one local. `capture_player_spawn_placements`, run from the `install_world_cpu` hook, binds every such pawn to its placement. `occupied_live_placements` then reports all of them as occupied.
- **V.** When every index is occupied, `assign_placement` falls back to `next_placement_cursor % count` and logs "every player_spawn placement is occupied". On a single-spawn map the client therefore reuses index 0, the host's spawn, which is also the client's stand-in view. **On current content, the jump the SH brief accepts never happens.**
- **I (defect).** On an N-spawn host map, the host's own spawn pass occupies all N placements. The first joining client therefore falls back to the cursor index, 0 on a fresh session, rather than a free spawn. That contradicts E15 AC-SEAT-1's intent ("a placement that no live pawn occupies ... whenever a free one exists"). It only holds because unit tests bind a single placement (`live_placement_occupancy_survives_pawn_movement`). The fix belongs in its own brief. Until it lands, the manual row of this brief cannot pass for the first client.
- **V.** The placement is sticky per seat within a level (`assign_placement`'s early return), cleared at level unload (E15), and scoped by `placement_count`.
- **V.** `level_content_digest` hashes movers, waypoints and static collision inputs. It does not hash `player_spawn` placements.
- **V.** On a host `restartLevel`, the client does not reload: `relevel_is_already_selected` returns early for the active catalog id. It is demoted (the `Holding` arm of `client_drain_control` clears its state) and then re-promoted in place at a newly assigned placement.

## Candidate shapes

| Shape | Wire change | Host sees | Timeout | Verdict |
|---|---|---|---|---|
| (a) Client declares at install and stays in Settling until armed. A separate "ready" message ends the pawn's exposure | New client message; host pawn-hold state | Pawn spawned and ticked while the client is still on the splash, unless it is held | Needs snapshot apply during Settling, but the world-less poll excludes it | Rejected. Two new mechanisms, and it breaks "entry spawns pawn" |
| (b) Host sends the spawn pose before promotion; the client settles there; parity at reveal | One appended server Control message | Pawn spawns at client reveal, at the pre-assigned placement | Unchanged; the hint is advisory | **Chosen** |
| (c) Reveal at the stand-in, then re-settle after arming | None | Pawn ticked during the re-settle | A second hold, or a visible SH fill | Rejected. Two reveals, or the pop stays |
| (d) Client derives the placement from its seat | None | — | — | Rejected. Assignment depends on occupancy (E15) |

## Settling interaction

- **V.** Under the SH brief, the client's declaration is `None` during Settling, so it is held as level-absent. That is the same diagnostic it already gets during Loading today. The join seed rides with the next declaration: `NetClient::set_join_seed` resets `join_seed_sent`, and seeds are sent only alongside a parity send.
- **V.** The host's world-less poll handles admission and disconnects. The hint's admission-time send must hook both host polls, because a joiner can arrive during the host's Settling.
- **I.** The gap between reveal and arming is about the connection's round trip plus up to one snapshot interval. It was not measured.

## Premises in the SH brief

- **I (hazard).** Only `set_level_parity` should move to reveal. `set_relevel_catalog_id` sits beside it in `install_level_payload`. Moving the whole block would delay every client's load until the host's settle ends.
- **V.** Today, the camera the SH brief settles from is the spawn origin with no eye height. The first Running frame lifts it by `capsule.eye_height` (`follow_camera_to_local_pawn`). So the settle view is not exactly the view that is revealed.
