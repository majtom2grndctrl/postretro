# coop--client-spawn-view-handoff

Brief · compact · reads: `context/lib/networking.md` §Slot lifecycle, §Two-gate handshake, §Session-state ledger, §Version gates, `context/lib/boot_sequence.md` §1 (Settling, Presented pose, `--connect` client) · read at b215170

**Sequence:** prerequisite of Epic 25 U2 (`drafts/E25--landmark-level-change/`).

## Problem
Raised by the owner while drafting the SH reveal gate. Basis: a defect read in source (`research.md`). A connected client's first world view is not its spawn view. The host assigns the client's spawn at promotion and sends nothing before the pawn's first baseline, so the client settles and reveals at a stand-in: the first player's `player_spawn`, at capsule centre, with that spawn's yaw and pitch. Arming moves the camera to the pawn's eye and keeps yaw and pitch. Every join lifts the view on its first armed frame, and where the client's placement is not the first player's, the view jumps to a spot it never settled, facing the wrong way, possibly with ambient-floor SH. On the usual `--connect` path the frontend menu, pushed as the fallback for the host's load, stays over the installed level, and the client's own level verbs cannot dismiss it. When done: on an ordinary join the client settles, reveals and arms at the eye position, yaw and pitch its pawn spawns with, with no menu over the host's level, no visible jump at arming and no in-play SH miss at the spawn view.

## Decisions
- **One host-side spawn-pose resolver.** The host takes each remote seat's spawn pose (eye position, yaw, pitch) from one resolver, and the hint and the promotion spawn both read its recorded result. Today it returns the seat's assigned `player_spawn`: the standing position plus the placement descriptor's eye height, and yaw and pitch from the placement's angles. A later consumer may return a pose that no placement backs, or a single-use source; once a seat's spawn consumes one, the seat resolves afresh to a player start.
- **Resolve when the host first names an installed level to the seat.** That is host install for every client-bound seat, after the host's own pawn binds its placement. It is admission for a seat admitted while a level is installed, Settling included; a seat admitted with no level installed, a host's frontend backdrop included, waits for the next install.
- **A placement-backed pose reserves its player start through the seat's placement assignment.** Other seats' placement checks and respawn (`done/E18--player-life-states` AC13) already treat an assignment as taken; this brief makes it at resolution, so it reserves before a pawn exists, until unload. A seat whose pose no placement backs holds none.
- **The shared occupancy rule still never spawns a pawn inside a live pawn (owner).** If a live pawn stands on a seat's reserved start when it promotes, the host re-resolves, reserves the new placement and sends a fresh hint. That client's view jumps once on arming.
- **The hint is a pose, not a placement index.** `level_content_digest` does not hash `player_spawn` placements, and the hint arrives before parity is proven, so an index could name a different spawn on the client. It is tagged with the install it describes, the pair `Revealed` declares: parity identity, and host level life on a catalog level. Relevel goes first; an uncatalogued host still sends a hint, and a level with no `player_spawn` sends none.
- **The hint precedes promotion for a client that reveals after admission (owner).** On Control, that seat's hint for an install comes before the marker arming its participation epoch there. Elsewhere the view moves once: the overlap re-resolution, whose hint follows the marker; a client that revealed a matching uncatalogued level before connecting and is promoted in its admission batch; and a re-promotion without reload, which returns the view to the reserved spawn at arming.
- **Advisory, never a gate.** A missing, late or mismatched hint leaves today's first-spawn stand-in in place. It never delays reveal, never touches the Settling timeout, and never enters the revealed term. The first baseline stays the only authority on the pawn's pose.
- **The unarmed stand-in takes the hint's eye, yaw and pitch.** The client applies a hint matching its installed or installing level at install, and on arrival during Settling or while Running unarmed. It keeps only the latest hint, through the level-scoped reset and demotion, until it leaves the session. Once the pawn arms, a hint moves nothing.
- **A connected client following its host's level clears every frontend menu layer (owner).** A host-named level is one the client loads because its host's relevel requested it, or an in-flight load of that level the relevel adopted; an id match alone never counts. Its install clears the menu before the presented pose resolves. A backdrop already installed when the relevel names it reloads as the host's level, so a backdrop never serves as a played level.
- **A failed host-named load puts the menu back (owner).** That covers a load that fails and a later relevel naming a catalog id the client lacks, which unloads the stale level. The fallback presents the menu without loading a backdrop. Single-player and host are unchanged.
- **A host's frontend backdrop names no level (owner).** A host at its frontend sends no Relevel and no hint and promotes no one; joiners wait at their own menu until the host starts a level.
- **Arming moves position only.** Arming writes no orientation today and keeps writing none, so the client arms with the stand-in's yaw and pitch. The replicated rotation is not a fallback: once the host plays a client's commands, it rewrites that pawn's rotation yaw-only from the client's own facing.
- **Layer placement.** The net crate carries the pose as plain `f32`, glam-free and registry-blind. The resolver takes caller-resolved placement sites, so U2 can supply its own fit test, and the seat table stays descriptor-blind.
- **Non-goals.**
  - SH prefetch beyond moving the stand-in camera. Settling at the right view is this brief's whole SH fix.
  - The client's view at a mid-level respawn. E18 owns respawn; the roadmap carries it as a candidate draft.
  - A connected client's own frontend verbs (New game, quit). `drafts/coop--client-leave-session` owns them.

