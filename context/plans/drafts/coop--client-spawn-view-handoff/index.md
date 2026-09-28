# coop--client-spawn-view-handoff

Brief · compact · **gated** · reads: `context/lib/networking.md` §Slot lifecycle, §Two-gate handshake, §Session-state ledger, `context/lib/boot_sequence.md` §1, §4 · read at 3f070e32d (`feat/preferences-comfort-floor`)

**Sequence: 4 of 4.** Build after `sh-streaming--reveal-gate-and-warm-horizon` (3 of 4) and `coop--spawn-placement-occupancy` (2 of 4) land. No dependency on `animated-lightmap-compact-atlas` (1 of 4). The SH brief provides Settling, its eye-pose chokepoint, and the revealed hold that promotion waits on. The occupancy brief makes a free placement reachable; without it the manual row cannot pass.

## Problem
Raised by the owner while drafting the SH reveal gate. Basis: an anticipated need. On current content it is latent, because every shipped map has one `player_spawn` (see `research.md`). A connected client learns its spawn only from the first baseline after the host promotes it. Until then it settles, reveals and renders from the first `player_spawn` with that spawn's yaw. Arming then moves the camera's position to the pawn and leaves its yaw alone. On a map where the host assigns the client another placement, the client's first controllable frame jumps to a view it never settled, which may show ambient-floor SH, and faces the first spawn's direction. When done: a connected client settles and reveals at the eye pose its pawn will arm at, facing that placement's authored direction. Arming causes no visible jump and no in-play SH miss at the spawn view; without a hint, arming still faces the pawn's direction.

## Decisions
- **Host-sent spawn pose.** The host assigns a seat's placement at the first moment it can name the level to that seat — host install for every admitted seat, admission for a seat admitted mid-level — and sends that seat its spawn eye pose on Control, tagged with the level identity. Promotion reuses it (`SeatTable::assign_placement` is sticky within a level), and a pre-assigned placement counts as occupied, extending `coop--spawn-placement-occupancy`'s rule. The client cannot derive it: assignment depends on the host's cursor and live occupancy (`plans/done/E15--seat-session-identity-roster` AC-SEAT-1).
- **Placement binds before promotion.** This forecloses choosing a spawn at promotion time: a later policy such as spawning near teammates would need a fresh hint and would reintroduce the jump. Undo cost: moving assignment back to promotion drops the hint's value.
- **Pose, not placement index.** `level_content_digest` does not cover `player_spawn` placements, and the hint arrives before parity is proven, so an index could resolve to a different spawn on the client. The pose carries the eye height the host resolves for that placement's descriptor, the value the tuning payload carries later (`networking.md` §What gates, and what replicates instead).
- **Advisory, never a gate.** A missing, late or mismatched hint falls back to today's first-spawn stand-in. It never delays reveal, never extends or resets the Settling timeout, and never enters the revealed term. The first baseline stays the only authority on the pawn's pose.
- **Stand-in camera follows the hint while unarmed.** The client applies a hint whose identity matches its installed or loading level at Settling entry, on arrival during Settling (retargeting the settle view), and while Running unarmed — the host-restart case, which the client does not reload for (`relevel_is_already_selected`). It keeps only the latest hint, and the level-scoped client reset does not clear it, so a hint sent right after Relevel survives the unload Relevel queues.
- **Arming adopts the pawn's replicated facing.** When no matching hint placed the stand-in camera, arming sets the camera's yaw from the pawn's replicated rotation (`WireTransform.rotation`), which the host spawns at the placement's authored rotation. Today arming writes position only (`follow_camera_to_local_pawn`), so the client keeps the first spawn's yaw.
- **Layer placement.** The net crate carries the pose as plain `f32`, glam-free and registry-blind. Engine host work resolves a placement to a pose beside the promotion handler. The hint store is client endpoint state; it survives a level change, so `networking.md` §Session-state ledger lists the client's latest hint, at promotion.
- **Non-goals.**
  - A rule that derives placement from the seat. It conflicts with E15's occupancy-aware assignment.
  - SH prefetch beyond moving the stand-in camera.

