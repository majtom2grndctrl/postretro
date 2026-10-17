# coop--lossy-link-delivery — research

Read at b0a5c0f. All claims cite symbols; re-verify before relying on them.

## Input stall mechanics

- Client: one `ClientMessage::Input(InputCommand)` per fixed tick (`client_predict_tick_with_sender`, `NetClient::send_input`). Input is `ReliableOrdered { resend_time: RELIABLE_RESEND = 300 ms }` (`connection_config`). No redundancy anywhere in the stack.
- renet 2.0.0 ordered receive pops only the oldest pending id (reliable.rs). Resend fires when `now - last_sent >= resend_time`. There is no RTT adaptation, no NACK and no fast retransmit.
- Host during a stall (`HostCommandQueues::resolve_tick`):
  - frontier freeze (`hold_without_advance`) for `INPUT_HOLD_TICKS` = 3: Held, cursor not advanced;
  - give-up to Neutral (cursor +1, buildup latch re-arms);
  - buildup withhold, then repeat.
  - `neutral_sim_command` clears wish, crouch and jump.
- Burst: `enqueue` stale-drops ticks at or below the crept-forward cursor. Catch-up keeps `INPUT_BUFFER_TARGET` = 2 when depth exceeds `INPUT_BUFFER_MAX` = 8.
- Stall length: ~270–360 ms for one loss, ~600 ms when the resend is lost too. At 60 datagrams/s and 5% loss, roughly one stall cluster per second.
- Analytic pull-back. Ground coast is `GROUND_STOP_FRICTION` = 6/s. Speeds from content/dev/scripts/player.ts: walk 7 m/s, run 11 m/s. With T′ = stall − 50 ms, gap ≈ v·[T′ − (1 − e^(−6T′))/6]:

  | Stall | Walk | Run | Class |
  |---|---|---|---|
  | 300 ms | 0.84 m | 1.33 m | smoothed, oversized |
  | 400 ms | 1.43 m | 2.24 m | smoothed, oversized |
  | ~650 ms (resend also lost) | 3.1 m | 4.8 m | snap |

  These are not measured. The first Acceptance row measures them.
- Correction bands (`classify_correction`): Ordinary ≤ 0.5 m. Dash ≤ 2 m when the window had Dash, Slide or Vault. OversizedSmoothed < 3 m. Teleport ≥ 3 m.
- The existing gate (`latency_harness_converges_within_tolerance_under_mandated_profile`) asserts only final error ≤ 0.05 m and every correction < 3 m.

## Message inventory (Control and Input today)

| Family | Dir | Shape | Dedupe today | Destination |
|---|---|---|---|---|
| `ClientMessage::Input` | C→S | per-tick stream with embedded edges | ticks: yes. Edges: only strictly newer ticks observed (`observe_reload_level`, press-edge `observe`), which in-order delivery keeps correct. Activation: by token | Input, one-tick resend |
| `ClientMessage::Ack` | C→S | state, incremental lists | monotonic per id | Input, one-tick resend |
| `BaselineRefresh` / `StateBaselineRefresh` | C→S | idempotent request | `(nid, ref)`. Client resends at 200 ms | Input, one-tick resend |
| `ClientMessage::TimeSync` / `ServerMessage::TimeSync` | both | probe / echo | `sample_id` | Input, one-tick resend |
| `ClientMessage::HitDeclaration` | C→S | event, keyed by shot id | yes | Input, one-tick resend |
| `ServerMessage::ShotVerdicts`, `HitRefused` | S→C | event, keyed by shot id | keyed | Input, one-tick resend |
| `ServerMessage::ActivationOutcomes` | S→C | ordered per token | none at intake (`endpoint.rs` push) | Input, one-tick resend |
| `ServerMessage::WeaponCues` | S→C | event | epoch filter only, no dedupe | Input, one-tick resend |
| `ClientControlMessage::SwitchDeclaration` | C→S | ordered event (`declaration_id`, client tick) | host lane by tick | Input (moves from Control) |
| `SwitchAccepted` / `SwitchRefused` | S→C | strictly ordered chain (`resolve_switch_outcome`) | no | Input (moves from Control) |
| Admission, Parity, JoinSeed, Revealed, participation marker, Divergence, Relevel, SessionRoster | both | join/control | various | Control, ~100 ms |
| `ServerControlMessage::Tuning` | S→C | state, possibly sliced | whole-struct dedupe | bulk, 150–200 ms |

Ordering dependencies to keep:
- the participation marker gates Input sends and snapshot acceptance;
- Control order within a client (Admission → Parity → JoinSeed → Revealed);
- tuning is sent immediately before the snapshot (best effort today);
- the ack comes before refresh within one message.

Acks today (`AckMessage`): `latest_snapshot_sequence`, `acked_server_tick`, plus lists of newly applied entity baselines, tombstones and slot baselines, and the faction baseline. The host skips an entity with an in-flight FullBaseline until an ack's sequence covers it (`replication/encode.rs`), so an ack stall freezes new entities. A Delta at the held id is neither re-applied nor re-acked, so a lost ack leaks resends until the entity changes.

## Resend policy

