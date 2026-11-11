# coop--client-spawn-view-handoff — research

Read at b215170. Earlier reads: b6e9a7c, 3f070e32d. Symbols, not lines. **V** means read in source. **I** means inferred from reading and not executed.

## Moved since b6e9a7c

| Old premise | Now | Effect on the brief |
|---|---|---|
| Private `assign_placement` counts other seats' assignments only for seats without a client binding, so a reservation must widen the rule | `assign_placement` and `occupied_live_placements` are gone. `SeatTable::held_elsewhere` counts every other seat's `placement_assignments` entry, client-bound or held, plus live install pawns' `level_spawn_placements` (V). `PlacementOccupancy` (`crates/sim/src/placement_occupancy.rs`) judges free vs live overlap (V) | Nothing to widen. The reservation is the assignment, made at resolution instead of promotion |
| `assign_joiner_placement(registry, seat, eligible)` | Takes `sites: &[PlacementSite]` from `joinable_sites` (foot anchor, capsule; no yaw or pitch) and returns `None` without the seat's carried loadout (V) | Path names the new inputs. Resolution must run after the carried loadout exists |
| Recalled assignment is sticky | Sticky only while no live pawn overlaps it; otherwise the lowest free site (V) | Owner: keep the guard; re-resolve and resend the hint |
| Respawn pick lands later (E18 AC13) | Landed. `drain` (`crates/sim/src/sim/respawn.rs`) reads `RespawnPlacements.seats`, a snapshot of seat assignments published by `respawn_placements::build`/`publish`, and treats other seats' entries as held. A seat with no placement prefers `first_player`, then the first free site (V) | Respawn already honours reservations once the snapshot is republished after resolution. The degraded `live_unoccupied` fallback ignores holds by design, only when nothing is free |
| Host restart: the client does not reload (`relevel_is_already_selected`) | A level life advances on every catalog install, same-map restart included. `follow_relevel_catalog` reloads on a later life naming the active map; `relevel_load_is_selected` only skips a load already in flight (V; `networking.md` §Slot lifecycle, "A host restart is a relevel") | The hint carries the level life. Running-unarmed apply serves a late or re-resolved hint (O5, O11) and an uncatalogued host reinstall, which installs and hints afresh; a mod-digest re-promotion gets no hint (O10) |
| `install_level_payload` is the install | It is a `#[cfg(test)]` oracle. Production runs `lifecycle_commit.rs` → `install_level_identity` (Relevel via `set_relevel_catalog_id`) → `install_level_segment_b` (`bind_installed_first_player`, `respawn_placements::publish`, `host_spawn_points`) (V) | Send site moves after segment b's bind |
| Stand-in camera write in `install_level_payload` | `App::install_level_camera_pose` (`startup/lifecycle_install_presentation.rs`); `followed_pawn_eye` in `startup/lifecycle_spawn_residency.rs` (V) | Path pointers only |
| `coop-presentation-test` has two `player_spawn` | Three (V) | Manual row unchanged |
| Wire constants | `PROTOCOL_ID` `PRLC`, `WIRE_VERSION` 39, `SNAPSHOT_VERSION` 25. `ServerControlMessage` still ends at `SessionRoster`. Append guards: `session_roster_append_preserves_shipped_control_encodings` and siblings in `handshake.rs` (V) | App protocol moves; `WIRE_VERSION` only if the guard fails |

## Frontend menu on the join path

- **V.** `start_boot_destination` (`startup/splash_lifecycle.rs`) presents the frontend menu whenever a host load is queued at splash clear: the `Frontend` arm and the `BootMap(_) if host_load_queued` arm. With no queued host load and no map argument, `populate_frontend` pushes it with a backdrop load.
- **V.** No level load, install or unload touches the modal stack. The only `clear_pushed` calls outside tests are the first-launch hold and the `LoadLevel`/`RestartLevel` system commands, which `refuse_client_level_verb` stops on a connected client first.
- **V.** `apply_frontend_menu_camera_pose_if_present` → `apply_menu_camera_pose` writes `camera` position, yaw and pitch every frame while the root is pushed, so after dismissal the view keeps the menu's yaw and pitch.
- **I.** Back/cancel never pops the frontend root (`ui.md`). A mod `popTree` reaction might; not checked.
- **V.** A map argument with no queued host load boots that map with an empty stack. A relevel arriving after it runs loads with no menu. One arriving while that boot load is in flight is dropped by `enqueue_level_request` (pinned behavior).
- **I.** Pawn arming and `follow_camera_to_local_pawn` have no menu gate, so a client behind the menu arms with the menu's yaw and pitch.
- **Interplay with `drafts/coop--client-leave-session`.** Until that draft lands, a connected client that chose quit-to-menu stays connected, and its host's next relevel installs the level and now also clears its menu. That draft's Problem already describes the client being pulled back into play. Once it lands, quit-to-menu leaves the session, so every connected client that installs a host-named level is following its host. The two drafts need no ordering.

