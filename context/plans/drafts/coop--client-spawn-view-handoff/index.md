# coop--client-spawn-view-handoff

Brief · compact · **gated** · reads: `context/lib/networking.md` §Admission and content parity, §Slot lifecycle, §Session-state ledger, `context/lib/boot_sequence.md` §1, §4 · read at 695763db4 (`feat/preferences-comfort-floor`)

Build after `sh-streaming--reveal-gate-and-warm-horizon` lands. That brief adds the Settling state and publishes level parity at reveal, and this brief builds on both.

## Problem
Raised by the owner while drafting the SH reveal gate. Basis: an anticipated need. On current content it is latent, because every shipped map has one `player_spawn` (see `research.md`). A connected client learns its spawn only from the first baseline after the host promotes it. Until then it settles, reveals and renders from the first `player_spawn` with that spawn's yaw. Arming then moves the camera's position to the pawn and leaves its yaw alone. On a map where the host assigns the client another placement, the client's first controllable frame jumps to a view it never settled, which may show ambient-floor SH, and faces the first spawn's direction. When done: a connected client settles and reveals at the eye pose its pawn will arm at, facing that placement's authored direction. Arming causes no visible jump, and no in-play SH miss at the spawn view.

## Decisions
- **Host-sent spawn pose.** The host assigns a seat's placement at the first moment it can name the level to that seat: at host install for every admitted seat, and at admission for a seat admitted while a level is installed. It then sends that seat the spawn eye pose on Control, tagged with the level identity. Promotion reuses the assignment, since `SeatTable::assign_placement` returns a seat's existing placement within a level. The client cannot work the placement out itself. Assignment depends on the host's cursor and live occupancy (`plans/done/E15--seat-session-identity-roster` AC-SEAT-1).
- **Pose, not placement index.** `level_content_digest` does not cover `player_spawn` placements, and the hint arrives before parity is proven. An index could therefore resolve to a different spawn on the client. The host includes the eye height it resolves for that placement's descriptor, the same value the tuning payload carries later. The client uses the host's numbers (`networking.md` §What gates, and what replicates instead).
- **Advisory, never a gate.** A missing, late or mismatched hint falls back to today's first-spawn stand-in. The hint never delays reveal and never extends or resets the Settling timeout. The first baseline stays the only authority on the pawn's pose.
- **Stand-in camera follows the hint while unarmed.** The client applies a hint whose level identity matches its installed or loading level:
  - at Settling entry;
  - on arrival during Settling, which retargets the settle view;
  - while it is Running with no armed pawn.

  The Running case covers a host restart: the client does not reload for it (`relevel_is_already_selected`), is demoted, and is later re-promoted at a new placement.
- **The hint survives the client's unload.** The host sends it right after Relevel on the same reliable channel. The client therefore receives it before its queued unload runs. The client keeps only the latest hint, matched by identity, so the level-scoped client reset must not clear it.
- **Readiness stays in the parity declaration.** The SH brief publishes the client's declaration at reveal. The host spawns the client's pawn at promotion, so no pawn exists while its player is on the splash. The design adds no readiness message and no host-side pawn hold. The rival is a separate "ready" signal. It needs a pawn-hold state, or it breaks "any entry to participating spawns its pawn" (`networking.md` §Slot lifecycle), and nothing needs it yet. It can be added later without a one-way door. See Open questions.
- **Reveal at settle, not at arm.** Between reveal and arming there is one round trip plus a snapshot interval. During it the client shows the correct spawn view with no pawn, and it sends no input, because it has no participation epoch yet. See Open questions.
- **Layer placement.**
  - The net crate carries the pose as plain `f32` values and stays glam-free and registry-blind.
  - Resolving a placement to a pose is engine host work, done beside the promotion handler.
  - The hint store is client endpoint state.
- **Non-goals.**
  - The placement-occupancy defect in `research.md`. It is its own brief, and it blocks this brief's manual proof.
  - A waiting or divergence overlay for a client that never gets promoted. That is UI policy, and the divergence diagnostic already covers the cause.
  - A rule that derives placement from the seat. It conflicts with E15's occupancy-aware assignment.
  - SH prefetch beyond moving the stand-in camera.