## Acceptance
### Automated
**Resolver and reservation (host)**
- [ ] On host install, each client-bound seat receives that install's Relevel and then its hint, before its promotion. At promotion, the pawn's eye is within 1 cm of the hinted eye, and its spawn rotation carries the hinted yaw and pitch (`research.md` O8).
- [ ] A test resolver returns a pose that no placement backs for one seat. The hint, that client's stand-in camera and its spawned pawn all use that pose. The seat reserves no placement, and a seat resolved after it takes the lowest free placement as if the first seat were absent.
- [ ] A respawn never takes a placement reserved for another seat while a free one remains, including a seat resolved but not yet promoted. A seat holding no placement respawns at the first free pick.
- [ ] A single-use test source: after the seat's spawn consumes it, the seat's re-promotion, and its rejoin within the hold, resolve to a player start, which it then reserves; the hint names that start and reaches the client before that re-promotion's participation marker (`research.md` O6).
- [ ] A seat's spawn stays at its hinted pose when another seat takes a placement or frees one between its hint and its promotion.
- [ ] With a live pawn standing on a seat's reserved start at promotion, the seat's pawn spawns at a free placement clear of every live pawn, and its client receives a hint naming that placement.
- [ ] Two seats resolved before either spawns get distinct placements, neither the host's nor a held seat's, while free placements remain. With every placement taken, they share placements round-robin, each overflowing seat warning once, as today.
- [ ] A seat demoted mid-level keeps its reservation, and so does a seat held after its client disconnects. A joiner resolved meanwhile does not take it, and the returning seat spawns at its hinted pose; a client demoted without reload receives no fresh hint, and its view returns to that pose when it re-arms (`research.md` O10).
- [ ] A seat admitted while a level is installed receives Relevel and then its hint, whether the host is Running or still Settling. On an uncatalogued level it receives the hint alone.
- [ ] A seat admitted while the host is loading its next level receives no hint at admission. When that level installs, it receives Relevel and then a hint resolved against the new level's placements, the same pose a seat admitted after install would get (`research.md` O3).
- [ ] For a client that reveals after admission, on a catalog host and on an uncatalogued host whose client reveals after connecting, the seat's hint for that install precedes its participation marker on Control.
- [ ] A client that revealed a matching uncatalogued level before connecting, and is promoted in its admission batch, receives its hint after the marker and applies it while unarmed (`research.md` O11).
- [ ] A level with no `player_spawn` sends no hint, and the client joins as it does today.
- [ ] The host's pawn keeps install's first-player spawn, and no joiner is assigned that placement.