## What the player sees today (source trace, not run)

- **Documented flow** (`--host` and `--connect` with the same `.prl`). Path levels send no Relevel, and the client's boot map installs with no menu. The client reveals at spawn index 0 at capsule centre. On arming the view rises by the dev eye height (0.5 m). On `coop-presentation-test` (three spawns, 64 units apart, all angles zero) the client is assigned spawn 1, so the view also slides about 1.6 m sideways. On `campaign-test` (one spawn) joiners share it, and the jump is only the eye lift. No dev map has spawns with differing yaw, so the facing half of the defect has no dev fixture yet.
- **No client map argument.** The menu stays over the host's level, and its pose overwrites the camera every frame. Escape and cancel never pop the frontend root. `nav.menu` defers to the active modal. The dev menu's level buttons are refused on a client. No dev script pops the root. The root captures input, so the client cannot move. The pawn arms on the host.
- **No test asserts either behavior.**

## Client join sequence today

| Step | Client renders | Host slot | Source |
|---|---|---|---|
| Connect, admission | splash or frontend | pending → admitted, seat minted | V: the `Admission` arm in `NetServer`; engine `admit_or_reclaim` |
| Host names the map | same | admitted; Relevel at admission or at host install | V: `set_relevel_catalog_id` in `install_level_identity`, before `install_level_segment_b` |
| Client loads, installs | splash | admitted, held | V: `follow_relevel_catalog` enqueues the load; install writes `first_spawn` to the camera, then `place_camera_at_presented_pose` |
| Client Settling | loading tree; streams from `presented_pose()` each frame | held: `ClientNotRevealed` | V: `run_settling_frame` |
| Client reveal | world at the first player's spawn, capsule centre, that spawn's yaw and pitch | — | V: reveal edge |
| Both revealed, parity matches | same | participating: `assign_joiner_placement` → `host_handle_accept_descriptor_at_placement`; marker, tuning | V: the `SlotEvent::Participating` arm in main.rs's Running host net frame; `begin_participation` |
| `local_player` baseline arms | camera moves to the pawn eye (placement standing position plus eye height); yaw and pitch unchanged | same | V: `maybe_arm_local_pawn` reports `armed_local_pawn`; the engine arm site writes no camera orientation; `follow_camera_to_local_pawn` writes position only |

## Join sequence after this brief

| Step | Client | Host slot |
|---|---|---|
| Host install, after the host's pawn binds | — | each client-bound seat resolved; Relevel (already sent), then hint |
| Admission mid-level | — | Relevel, then resolved hint |
| Client install | stand-in at hinted eye, yaw, pitch | held |
| Client Settling | settles there; a late hint retargets | held |
| Client reveal | world at the hinted pose | — |
| Promotion | — | pawn spawns at the recorded pose; marker follows the hint on Control |
| First baseline arms | position within 1 cm; yaw and pitch untouched | same |

## Placement and occupancy

- **V.** `held_elsewhere` counts every other seat's assignment plus live install pawns in `level_spawn_placements`.
- **V.** Assignment is sticky per seat within a level while no live pawn overlaps it: the early return recalls `placement_assignments`. With every eligible spawn occupied, `overflow_cursor` round-robins and warns.
- **V.** `clear_pawn_bindings_for_level_unload` clears `placement_assignments`, `level_spawn_placements` and `overflow_cursor`. `release_seat` drops the seat's assignment, and `hold_disconnected_client` does not.
- **Docs disagree.** `networking.md` §Slot lifecycle says assignments and the round-robin position are level-scoped and reset at unload, which matches source. §Session-state ledger still lists a "level-independent placement-assignment cursor" from E15's `next_assignment`, which `0d0aa24` replaced with the level-scoped `overflow_cursor`. Fix the ledger at promotion.
- **V.** `level_content_digest` does not hash `player_spawn` placements.
- **V.** A level with no `player_spawn` takes `host_handle_accept`, whose `TransformFixture` pawn has no movement component, so the client never arms one. Spawns that exist but none of which resolve close the slot.
- **V.** On a host `restartLevel`, the client reloads: the restart advances the level life, and `follow_relevel_catalog` treats a later life naming the active map as a reinstall. The slot holds until the client reveals the new life.