## Acceptance
### Automated
**Spawn pose hint (host)**
- [ ] On host install, each admitted seat is assigned a placement and sent a hint before its promotion. At promotion, its pawn spawns at the hinted placement.
- [ ] A seat admitted while a level is installed receives Relevel and then the hint, in that order.
- [ ] Two seats hinted before either spawns get distinct placements, neither a live pawn's, while free placements remain.
- [ ] A seat reclaimed after its client disconnected while held keeps its pre-assigned placement.
- [ ] A level with no `player_spawn` sends no hint, and the client's pawn arms as it does today.

**Spawn pose hint (client)**
- [ ] A hint in the same Control drain as Relevel still applies to the new level after the old level unloads.
- [ ] A hint for a different level identity is ignored, and the stand-in stays at the first spawn.
- [ ] A hint during Settling retargets the settle view. The Settling timer does not reset.
- [ ] A client that receives no hint reveals at the first-spawn stand-in with no added hold.
- [ ] A hint with a non-finite component is rejected before it is applied.
- [ ] Host restart with a Running client: the hint moves its unarmed camera to the new placement before re-promotion.

**Arming**
- [ ] With a matching hint applied, on the frame the pawn arms the camera position is within 1 cm of the pre-arm stand-in, and its yaw equals the placement's authored yaw.
- [ ] With no matching hint applied, on the frame the pawn arms the camera yaw equals the pawn's replicated facing.

**Wire**
- [ ] The hint survives an encode/decode round trip, and a peer on the previous wire constants is refused at gate 1.

### Manual
- [ ] Loopback co-op on a fixture with more than one `player_spawn`, where the client's placement is not index 0. The client's first world frame is at its own spawn, facing the authored direction, with no ambient-floor SH, and arming causes no jump.

## Path
- Hint: `SeatTable::assign_placement`, `occupied_live_placements`; the promotion handler in main.rs's Running host net frame is the only caller. `assign_placement` already counts other seats' assignments as occupied only when the seat has no client binding; a pre-assigned, client-bound seat without a pawn is the case to add. Pre-assign at install through the seat-table chokepoint `coop--spawn-placement-occupancy` adds for seat 0, which also deletes `capture_player_spawn_placements`. Admission sends hook `admit_or_reclaim` in both `poll_world_less_transport` and the Running host poll. Send after `NetServer::send_relevel`, per recipient; an uncatalogued host still sends the hint.
- Client stand-in: the `first_spawn` camera write in `install_level_payload`; the SH brief's eye-pose chokepoint is where the hint enters; `client_drain_control` is the arrival site. Facing: `maybe_arm_local_pawn` reports the armed pawn; the engine arm site reads its applied transform.
- Strongest rival: settle every spawn view. It fixes the SH miss at whichever spawn is assigned but not the position and yaw jump. Other shapes: `research.md`.
- First slice: pre-assignment and the hint send on the host, proven in the in-memory harness by a promotion that spawns at the hinted placement.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants); no manual endianness, prefixes or counts. The addition is appended, following the `JoinSeed` and `SessionRoster` precedent.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Spawn pose hint | `ServerControlMessage`, after `SessionRoster` | Control, host → one client | level identity `String` (the parity string), eye position `[f32; 3]`, yaw `f32`, pitch `f32` | message not sent |

The client rejects non-finite pose values and keeps only the latest hint.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Spawn pose hint | new `ServerControlMessage` variant; engine resolver beside the promotion handler; client hint store | appended variant, Control | none | n/a |

## Open questions
- Whether adopting the pawn's replicated facing at arm snaps an unarmed client's free-look, and whether it should — **delegated**
- Which wire constants the hint bumps — **delegated**: follow the rule the SH brief's build chose for its revealed declaration.