**Client stand-in**
- [ ] With a matching hint, the client installs, settles and reveals at the hinted eye position, yaw and pitch, not at the first player start. On a fixture whose spawn has nonzero pitch, the revealed pitch equals it.
- [ ] A client whose local `player_spawn` list differs in order from the host's still settles at the host's pose.
- [ ] A hint in the same Control drain as Relevel still applies to the new level after the old level unloads.
- [ ] A holding notice that reaches the client after its hint, the ordinary revealed hold included, leaves the hint in place, and the client installs and settles at the hinted pose (`research.md` O4).
- [ ] A hint naming a different level identity, or an earlier level life of the same catalog level, is ignored, and the stand-in stays at the first spawn.
- [ ] After a host same-map restart, the client reloads and settles at the hint naming the new level life.
- [ ] A hint during Settling retargets the settle view, and the Settling timer does not reset.
- [ ] A hint that first arrives on the reveal frame leaves that frame at the settled pose; the camera moves to it on a later frame while the pawn is unarmed (`research.md` O7).
- [ ] A Running unarmed client that receives a fresh hint for its installed level moves its camera to it before arming.
- [ ] A hint for the installed level that arrives after the pawn arms changes neither the camera's position nor its yaw and pitch, including the fresh hint an overlap re-resolution sends after the participation marker (`research.md` O5).
- [ ] A client that receives no hint reveals at the first-spawn stand-in with no added hold.
- [ ] A hint with a non-finite component is rejected before it is applied.

**Frontend menu**
- [ ] A connected client whose frontend menu is up when it installs its host's level reveals at the hinted pose with the menu gone. This holds on a `--connect` boot with no map argument and on one whose map argument the host's map replaced.
- [ ] A connected client with a submenu open over its frontend menu installs its host's level with neither showing (`research.md` O15).
- [ ] A connected client whose frontend backdrop load of the host's catalog level is still in flight when the relevel arrives installs it as the host's level, with the menu gone, settling at the hint (`research.md` O2).
- [ ] A connected client whose backdrop of the host's catalog level is already installed, Settling or Running, reloads it when the relevel arrives, and settles and reveals at the hint (`research.md` O1, O12, O13).
- [ ] A connected client loading its host's level with no matching hint takes the first-spawn stand-in, not the menu pose (`research.md` O14).
- [ ] A host load that fails, or names a catalog id the client lacks, leaves the client at the frontend menu with no backdrop load, on the first host-named load and on a later one after install cleared the menu (`research.md` O9, O16, O18).
- [ ] A connected client that loads its frontend backdrop of its own accord keeps the menu, even when the backdrop is the level its host last named (`research.md` O17).
- [ ] A client admitted while the host sits at its frontend receives no Relevel and no hint, stays at its menu, and is not promoted. When the host starts a level, the client receives Relevel and then its hint (`research.md` O19).
- [ ] A frontend backdrop the host has not named presents the menu pose, and so does a single-player install with the menu pushed.

**Arming**
- [ ] With a matching hint applied, on the frame the pawn arms the camera is within 1 cm of the pre-arm stand-in, with yaw and pitch unchanged. This holds on a single-spawn level and on a level where the client's placement is not the first player's.
- [ ] Regression guard: a client that turns its view while unarmed keeps that yaw and pitch when it arms.

**Wire**
- [ ] The hint survives an encode/decode round trip with its identity, level life, eye position, yaw and pitch. A peer on the previous app protocol is refused at gate 1, no shipped Control encoding changes, and the net crate still has no internal dependency.

### Manual
- [ ] Loopback co-op on `content/dev/maps/coop-presentation-test.map`, with the client's placement given a yaw distinct from the first player's (every spawn there faces the same way today), launched as `coop-presentation-test.README.md` documents and again with no client map argument. Each time the client's first world frame is at its own spawn eye, facing the authored direction, with no menu and no ambient-floor SH, and arming causes no jump.