## Acceptance
### Automated
**Host**
- [ ] On host install, each admitted seat is assigned a placement and sent a hint before its promotion. At promotion, its pawn spawns at the hinted placement.
- [ ] A seat admitted while a level is installed receives Relevel and then the hint, in that order.
- [ ] With free placements available, two admitted seats get distinct hinted placements, and neither matches a live pawn's placement.
- [ ] A level with no `player_spawn` sends no hint, and the client's pawn arms as it does today.

**Client**
- [ ] A hint that arrives in the same control drain as Relevel still applies to the new level after the old level unloads.
- [ ] A hint for a different level identity is ignored, and the stand-in stays at the first spawn.
- [ ] A hint that arrives during Settling retargets the settle view. The Settling timer does not reset.
- [ ] A hint with a non-finite component is rejected before it is applied.
- [ ] On the frame the pawn arms, the camera position is within 1 cm of the pre-arm stand-in, and its yaw equals the placement's authored yaw.
- [ ] Host restart without a client reload: the hint moves the unarmed client's camera to its new placement before the client is re-promoted.
- [ ] Wire: the new message survives an encode/decode round trip, and a peer on the previous wire constants is refused at gate 1.

### Manual
- [ ] Loopback co-op on a fixture with more than one `player_spawn`, where the client's placement is not index 0. The client's first world frame is at its own spawn, facing the authored direction, with no ambient-floor SH, and arming causes no jump. Precondition: the placement defect is fixed.

## Path
- Host assignment: `SeatTable::assign_placement` and `occupied_live_placements`. The promotion handler in the host net frame in main.rs is the only caller today. Pre-assignment must run after `capture_player_spawn_placements` binds the host's own pawns during install (the `install_world_cpu` hook in `startup/lifecycle.rs`). Admission-time sends hook the `admit_or_reclaim` sites, in both `poll_world_less_transport` and the Running host poll, because a joiner can arrive during the host's Settling.
- Send order: `NetServer::set_relevel_catalog_id` / `send_relevel` send Relevel. Send the hint after it, per recipient. An uncatalogued host sends no Relevel, but still sends the hint.
- Client stand-in: the camera teleport from `products.first_spawn` in `install_level_payload`. `follow_camera_to_local_pawn` writes position only. `client_drain_control` is the arrival site.
- Precedent for an appended Control variant: `ClientControlMessage::JoinSeed`, which advanced `WIRE_VERSION`.
- First slice: host pre-assignment and hint send, plus client apply at Settling entry. Prove it with the in-memory harness on a two-placement fixture before touching the Running-unarmed path.

## Wire format
Bitcode owns the layout (`networking.md` §Wire/codec invariants). A new `ServerControlMessage` variant, appended after `SessionRoster`, on the Control channel, host to one client. Fields: the level identity string (the same string parity uses), eye position `[f32; 3]`, yaw `f32`, pitch `f32`. There is no sentinel: an absent hint is a message not sent. The client rejects non-finite values. The version constant bump follows the `JoinSeed` precedent.

## Boundary inventory
| Name | Rust | Wire | Script |
|---|---|---|---|
| Spawn pose hint | new `ServerControlMessage` variant, engine resolver beside the promotion handler | bitcode-appended variant, Control | none |

## Open questions
- Readiness stays folded into the parity declaration, which makes "participating" also mean "revealed". The alternative is a separate readiness signal, now or later. Recommend folded, recorded in `networking.md` §Admission and content parity when the SH brief is promoted — owner — **blocks build**
- Reveal at settle, with a pawnless gap of about one round trip, or keep the splash until the pawn arms. The second option needs snapshot apply during Settling. Recommend reveal at settle — owner — **blocks build**
- The placement-occupancy defect (`research.md`) must be fixed before the manual row can pass. Sequence that fix first, as its own brief — owner — **blocks build**
- Whether the new variant bumps `WIRE_VERSION`, as the `JoinSeed` precedent did, or `PROTOCOL_ID`, as `networking.md` §Two-gate handshake words it — **delegated**