Profile: one-way 45 + U(0,60) ms. RTT ≈ 90–210 ms (~158 ms effective with renet's ack delay). Losses independent at 5%.

| Policy | 0 losses | 1 loss | 2 losses | Copies/msg |
|---|---|---|---|---|
| Fixed 300 ms (today) | 45–105 | 345–422 | 645–739 | ≈1.05 |
| Fixed 100 ms | 45–105 | 145–222 | 245–339 | ≈2.0 |
| SRTT + 4·RTTVAR (~240 ms) | 45–105 | 285–362 | 525–620 | ≈1.05 |
| QUIC-style NACK + PTO | 45–105 | ≈225–305 | ≈405–505 | ≈1.05–1.1 |
| Every tick until acked (16–20 ms) | 45–105 | 62–125 | 79–145 | ≈9–10 |

Selected: every-tick resend for the gameplay families, all on Input; ~100 ms for Control; 150–200 ms for a bulk tuning channel. Rejected: a custom unreliable redundant stream. It would bound delivery of edges, starts and switches by the redundancy window and would need an edge-identity rework (`/validate-plan`, Q3 and Q6).

Upstream cost for Input at a one-tick resend: about RTT ÷ tick + 1 ≈ 9–10 copies of each in-flight command, so about 30 KB/s at 60 Hz with ~50 B commands. It is measured and bounded by the executor.

Bandwidth for every-tick resend through renet channels: each frame with anything unacked emits one extra datagram with ≈57 B overhead. At 10 messages/s that is ≈8 KB/s, small next to 30 Hz snapshots. Folding a family into the command stream removes the per-datagram overhead.

renet 2.0.0 facts:
- `resend_time` is per channel, fixed at construction, and identical for every connection (`RenetServer` clones one config).
- `rtt()` is exposed. There is no RTTVAR.
- Acks are redundant: ranges repeat until their ack packet is itself acked.
- Unreliable messages over 1200 B are sliced, and losing any slice loses the message.

Sources (techniques public; no current encumbrance found; not legal advice):
- Quake 3 sends all unacked reliable commands each packet, and the client duplicates up to 5 past inputs (`cl_packetdup`). id-Software/Quake-III-Arena, GPL code; technique not copied, prior art 1999.
- Gaffer On Games, "Reliable Ordered Messages"; yojimbo (BSD-3, `messageResendTime = 0.1`).
- Tribes / Torque "Notify" ack bitfield: opentnl.sourceforge.net.
- RFC 6298 (TCP RTO), RFC 9002 (QUIC loss recovery). ENet (MIT), KCP (MIT), Valve GameNetworkingSockets (BSD-3; NACK after 3 ms).
- Rocket League redundant input history (developer forum, secondhand).
- Patent hits: US7013419B2 (expired) and US10369465B2 (expired). US20170034234A1 covers streaming-specific redundant command packets, status unknown, and the claims look narrow.

## Suspects cleared (PR #601 follow-up audit)

- Damaged-enemy rosters (`ClientOverlayFactState::receive`): revision newest-wins, complete state. Fine.
- Tombstones and projectile terminal pose (`baseline_acked_by_all_recipients`, `prune_acked_tombstones`): `>=`, monotonic. Fine.
- Mover and teleporter state: arrive only through `apply_delta`'s newest-wins gate. Mover ticks compare serially. Fine.
- Owner-private slots: same loop as shared slots (`ClientStateApply::apply_snapshot_state`). Fine.
- Carried magazine and reserve: no client reader. `client_reload.rs` reads `ReplicatedWeaponProjection`. No defect.
- Latent (not this brief): host ack apply uses plain `>`, not serial. It only matters past a `u32` wrap (~days at high churn). The entity baseline allocator's `wrapping_add` can hand out sentinel 0.

## Reload meter

- `hud.ts` `reloadMeter` binds `player.reloadProgress` with no tween, and visibility follows `player.reloadActive`.
- On a connected client, `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon` skips reload publish. The value is the host's `WeaponComponent::reload_feedback_sample`: an endpoint stream, else `1 - remaining/total`. It is sent as `ReplicatedWireShape::WieldableSlotNumber` at `SNAPSHOT_TICK_INTERVAL` = 2 (~30 Hz, unreliable).
- `ClientStateApply` writes it straight into the store. Jitter reorders snapshots, and newest-wins drops the late ones. A loss leaves a ~66 ms hold.
- `WEAPON_COOLDOWN_SLOT` (`player.weaponCooldownMs`) shares the wire shape, so the executor checks whether any HUD binds it.
- Host-time anchor precedent: `PresentationBindings::accept_presentation_clock` / `presentation_host_time` (client/bindings.rs), consumed by mesh_anim.rs `rebase_time`.

## Harness inventory

- Acks applied instantly: `predict_reconcile_harness_test_fixtures.rs` (`LoopbackHarness::apply_acks`) and everything built on it (swim, vault, immersion, fluid tables, teleport, teleport join, boot spawn gate). Also `enemy_replication_harness_test.rs` by default.
- Acks over a conditioned link: `enemy_replication_harness_test.rs::with_ack_link`, and `state_slot_loss_harness_test.rs`, which lacks the newest-snapshot gate.
- Full renet relay: `e18_timed_reaction_replication_harness_test.rs`, `trigger_state_channel_harness_test.rs`, `host_activations/conditioned_tests.rs`. These cover activation and presentation, not movement reconcile.
