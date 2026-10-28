# coop--client-spawn-view-handoff

Brief · compact · reads: `context/lib/networking.md` §Slot lifecycle, §Two-gate handshake, §Session-state ledger, §Version gates, `context/lib/boot_sequence.md` §1 (Settling) · read at b6e9a7c

**Sequence:** prerequisite of Epic 25 U2 (`drafts/E25--landmark-level-change/`).

## Problem
Raised by the owner while drafting the SH reveal gate. Basis: a defect read in source, not yet seen in play (`research.md`). A connected client learns its spawn only from the first baseline after the host promotes it. Until then it settles and reveals at a stand-in: where a pawn stands at the first player's `player_spawn`, at capsule centre rather than eye height, with that spawn's yaw and pitch. Arming then moves the camera to the pawn's eye and leaves yaw and pitch alone. Cause: the host picks the client's spawn at promotion and sends it nothing before the pawn arrives. So every co-op join lifts the view by the eye height on its first armed frame. Where the host assigns the client another placement (two dev maps today), the view also jumps to a spot it never settled, which may show ambient-floor SH, still facing the first spawn's way. When done: the client settles, reveals and arms at the eye position, yaw and pitch its pawn spawns with, and arming causes no visible jump and no in-play SH miss at the spawn view.

## Decisions
- **One host-side spawn-pose resolver.** The host takes each remote seat's spawn pose (eye position, yaw, pitch) from one resolver, and the hint and the promotion spawn both read its result, so they cannot disagree. Today it returns the pose of the seat's assigned `player_spawn`: where the pawn stands plus the eye height of the descriptor the placement names, and yaw and pitch from the placement's angles, as install seeds the host camera. The result is recorded per seat and holds until level unload. A later consumer may make the resolver return a pose that no placement backs.
- **Resolve at the first moment the host can name the level to the seat.** That is host install for every client-bound seat, after the host's own pawn binds its placement, and admission for a seat admitted while a level is installed. The client cannot derive the pose, because assignment depends on live occupancy (`plans/done/E15--seat-session-identity-roster` AC-SEAT-1). This forecloses choosing a spawn at promotion: a later policy such as spawning near teammates would need a fresh hint. The host's own seat keeps install's first-player spawn.
- **Only a placement-backed pose reserves a player start.** A seat whose pose comes from a placement reserves it from resolution until unload, with or without a pawn. Every other seat's reservation counts as occupied, held or client-bound. Today `0d0aa24`'s rule counts a client-bound seat only while its pawn lives. A seat whose pose no placement backs holds none. Among seats that use placements, today's distinctness and round-robin overflow stand.
- **The hint is a pose, not a placement index.** `level_content_digest` does not hash `player_spawn` placements, and the hint arrives before parity is proven, so an index could name a different spawn on the client. It goes on Control to that seat's client alone, tagged with the level identity (the parity identity string). When Relevel goes out with it, Relevel goes first. An uncatalogued host still sends it, and a level with no `player_spawn` sends none.
- **The hint precedes promotion.** On Control, a seat's hint for a level comes before the marker that arms its participation epoch for that level, so every client that arms already holds its hint. This holds by construction: a connection declares no reveal until it reveals a level after admission, and promotion waits for that reveal.
- **Advisory, never a gate.** A missing, late or mismatched hint leaves today's first-spawn stand-in in place. It never delays reveal, never extends or resets the Settling timeout, and never enters the revealed term. The first baseline stays the only authority on the pawn's pose.
- **The unarmed stand-in takes the hint's eye, yaw and pitch.** The client applies a hint whose identity matches its installed or installing level at three points: at install, so Settling starts there; on arrival during Settling, which retargets the settle view; and while Running unarmed. The last covers a host restart, which the client does not reload for (`relevel_is_already_selected`). The client keeps only the latest hint, and the level-scoped client reset does not clear it, so a hint sent right after Relevel survives the unload that Relevel queues. With the frontend menu present, the menu pose still wins (`boot_sequence.md` §1, Presented pose).
- **Arming moves position only.** Arming writes no orientation today and keeps writing none, so the client arms with the stand-in's yaw and pitch. The replicated rotation is not a fallback: once the host plays a client's commands, it rewrites that pawn's rotation yaw-only from the client's own facing.
- **Layer placement.** The net crate carries the pose as plain `f32`, glam-free and registry-blind. The resolver is host netcode beside the promotion handler. The seat table records reservations and stays descriptor-blind. The hint store is client endpoint state that survives a level change. At promotion, `networking.md` §Session-state ledger gains the store, and §Slot lifecycle's entry rule gains resolution before promotion.
- **Non-goals.**
  - A rule that derives placement from the seat. It conflicts with E15's occupancy-aware assignment.
  - SH prefetch beyond moving the stand-in camera.
  - Dismissing the frontend menu on a relevel. The frontend hub owns it (`plans/done/sh-streaming--reveal-gate-and-warm-horizon`, One presented pose).
  - Any source of a pose that no placement backs. The resolver's later consumer owns it.