## Path
- Resolver and reservation: `SeatTable::assign_joiner_placement(registry, seat, sites)` with `PlacementSite` from `joinable_sites`, and `PlacementOccupancy` (`crates/sim/src/placement_occupancy.rs`). Its recalled-assignment early return already gives way to a live overlap. Its carried-loadout check passes from seat mint. `host_spawn_points` outlives unload, so resolution needs an installed-level gate. Pose inputs: `standing_position`, the descriptor's `movement.capsule.eye_height`, and the placement's `angles` (x pitch, y yaw). Respawn reads `RespawnPlacements.seats` from `respawn_placements::build`/`publish`; republish after each resolution. The promotion handler is the `SlotEvent::Participating` arm in main.rs's Running host net frame, which calls `assign_joiner_placement`, then `host_handle_accept_descriptor_at_placement`. That takes a placement index, and `spawn_descriptor_instance` builds the transform from the placement, so a pose that no placement backs needs a spawn entry that takes the pose and a descriptor class.
- Send sites: `install_level_identity` sends Relevel through `set_relevel_catalog_id` before `install_level_segment_b` binds the host's pawn (`bind_installed_first_player`); `install_level_post_load_passes` sets `host_spawn_points`, so resolve and send after both. The participation marker for a same-batch reveal goes out inside `NetServer`'s poll (`reevaluate_parity` → `begin_participation`), before the engine sees the admission. At admission, `NetServer` sends Relevel inside handshake processing. The engine's `admit_or_reclaim` sites in `poll_world_less_transport` and the Running host poll follow it. The Running poll moves `host_spawn_points` into a local for its duration; the resolver reads that. The epoch marker is `begin_participation`, on Control.
- Docs: promotion recorded this brief's contracts as decided, not yet built, in `networking.md` §Slot lifecycle and §Session-state ledger and `boot_sequence.md` §1 Presented pose. Landing drops those markers and adds the hint to §Version gates. `docs/` notes the path-host join's one view move.
- Client: `install_level_camera_pose` (`startup/lifecycle_install_presentation.rs`) writes the stand-in, then `place_camera_at_presented_pose`. Settling reads `presented_pose()` every frame. The arrival site is `client_drain_control`, which drains in world-less polls too. The client's followed life is `last_relevel` (`follow_relevel_catalog`).
- Menu: `start_boot_destination` pushes it for a queued host load; the dev backdrop is `combat-demo`, a playable catalog level. Adoption is the `active && first` branch and the in-flight early return of `follow_relevel_catalog`; `last_relevel` is never cleared, so host-named needs its own marker on the load request. Re-present with `present_frontend_menu`, not `populate_frontend`, which enqueues a backdrop; `finish_level_failure` is the failure landing. The host's backdrop install publishes parity and the relevel id in `install_level_identity`. `apply_frontend_menu_camera_pose_if_present` writes the camera's yaw and pitch every frame while it is present. The only clears are behind the client-refused level verbs (`refuse_client_level_verb`).
- Strongest rival: present at arm. Keep the client loading until its pawn's first record arrives, then settle from the real eye. No wire change, and the pose is exact, but the host's pawn exists while its player cannot see, which `boot_sequence.md` §1 forbids. Other shapes are in `research.md`.
- First slice: the resolver, reservation and install send in the in-memory harness. It is proven by a promotion that spawns at the hinted pose and by a substitution that reserves nothing.
- `main.rs`, `net/src/transport.rs` and `netcode/src/seat.rs` are large. Land the resolver and the hint store in their own modules, and touch those files only at call sites.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants), so no manual endianness, prefixes or counts. The variant is appended, following the `SessionRoster` precedent. An appended variant moves the app protocol id (`networking.md` §Version gates); `WIRE_VERSION` moves only if the append-layout guard shows a shipped encoding changed.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Spawn pose hint | `ServerControlMessage`, after `SessionRoster` | Control, host → one client | level identity `String` (the parity identity), level life `Option<u64>` (`None` on an uncatalogued level), eye position `[f32; 3]`, yaw `f32`, pitch `f32` (radians, engine convention) | message not sent |

The client rejects non-finite pose values.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Spawn pose hint | new `ServerControlMessage` variant; host resolver beside the promotion handler; client hint store on the endpoint | appended variant, Control | none | n/a |

## Open questions
- Which descriptor class and key bag a pose that no placement backs spawns with — **delegated**: the test substitution names one beside its pose; the resolver's later consumer pins its own.