## Pose, pitch and facing

- **V.** The host spawns a slot pawn through `spawn_net_slot_pawn_with_carried_loadout` → `spawn_descriptor_instance`. Position is `origin + spawn_origin_shift` (the capsule centre), and rotation is the placement's `rotation_quat()`, YXZ from `angles`, so it carries authored pitch.
- **V.** Host install seeds its own camera from `first_spawn`: yaw `angles.y`, pitch `angles.x`. The hint's yaw and pitch follow the same convention.
- **V.** How a client gets aim pitch today: its own camera pitch, seeded at install from the stand-in spawn's `angles.x`, then mouse look (`Camera::rotate`, not gated on arming). The client sends it in each Input command. The host keeps `latest_aim_pitch` per client and echoes it only as `WireMovementState.aim_pitch`, which is presentation for remote avatars. That field is 0 until the client's first command (`current_aim_pitch(..).unwrap_or(0.0)` in replication). The client never takes its own pitch from the host, so the hint is the only host value that sets it.
- **V.** `WireMovementState` carries `capsule_eye_height`, so the armed pawn's eye uses the host's eye height. The hinted eye uses the same descriptor value.
- **I.** Until the host plays the client's first command, the pawn is not ticked and stays at its spawn pose, so the arming baseline's eye equals the hinted eye.

## Ordering

- **V.** Control is reliable-ordered. `send_control` and `begin_participation` (the payload-less marker that arms an epoch) both ride it. Snapshots are epoch-framed, and the client drops traffic outside its active epoch.
- **V.** A fresh `NetClient` starts with `revealed_level: None` and `revealed_sent: true`, so it declares a reveal only after revealing a level post-connect. Promotion needs that reveal, which comes after admission and after the hint the engine queues there.
- **V.** `client_drain_control` runs from `poll_world_less_transport` (Loading, Settling, Frontend) and from the Running poll, so a hint is seen in every state.
- **V.** `reset_level_scoped_client_state` clears tuning, prediction, interpolation and slot schema. `demote_client_state` also despawns mapped entities. Neither may touch the hint store.

## Candidate spawn-view shapes