## Acceptance
### Automated
**Resolver and reservation (host)**
- [ ] On host install, each client-bound seat is sent a hint before its promotion. At promotion, the pawn's eye is within 1 cm of the hinted eye, and its spawn rotation carries the hinted yaw and pitch.
- [ ] A test resolver returns a pose that no placement backs for one seat. The hint, that client's stand-in camera and its spawned pawn all use that pose. The seat reserves no placement, and a seat resolved after it takes the lowest free placement as if the first seat were absent.
- [ ] A seat's spawn stays at its hinted pose when occupancy changes between its hint and its promotion, whether another seat takes a placement or frees one.
- [ ] Two seats resolved before either spawns get distinct placements, neither the host's nor a held seat's, while free placements remain. With every placement taken, they share placements round-robin with one warning, as today.
- [ ] A seat demoted mid-level keeps its reservation, and so does a seat held after its client disconnects. A joiner resolved meanwhile does not take it, and the returning seat spawns at its hinted pose.
- [ ] A seat admitted while a level is installed receives Relevel and then its hint. On an uncatalogued level it receives the hint alone.
- [ ] On every promotion, the seat's hint for that level precedes its participation marker on Control.
- [ ] A level with no `player_spawn` sends no hint, and the client joins as it does today.
- [ ] The host's pawn keeps install's first-player spawn, and no joiner is assigned that placement.

**Client stand-in**
- [ ] With a matching hint, the client installs, settles and reveals at the hinted eye position, yaw and pitch, not at the first player start. On a fixture whose spawn has nonzero pitch, the revealed pitch equals it.
- [ ] A client whose local `player_spawn` list differs in order from the host's still settles at the host's pose.
- [ ] A hint in the same Control drain as Relevel still applies to the new level after the old level unloads.
- [ ] A hint for a different level identity is ignored, and the stand-in stays at the first spawn.
- [ ] A hint during Settling retargets the settle view, and the Settling timer does not reset.
- [ ] A client that receives no hint reveals at the first-spawn stand-in with no added hold.
- [ ] A hint with a non-finite component is rejected before it is applied.
- [ ] After a host restart, a Running client's hint moves its unarmed camera to the new eye, yaw and pitch before re-promotion.
- [ ] With the frontend menu present, the client presents the menu pose, with or without a hint.

**Arming**
- [ ] With a matching hint applied, on the frame the pawn arms the camera is within 1 cm of the pre-arm stand-in, with yaw and pitch unchanged. This holds on a single-spawn level and on a level where the client's placement is not the first player's.
- [ ] Regression guard: a client that turns its view while unarmed keeps that yaw and pitch when it arms.

**Wire**
- [ ] The hint survives an encode/decode round trip with its identity, eye position, yaw and pitch. A peer on the previous wire constants is refused at gate 1, and the net crate still has no internal dependency.

### Manual
- [ ] Loopback co-op on `content/dev/maps/coop-presentation-test.map`, where the client's placement is not the first player's. The client's first world frame is at its own spawn eye, facing the authored direction, with no ambient-floor SH, and arming causes no jump.

## Path
- Resolver and reservation: `SeatTable::assign_joiner_placement` and its private `assign_placement` and `occupied_live_placements`. The client-binding filter in `assign_placement` is the occupancy rule to widen. Pose inputs: `joinable_spawns`, `standing_position`, the descriptor's `movement.capsule.eye_height`, and the placement's `angles` (x pitch, y yaw). The promotion handler in main.rs's Running host net frame calls `assign_joiner_placement`, then `host_handle_accept_descriptor_at_placement`. That function takes a placement index, and `spawn_descriptor_instance` builds the transform from the placement, so a pose that no placement backs needs a spawn entry that takes the pose and a descriptor class.
- Send sites: `install_level_payload` sends Relevel through `set_relevel_catalog_id` before `install_world_cpu` binds the host's pawn (`bind_installed_first_player`), so resolve and send after that bind. At admission, `NetServer` sends Relevel inside handshake processing. The engine's `admit_or_reclaim` sites in `poll_world_less_transport` and the Running host poll follow it, and both can reach `App.host_spawn_points`. The epoch marker is `begin_participation`, on Control.
- Client: the `first_spawn` camera write in `install_level_payload`, then `place_camera_at_presented_pose`. Settling reads `presented_pose()` every frame. The arrival site is `client_drain_control`, which drains in world-less polls too. `reset_level_scoped_client_state` and `demote_client_state` must leave the store alone.
- Strongest rival: raise the client's stand-in to eye height from its own descriptor and leave the hint to Epic 25 U2, the first plan that needs a pose no player start backs. No wire change, and it fixes the lift on every join. It loses three ways: the eye height would come from the client's local registry, a value the host replicates; on a multi-spawn map the view still faces the first spawn's way after arming, which writes no orientation; and U2 needs the resolver anyway. Settling every spawn view fixes only the SH miss. Other shapes are in `research.md`.
- First slice: the resolver, reservation and install send in the in-memory harness. It is proven by a promotion that spawns at the hinted pose and by a substitution that reserves nothing.
- `main.rs`, `startup/lifecycle.rs`, `net/src/transport.rs` and `netcode/src/seat.rs` are past ~800 lines. Land the resolver and the hint store in their own modules, and touch those files only at call sites.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants), so no manual endianness, prefixes or counts. The variant is appended, following the `JoinSeed` and `SessionRoster` precedent.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Spawn pose hint | `ServerControlMessage`, after `SessionRoster` | Control, host → one client | level identity `String` (the parity identity), eye position `[f32; 3]`, yaw `f32`, pitch `f32` (radians, engine convention) | message not sent |

The client rejects non-finite pose values and keeps only the latest hint.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Spawn pose hint | new `ServerControlMessage` variant; host resolver beside the promotion handler; client hint store on the endpoint | appended variant, Control | none | n/a |

## Open questions
- Which descriptor class and key bag a pose that no placement backs spawns with — **delegated**: the test substitution names one beside its pose; the resolver's later consumer pins its own.
- Which wire constants the hint bumps — **delegated**: follow `networking.md` §Version gates and the revealed-declaration precedent (app protocol bumps; `WIRE_VERSION` by measured layout).
