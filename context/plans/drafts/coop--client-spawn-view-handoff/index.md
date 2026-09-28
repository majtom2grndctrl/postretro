# coop--client-spawn-view-handoff

Brief · compact · **gated** · reads: `context/lib/networking.md` §Admission and content parity, §Slot lifecycle, §Two-gate handshake, §Session-state ledger, `context/lib/boot_sequence.md` §1, §4 · read at 05ab2d3da (`feat/preferences-comfort-floor`)

Build after `sh-streaming--reveal-gate-and-warm-horizon` and `coop--spawn-placement-occupancy` land. The SH brief adds Settling and publishes parity at reveal as an interim, which this brief replaces. The occupancy brief makes a free placement reachable; without it the manual row cannot pass.

## Problem
Raised by the owner while drafting the SH reveal gate. Basis: an anticipated need. On current content it is latent, because every shipped map has one `player_spawn` (see `research.md`). A connected client learns its spawn only from the first baseline after the host promotes it. Until then it settles, reveals and renders from the first `player_spawn` with that spawn's yaw. Arming then moves the camera's position to the pawn and leaves its yaw alone. On a map where the host assigns the client another placement, the client's first controllable frame jumps to a view it never settled, which may show ambient-floor SH, and faces the first spawn's direction. When done: a connected client settles and reveals at the eye pose its pawn will arm at, facing that placement's authored direction. Arming causes no visible jump, and no in-play SH miss at the spawn view. The host spawns no pawn for a player who cannot yet see.

## Decisions
- **Host-sent spawn pose.** The host assigns a seat's placement at the first moment it can name the level to that seat — host install for every admitted seat, admission for a seat admitted mid-level — and sends that seat its spawn eye pose on Control, tagged with the level identity. Promotion reuses it (`SeatTable::assign_placement` is sticky within a level), and a pre-assigned placement counts as occupied, extending `coop--spawn-placement-occupancy`'s rule. The client cannot derive it: assignment depends on the host's cursor and live occupancy (`plans/done/E15--seat-session-identity-roster` AC-SEAT-1).
- **Pose, not placement index.** `level_content_digest` does not cover `player_spawn` placements, and the hint arrives before parity is proven, so an index could resolve to a different spawn on the client. The pose carries the eye height the host resolves for that placement's descriptor, the value the tuning payload carries later (`networking.md` §What gates, and what replicates instead).
- **Advisory, never a gate.** A missing, late or mismatched hint falls back to today's first-spawn stand-in. It never delays reveal and never extends or resets the Settling timeout. The first baseline stays the only authority on the pawn's pose.
- **Stand-in camera follows the hint while unarmed.** The client applies a hint whose identity matches its installed or loading level at Settling entry, on arrival during Settling (retargeting the settle view), and while Running unarmed — the host-restart case, which the client does not reload for (`relevel_is_already_selected`). It keeps only the latest hint, and the level-scoped client reset does not clear it, so a hint sent right after Relevel survives the unload Relevel queues.
- **Parity is content identity again; readiness is its own signal.** Both peers publish level parity at install, and a slot participates iff parity matches and both peers have revealed the host's installed level. The client declares its revealed level identity on Control (the identity at reveal, none at unload); the host records its own reveal locally, and an entry that skips Settling reveals at install. The host half is load-bearing: a Settling poll's promotion handler spawns no pawn, so a restarted host must not promote a still-ready client before it reveals.
- **The pawn hold is admitted, not a new stage.** A parity-matched slot that is not ready stays admitted, held with a readiness holding cause that, like any hold, retires the client's epoch on demotion. `networking.md` §Slot lifecycle keeps its four stages and "any entry to participating spawns its pawn"; only the predicate sentence gains the readiness term, recorded at promotion. Rival shapes: `research.md`.
- **Readiness lifetime.** A slot's readiness survives parity changes and host level changes; only the client's own declaration or the slot closing changes it. So a mod-digest demotion re-promotes without a fresh reveal, and a restarted host re-promotes a Running client at its own reveal. No readiness timer on either peer: the Settling timeout bounds a live client, and a silent one is held like any parity hold, bounded by keepalive.
- **Reveal at settle, not at arm.** The client reveals when Settling ends and declares ready on that frame, so the pawnless window after reveal is about one round trip plus a snapshot interval; the client sees the right view, sends no input, and nothing acts on its player. Holding the splash until arm would need snapshot apply during Settling, which the world-less poll excludes.
- **Layer placement.** The net crate owns the readiness record and the predicate term, carrying identities as opaque strings; it stays registry-blind and glam-free, and carries the pose as plain `f32`. Engine host work resolves a placement to a pose beside the promotion handler. The engine publishes each peer's reveal from the SH brief's Settling→Running edge. The hint store is client endpoint state.
- **Non-goals.**
  - A waiting or divergence overlay for a held client. That is UI policy; the holding diagnostic covers the cause.
  - A rule that derives placement from the seat. It conflicts with E15's occupancy-aware assignment.
  - SH prefetch beyond moving the stand-in camera.
  - Applying snapshots during Settling.

## Acceptance
### Automated
**Spawn pose hint (host)**
- [ ] On host install, each admitted seat is assigned a placement and sent a hint before its promotion. At promotion, its pawn spawns at the hinted placement.
- [ ] A seat admitted while a level is installed receives Relevel and then the hint, in that order.
- [ ] Two seats hinted before either spawns get distinct placements, neither a live pawn's, while free placements remain.
- [ ] A level with no `player_spawn` sends no hint, and the client's pawn arms as it does today.

