# coop--client-spawn-view-handoff — research

Read at b6e9a7c (`claude/sleepy-albattani-zzoos7`). The first draft was read at 3f070e32d. Symbols, not lines. **V** means read in source this session. **I** means inferred from reading and not executed.

## Moved since 3f070e32d

| Old premise | Now | Effect on the brief |
|---|---|---|
| Gated behind `sh-streaming--reveal-gate-and-warm-horizon` | Shipped (`plans/done/`). Settling, `presented_pose()` / `place_camera_at_presented_pose`, the settle chokepoint and the revealed hold all exist (V) | Gate dropped. The hint enters the spawn half of the presented pose |
| Every `content/` map has at most one `player_spawn`, so the defect is latent | `coop-presentation-test`, `ghost-links` and `ghost-links-luau` have two each, at the same angles and 64 units apart (V). On `ghost-links` the second is `coop_only` | The position jump is reachable on dev content now |
| The stand-in is "the spawn origin with no eye height" (old SH note) | Still true on a client. `first_spawn` is `standing_position` (the capsule centre). `followed_pawn_eye` is `None` without a local pawn, so `place_camera_at_presented_pose` keeps the capsule centre (V). Dev `player` eye height is 0.5 above it (V: `content/dev/scripts/player.ts`) | Every join lifts the view on its arm frame, single-spawn maps included (I) |
| Install spawns a pawn at every `player_spawn`; `capture_player_spawn_placements` binds them | Install spawns one pawn, the first player's, in `first_player_spawn_order` (open spawns first, then `coop_only`), and binds it through `bind_installed_first_player` → `bind_level_spawn_placement` + `bind_pawn` (V) | Install-time pre-assignment goes after this bind |
| `SeatTable::assign_placement` is the entry point | It is private. `assign_joiner_placement(registry, seat, eligible)` is the public entry, and `eligible` comes from `joinable_spawns` (spawns whose `entity_class` resolves) (V) | Path names the public entry |
| Admission's Relevel hooks the engine's `admit_or_reclaim` | `NetServer` sends Relevel inside its handshake processing, at admission (V: the `Admission` arm calls `send_relevel`). The engine's `admit_or_reclaim` runs afterwards, in `poll_world_less_transport` and the Running host poll (V) | The hint queued after `admit_or_reclaim` follows Relevel on Control |
| Arming adopts the pawn's replicated facing | Host movement rewrites a remote pawn's rotation as `from_rotation_y(facing_yaw)` from the client's command on every played tick (V: `run_host_movement_tick`). Before the client's first command arrives, no queue state exists and the pawn is not ticked, so it keeps its spawn rotation (V: `resolve_tick_with_switches` returns `None`) | Decision dropped. The replicated rotation can be either the authored one or the client's own echoed yaw, with pitch zeroed |
| None of the hint shipped | Still none: no spawn-pose variant in `ServerControlMessage` (`net/src/wire/control.rs`), and no hint symbol in `crates/net` or `crates/netcode` (V) | — |

## Client join sequence today

| Step | Client renders | Host slot | Source |
|---|---|---|---|
| Connect, admission | splash or frontend | pending → admitted, seat minted | V: the `Admission` arm in `NetServer`; engine `admit_or_reclaim` |
| Host names the map | same | admitted; Relevel at admission or at host install | V: `set_relevel_catalog_id` at the top of `install_level_payload`, before `install_world_cpu` |
| Client loads, installs | splash | admitted, held | V: `follow_relevel_catalog` enqueues the load; install writes `first_spawn` to the camera, then `place_camera_at_presented_pose` |
| Client Settling | loading tree; streams from `presented_pose()` each frame | held: `ClientNotRevealed` | V: `run_settling_frame` |
| Client reveal | world at the first player's spawn, capsule centre, that spawn's yaw and pitch | — | V: reveal edge |
| Both revealed, parity matches | same | participating: `assign_joiner_placement` → `host_handle_accept_descriptor_at_placement`; marker, tuning | V: the `Participating` arm in main.rs's Running host net frame; `begin_participation` |
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

- **V.** `occupied_live_placements` counts seats with a live bound pawn (their assignment) plus install pawns in `level_spawn_placements`.
- **V.** `assign_placement` also counts other seats' assignments, but only for seats without a client binding (held seats). A client-bound seat with an assignment and no live pawn is not counted. That covers a seat that was pre-assigned or demoted mid-level. This brief widens the rule.
- **V.** Assignment is sticky per seat within a level: the early return recalls `placement_assignments`. With every eligible spawn occupied, `overflow_cursor` round-robins and warns.
- **V.** `clear_pawn_bindings_for_level_unload` clears `placement_assignments`, `level_spawn_placements` and `overflow_cursor`. `release_seat` drops the seat's assignment, and `hold_disconnected_client` does not.
- **Docs disagree.** `networking.md` §Slot lifecycle says assignments and the round-robin position are level-scoped and reset at unload, which matches source. §Session-state ledger still lists a "level-independent placement-assignment cursor" from E15's `next_assignment`, which `0d0aa24` replaced with the level-scoped `overflow_cursor`. Fix the ledger at promotion.
- **V.** `level_content_digest` does not hash `player_spawn` placements.
- **V.** A level with no `player_spawn` takes `host_handle_accept`, whose `TransformFixture` pawn has no movement component, so the client never arms one. Spawns that exist but none of which resolve close the slot.
- **V.** On a host `restartLevel`, the client does not reload: `relevel_is_already_selected` returns early. The `Holding` arm of `client_drain_control` demotes it, and the host re-promotes it at the host's reveal.

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
| Client derives the placement from its seat | None | Rejected: assignment depends on occupancy (E15) |
| Settle every spawn view | None | Rejected: fixes the SH miss only, not the position jump or the eye lift; cost grows with spawn count |
| Assign at promotion, send the pose with the epoch | One message | Rejected: arrives after reveal, when the settle view is already fixed |
| Arm-time adoption of the replicated rotation as a fallback | None | Rejected: no live case (every arming client holds its hint), and the rotation may already be the client's own yaw with pitch zeroed |