| Shape | Wire change | Verdict |
|---|---|---|
| Host resolves the pose before promotion; the client settles there | One appended server Control message | **Chosen** |
| Reveal at the stand-in, then re-settle after arming | None | Rejected: two reveals, or the pop stays |
| Keep the splash until arm | None | Rejected: needs snapshot apply during Settling |
| Raise the stand-in to eye height from the client's own descriptor; leave the hint to E25 U2 | None | Rejected: reads a value the host replicates; on a multi-spawn map the view keeps the first spawn's facing; U2 needs the resolver anyway |
| Client derives the placement from its seat | None | Rejected: assignment depends on occupancy (E15) |
| Settle every spawn view | None | Rejected: fixes the SH miss only, not the position jump or the eye lift; cost grows with spawn count |
| Assign at promotion, send the pose with the epoch | One message | Rejected: arrives after reveal, when the settle view is already fixed |
| Arm-time adoption of the replicated rotation as a fallback | None | Rejected: no live case (every arming client holds its hint), and the rotation may already be the client's own yaw with pitch zeroed |

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| O1 | Connected client, no map argument; mod's frontend backdrop is the catalog level the host plays | backdrop installs before the first Relevel, which would adopt the active map | the installed backdrop reloads as the host's level: menu cleared, settles at the hint |
| O2 | Same as O1 | first Relevel arrives while the backdrop load of that id is in flight; the load is not restarted and installs | install is the host's level: menu cleared, settles at the hint |
| O3 | Seat admitted while the host is loading its next level or at its frontend | admission (no level installed) → host install | no hint at admission; at install, Relevel then a hint resolved against the new level's placements |
| O4 | Ordinary join; host answers parity with a revealed hold, or a relevel retracts the client's old parity | hint → holding notice (client demotion) → install / Settling | hint survives the demotion; client settles at it |
| O5 | Live pawn on a seat's reserved start at promotion | marker → engine re-resolves → fresh hint; a snapshot may arm before the hint lands | arm at the pawn eye; a hint arriving after arming moves nothing |
| O6 | Seat spawns from a single-use source; later re-promotes without reload | spawn consumes source → fresh resolution and hint → later marker | the fresh hint precedes the next marker |
| O7 | Hint (resent after loss) lands on the reveal frame | Settling releases on the old stand-in → reveal frame's world poll drains the hint | reveal frame presents the settled pose; the hint applies from the next frame while unarmed |
| O8 | Host install with client-bound seats | Relevel (identity stage) → host pawn binds → spawn points set → per-seat hint | each seat's Relevel precedes its hint at install, as at admission |
| O9 | Connected client whose menu an install cleared; host's next level fails on the client | install clears menu → later relevel → unload → load fails | the frontend menu is presented again, with no backdrop load |
| O10 | Mod-digest demotion of a Running client, then re-promotion on the same install | demotion (pawn despawned, camera where it stood) → re-promotion at the reserved pose → arm | view moves once at arming, as at a respawn; no hint is sent |
| O12 | Backdrop is the host's catalog level and is still Settling when the first Relevel arrives | backdrop install (menu pose) → Relevel | the backdrop reloads as the host's level and settles at the hint |
| O13 | Same, but the backdrop has revealed and is Running | backdrop reveal at the menu pose → Relevel | reloads as O12; never adopted unsettled |
| O14 | Host's level loads with no hint that matches (no `player_spawn`, or a non-finite hint) | menu cleared at install → no hint | camera takes install's first-spawn stand-in, not the menu pose |
| O15 | Submenu (Options, or a mod tree) open over the frontend root | submenu pushed → host-named install | every frontend layer cleared |
| O16 | Connected client whose menu an install cleared; host relevels to a catalog id the client lacks | install clears menu → Relevel(unknown id) | stale level unloads; frontend menu presented, no backdrop load |
| O17 | Client following its host on the level its backdrop names; player quits to menu (before `drafts/coop--client-leave-session` lands) | quit-to-menu → backdrop load of the same id, not requested by a relevel | menu stays; the load is not host-named |
| O18 | Host-named load fails, and that level is also the backdrop | load fails → menu presented | no backdrop load, so no load–fail loop |
| O19 | Host booted with no map argument sits at its frontend over a catalog backdrop; a client is admitted | host backdrop install → admission | no Relevel, no hint, no promotion; client waits at its menu |
| O20 | Host's install finishes after the client is already Settling the same level (slower host) | client Relevel → client install → Settling → host install completes → hint | Settling retargets to the hint without resetting its timer |
| O11 | Uncatalogued path host; client booted the same map and revealed it before its connection came up | Admission, Parity and Revealed in one host batch → `reevaluate_parity` promotes and sends the marker inside the poll → engine admits the seat, resolves, sends the hint | hint follows the marker; client applies it while unarmed, or not at all if the pawn armed first |

## Decision rationale

- **Resolve before promotion.** Choosing a spawn at promotion arrives after reveal, when the settle view is fixed. Resolving earlier forecloses a promotion-time spawn policy; a later policy such as spawning near teammates would need a fresh hint.
- **Hint before marker.** For a client that reveals after admission it holds by construction: promotion waits for both reveals, and the host reveals only after the install that resolves. O11 is the one batch where the client's reveal predates admission.
- **Same-map restart.** A restart is a relevel with a new life, so the client reloads and matches the new life's hint. The Running-unarmed apply serves O5, O11 and a single-use re-resolution, not restarts.
- **Placement from the seat.** Rejected: assignment depends on live occupancy (E15 AC-SEAT-1), so the client cannot derive it.
- **Backdrops are menu-only (owner).** A backdrop carries only what navigating the menu needs; playing a map loads it from player spawns with full game-time behavior. So an installed backdrop the host names reloads, and a host's own backdrop names no level to clients.
- **Menu clear keyed on the host's level.** After `drafts/coop--client-leave-session` lands, every connected client at the frontend is following its host, so the clear never pulls a player who chose to leave. A backdrop the host has not named keeps the menu. The failure fallback (O9) restores what the never-cleared menu gave by accident.
- **Re-entries that move the view once (owner).** O5, O10 and O11 are rare and each returns the player to a spawn, like a respawn. Gating promotion on the hint would add a holding cause to the participation predicate for these cases alone.