**Spawn pose hint (client)**
- [ ] A hint in the same control drain as Relevel still applies to the new level after the old level unloads.
- [ ] A hint for a different level identity is ignored, and the stand-in stays at the first spawn.
- [ ] A hint during Settling retargets the settle view. The Settling timer does not reset.
- [ ] A hint with a non-finite component is rejected before it is applied.
- [ ] On the frame the pawn arms, the camera position is within 1 cm of the pre-arm stand-in, and its yaw equals the placement's authored yaw.

**Parity and readiness**
- [ ] Both peers publish level parity on the install frame. A content-divergent client receives its divergence cause while still in Settling.
- [ ] A parity-matched client in Settling is held: no pawn, no tuning, no snapshot, no epoch marker. The pawn spawns on the host poll that drains its ready declaration.
- [ ] A client whose Settling times out still declares ready on its reveal frame.
- [ ] Ready before promotion: a client that declares ready while parity mismatches is promoted the moment parity matches, with no second declaration.
- [ ] Ready never arrives: the slot stays held with the readiness cause, stays connected, and receives no entity state.
- [ ] A duplicate ready declaration for the current level causes no second participation entry, pawn or epoch.
- [ ] A ready declaration naming a level other than the host's installed one does not promote.
- [ ] Client unload retracts readiness. A re-install of the same level is held until its own reveal, even when the host's control drain collapses the unload and install into one batch.
- [ ] While the host is in Settling, a parity-matched ready client is not promoted. It is promoted on the host's reveal frame, exactly once.
- [ ] Host restart with a Running client: the client is re-promoted at the host's reveal without re-declaring, and the hint moves its unarmed camera to the new placement first.
- [ ] Host restart while the client is still settling: the client is promoted exactly once, after both reveals, in either order.
- [ ] A mod-digest demotion and recovery re-promotes a Running client without a new ready declaration.
- [ ] A client that disconnects while held spawns no pawn; its readiness record is gone, and the reclaimed seat keeps its placement and needs a fresh ready.
- [ ] A readiness-only demotion sends a Holding diagnostic and retires the client's epoch.

**Wire**
- [ ] Both new messages and both new holding causes survive an encode/decode round trip, and a peer on the previous wire constants is refused at gate 1.

### Manual
- [ ] Loopback co-op on a fixture with more than one `player_spawn`, where the client's placement is not index 0. The client's first world frame is at its own spawn, facing the authored direction, with no ambient-floor SH, and arming causes no jump.
- [ ] Loopback co-op across a host `restartLevel` with a long settle: the client never sees its own pawn act before the host reveals.

## Path
- Hint: `SeatTable::assign_placement`, `occupied_live_placements`; the promotion handler in main.rs's Running host net frame is the only caller. Pre-assign after `capture_player_spawn_placements` in the `install_world_cpu` hook. Admission sends hook `admit_or_reclaim` in both `poll_world_less_transport` and the Running host poll. Send after `NetServer::send_relevel`, per recipient; an uncatalogued host still sends the hint.
- Client stand-in: the `first_spawn` camera write in `install_level_payload`; `follow_camera_to_local_pawn` writes position only; `client_drain_control` is the arrival site.
- Readiness: the term joins `parity_cause` / `NetServer::reevaluate_parity`, after the parity checks. The client record mirrors `NetClient::set_level_parity` / `parity_sent` in `queue_control_messages`. `close_slot` clears it. `NetEndpoint::set_level_parity` is the dual-role precedent for the reveal setter; `clear_net_level_parity` retracts both. `client_drain_control` logs holds as content parity at warn; readiness causes should not.
- First slice: the readiness term and client declaration in the net crate, proven with the in-memory harness, before the hint.
- Derivation, rejected shapes and the SH-brief interaction: `research.md`.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants); no manual endianness, prefixes or counts. Every addition is appended, following the `JoinSeed` and `SessionRoster` precedent.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Spawn pose hint | `ServerControlMessage`, after `SessionRoster` | Control, host → one client | level identity `String` (the parity string), eye position `[f32; 3]`, yaw `f32`, pitch `f32` | message not sent |
| Revealed-level declaration | `ClientControlMessage`, after `JoinSeed` | Control, client → host | revealed level identity `Option<String>` | `None` = not revealed |
| Host-not-revealed hold | `HoldingCause`, after `LevelDigest` | inside `Divergence`, host → client | host level identity `String` | — |
| Client-not-revealed hold | `HoldingCause`, after the host variant | inside `Divergence`, host → client | host level identity `String` | — |

The client rejects non-finite pose values. The declaration is re-sent on change, like parity, and the host keeps the last one per slot. Holding-cause order is diagnostic precedence, so both readiness causes rank below every content cause.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Spawn pose hint | new `ServerControlMessage` variant; engine resolver beside the promotion handler | appended variant, Control | none | n/a |
| Revealed-level declaration | new `ClientControlMessage` variant; `NetClient` record and sent flag; `NetEndpoint` reveal setter for both roles | appended variant, Control | none | n/a |
| Readiness holding causes | two appended `HoldingCause` variants | inside `Divergence` | none | n/a |

## Open questions
- Whether the new variants bump `WIRE_VERSION`, as the `JoinSeed` precedent did, or also `PROTOCOL_ID`, as `networking.md` §Two-gate handshake words a vocabulary change — **delegated**
