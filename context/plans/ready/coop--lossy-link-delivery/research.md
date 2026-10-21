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
| `ServerControlMessage::Tuning` | S→C | state, possibly sliced | whole-struct dedupe | Control, ~100 ms (ordering vs. demotion kept) |

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

Selected: every-tick resend for the gameplay families, all on Input; ~100 ms for Control, which keeps tuning. A bulk tuning channel was rejected because Control's ordering keeps tuning from landing after a demotion or relevel (second `/validate-plan`). Rejected: a custom unreliable redundant stream. It would bound delivery of edges, starts and switches by the redundancy window and would need an edge-identity rework (`/validate-plan`, Q3 and Q6).

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

## Switch scoping off Control (read at d6f3471)

- Client→host: `NetClient::send_input` wraps payloads in `ParticipationFrame`; `NetServer::drain_input` drops frames whose epoch differs from the client's current one. `send_switch_declaration` sends on Control, gated only on `active_participation_epoch.is_some()`, with no frame epoch.
- Host→client: `NetClient::drain_input` epoch-filters only `WeaponCues`. `resolve_switch_outcome` refuses an outcome unless it matches the front of the pending chain by `declaration_id` and slot. `demote_client_state` and `reset_level_scoped_client_state` clear `pending_switch_declarations`; `next_switch_declaration_id` is set once at endpoint construction and only increments (`wrapping_add`). A stale outcome therefore meets an empty chain or a newer front.
- Host lane removal: `HostCommandQueues::remove_client` on slot close and pawn loss.

The reload-meter research moved to `ready/coop--timed-slot-projection/research.md` with the scope.

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Loss-off control | Same seed and jitter as the steady run, loss 0. Ordered delivery holds later commands behind a late datagram; a frontier freeze runs one host-only held step | Records no correction above the band. If it does, the cause is jitter, not loss |
| P2 | Single Input loss, resend vs send interval | Command t's datagram lost. The resend rides the next datagram whose send time is at least one resend after t's | Resend ≤ send interval: t rides with t+1 and resolves Real on its own tick. Resend above it (17–20 ms at a 60 Hz poll): t rides with t+2, the host deep-yields Held on t, then stale-drops it |
| P3 | Slide entry lost | The datagram carrying the slide or vault entry command is lost once | The host enters on the same client tick the client did |
| P4 | Switch datagram lost, later start survives | switch(t) sent, start(t+k) sent after it, the switch's datagram lost | One stream: the start waits behind the switch. Today's Control layout: the start resolves first (the networking.md known gap) |
| P5 | Switch and start on one command | declaration(t) goes on Input before command(t). The datagram carrying both is lost once | The switch applies first. The start on t lands after it in the same resolution |
| P6 | Declaration in flight across demote and re-promote | Framed under E, sent; the host retires E and arms E+1; the declaration arrives | The host's Input drain drops it: no lane entry, no outcome. Today it reaches the lane over Control |
| P7 | Dropped declaration still pending on the client | The host drops P6's declaration. The client gets Holding(E), then the E+1 marker, in one Control drain | demote_client_state runs on Holding and clears the chain before the first E+1 declaration. No E+1 outcome waits behind the dropped one |
| P8 | Declaration in the lane at demotion | The lane holds a declaration, and the slot is demoted before its command resolves | The lane goes with the queue. Nothing is applied or answered after re-promotion |
| P9 | Outcome in flight across demotion | The host sends an outcome on Input, unframed. The client handles Holding first | The chain is empty, so it resolves nothing |
| P10 | Stale outcome, newer front | The client declares n under E, is demoted and re-promoted, declares n+1. Outcome n arrives, then n+1 | Outcome n leaves the chain untouched. n+1 resolves |
| P11 | Two edges on one tick | Two presses on t, the datagram lost, then the resend | Both delivered once. renet dedupes by message id, so no copy reaches intake |
| P12 | Two-second outage burst | About 120 commands, 10 probes and the queued edges, starts and switches arrive at once | Depth > INPUT_BUFFER_MAX, so the queue trims to the newest 2. Lanes deliver in tick order. Starts past the 500 ms allowance are refused for lag, none dropped silently. No switch is refused as malformed. The 64-tick prediction history has overflowed, so reconcile trusts the authority |
| P13 | Probes across the outage | The 10 outage probes land in the 16-sample window beside 6 pre-outage samples | The pre-outage minimum holds. Outage samples are gated out and no reseed happens. The offset stays within one tick |
| P14 | Probe resent once | The probe's datagram is lost and renet resends it on a later tick | One echo per sample_id. RTT is at most one resend high, and the ≥ 20 ms gate tolerance admits it |
| P15 | Ack lost, new entity | FullBaseline sent, the client's ack datagram lost once | The host's first delta is no more than one snapshot later than with no loss |
| P16 | Sliced tuning on Control under loss | Tuning slices in flight with some lost, Input commands sent on the same ticks against the shared per-tick byte budget | Every Input message meets the single-loss latency bound |
| P17 | Tuning across demotion | Tuning sent under E, then Holding, then re-promotion and a new payload | The old payload never installs after Holding. The new one installs |
| P18 | Two corrections before decay | Correction A seeds the offset. B lands before A decays, possibly in the same frame | The presented eye is the same before and after B. The offset becomes A's remainder plus B. A teleport-class correction, a first-record adoption or a level reset clears it instead |

Read at d6f3471 (`/review-brief`). P2's first outcome is the chosen one: the gameplay resend is capped at one tick.

## Harness inventory

- Acks applied instantly: `predict_reconcile_harness_test_fixtures.rs` (`LoopbackHarness::apply_acks`) and everything built on it (swim, vault, immersion, fluid tables, teleport, teleport join, boot spawn gate). Also `enemy_replication_harness_test.rs` by default.
- Acks over a conditioned link: `enemy_replication_harness_test.rs::with_ack_link`, and `state_slot_loss_harness_test.rs`, which lacks the newest-snapshot gate.
- Full renet relay: `e18_timed_reaction_replication_harness_test.rs`, `trigger_state_channel_harness_test.rs`, `host_activations/conditioned_tests.rs`. These cover activation and presentation, not movement reconcile.
