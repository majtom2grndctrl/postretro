# coop--lossy-link-delivery

Brief · resumable · reads: `context/lib/networking.md` §Channel model, §Wire/codec invariants, §Time sync, §Host input command queue, §Combat authority, §Version gates, §Testing the conditioned link · read at b0a5c0f · direction rechecked at d6f3471 (after `coop--client-inventory-awaits-tuning`, `movement--static-overlap-invariant` and c7752fc landed)

## Problem
Owner playtest under the dev lag proxy (45 ms + 0–60 ms jitter + 5% loss each way) after PR #601: a connected client's own pawn rubber-bands, worst in slides and vaults. Basis: observed defect. Cause: gameplay traffic rides reliable-ordered channels, where one lost datagram stalls everything behind it for at least the 300 ms resend. For movement this means the host holds, neutral-walks, then stale-drops or trims the stalled commands, so the host never simulates a client's real input across a stall. The test harnesses hide it because they send input unordered and apply acks instantly. When done, under the dev profile a client's own movement, including slide and vault entry, matches the host closely enough that only small smoothed corrections occur. A single lost datagram costs gameplay traffic about one tick of delay, not a resend interval. The harness reproduces production channel semantics, so a regression fails a test.

## Decisions
- **Input stays reliable-ordered, with a resend of about one tick.** renet 2.0 resends each unacked message on its own timer, packs the resends into the next packet and dedupes by message id on receipt. At a 16–20 ms resend, every Input datagram therefore carries every command the peer has not acked: Quake-3-style redundancy that keeps exactly-once, in-order delivery.
  - The edge lanes, activation starts and catch-up keep their guarantees unchanged, including "a trim cannot lose a door press or a weapon drop" (`networking.md` §Host input command queue).
  - The channel M15 phase 3 chose stays (`plans/done/M15--p3-movement-prediction-reconciliation`). Only its resend changes: that phase assumed the gap policy absorbs a 300 ms stall, and it does not absorb a stall followed by a burst.
- **Every gameplay family rides Input at that resend.**
  - Client→host: input commands, acks, refresh requests, time-sync probes, HIT declarations, and weapon-switch declarations, which move from Control.
  - Host→client: shot verdicts, HIT refusals, activation outcomes, observer weapon cues, time-sync echoes, and switch outcomes, which move from Control.

  One ordered stream for input and switches closes the Control-versus-Input ordering gaps that `networking.md` §Combat authority names. At a one-tick resend, head-of-line blocking costs about a tick, so splitting ordering domains across channels buys nothing.
- **Switches keep their participation scoping off Control.** Leaving Control drops the epoch-tagged frames that ordered switches against relevel, demotion and divergence. Each direction stays safe without new framing:
  - Client→host: a declaration sent through `send_input` rides the `ParticipationFrame`, so the host's Input drain drops one from an earlier participation. Today's Control send is gated only on an active epoch, so this tightens.
  - Host→client: an outcome from an earlier participation matches nothing. Demotion and level unload clear the pending chain (`demote_client_state`, `reset_level_scoped_client_state`), and `next_switch_declaration_id` is connection-scoped and never resets, so `resolve_switch_outcome` sees an empty chain or a newer front and resolves nothing. Outcomes therefore need no epoch filter like `WeaponCues`; a regression row pins it.
- **Join traffic and tuning stay on Control, at about a 100 ms resend.** Control keeps admission, parity, join seed, revealed, participation, divergence, relevel, roster and the tuning payload. Once switches leave, nothing on Control needs sub-100 ms delivery. Control's ordering is what guarantees a tuning payload never lands after the demotion or level change that retires it. `coop--client-inventory-awaits-tuning` relies on that guarantee, and the tuning payload carries no participation or level framing of its own.
- **No renet fork, no own reliability layer, no new channel.** Channel configuration delivers the policy. Adaptive RTO gains nothing at one-tick resend. `research.md` §Resend policy has the arithmetic and the sources.
- **Time sync keeps its min-RTT window gate.** The gate becomes a jitter filter rather than a stall filter.
- **A new reconcile correction composes with the presentation offset still in flight.** Today it replaces that offset, so a correction landing before the previous one decays makes the camera jump.
- **The harness reproduces production channel semantics.** The predict/reconcile harness family carries input, acks and time sync over the production channel configuration. Its gate counts corrections above the ordinary band, replacing a 3 m ceiling that allows every symptom above. The root cause is the one #601 fixed for deltas: the test link differed from the shipped one.
- **The offset fix rides this brief, but not its gate.** Replacing an in-flight offset is a local reconcile defect with its own cause. It rides here because the same playtest found it, and one conditioned-link manual pass proves both fixes. The reproduction row gates only the transport change.
- **One protocol bump covers the change.** The application protocol id moves for the channel layout and the families that change channel. `WIRE_VERSION` moves for layout. `SNAPSHOT_VERSION` does not move. The previous peer is refused at the handshake.
- **Placement.** Channel layout and resend policy go in `postretro-net`. Send-site routing and switch-lane intake from Input go in `postretro-netcode`. Host gap policy and catch-up stay as they are; they now handle only genuine outages.
- **Non-goals.**
  - Client inventory composition before the tuning payload: `coop--client-inventory-awaits-tuning` owns it.
  - Smooth client display of host-projected timed values, such as the reload meter stepping. The owner reported it in the same playtest, but its cause is sample rate, not loss: `drafts/ui-tween-clock` owns it.
  - Pawns embedded in static geometry after a vault: `movement--static-overlap-invariant` owns it. The defect is in the movement substrate and occurs in every role.
  - Clipping the smoothed camera path against geometry. Corrections become small, and real embeds belong to the sibling brief.
  - Lag compensation and time dilation (`networking.md` §Non-goals).

## Acceptance
### Automated
**Own-pawn movement under the dev profile** (production channel semantics, seeded)
- [ ] Reproduces first, and gates the transport change. On the commit the build starts from, a steady-run scenario records at least one correction above the ordinary band, and the test is committed failing. That base includes `movement--static-overlap-invariant`, so a result is not confounded by the embeds it fixed. If it records none, stop and report before changing the transport. The presentation-offset row does not wait on it.
- [ ] Steady run, sustained strafe and repeated slides over a seeded minute: no snap-class correction, and none above the ordinary band.
- [ ] A slide or vault the client enters starts on the host on the same client tick.
- [ ] A single lost Input datagram causes no host hold, neutral tick or trim.

**Outage longer than the redundancy**
- [ ] A two-second total outage, followed by recovery: no disconnect. Every edge, activation start and switch sent during it is delivered exactly once, in client-tick order. Movement falls back to the existing gap and catch-up policy.

**Ordering and exactly-once** (regression guards)
- [ ] A switch and a start stamped after it, sent across a loss, resolve in client-tick order.
- [ ] Switch outcomes arrive in declaration order under loss and jitter.
- [ ] Two edges on one client tick both deliver, and no edge delivers twice under loss.
- [ ] Refuse: a switch outcome from before a demotion or level change, delivered after it, resolves nothing, and a declaration pending in the new participation still resolves on its own outcome. Permit: an outcome in the current participation resolves its declaration.
- [ ] A switch declaration sent before a demotion and delivered after re-promotion never reaches the host's switch lane.

**Latency and cost**
- [ ] On a relay pair under the profile, a single loss on Input in either direction is delivered within one-way delay plus two ticks.
- [ ] A newly spawned entity's first baseline is not frozen more than one snapshot past a single lost ack.
- [ ] A tuning payload larger than one slice, sent on Control under loss, delays no Input message.
- [ ] A tuning payload sent before a demotion or level change never applies after it.
- [ ] Client upstream bytes under the profile stay within the bound the executor sets, and the measurement is reported.

**Time sync**
- [ ] The RTT estimate tracks the true RTT within the existing estimator tolerance under the profile.

**Presentation offset**
- [ ] Two corrections landing before the first has decayed leave the presented eye position continuous.

**Version gate**
- [ ] A peer on the previous protocol id or wire version is refused at the handshake.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned at 45/60/5, then at 10% loss: a run, slide and vault course with no visible pull-back. Spawned projectiles and enemies don't freeze. Weapon switches land promptly. The Network tab RTT is plausible.
- [ ] Two-client session at 45/60/5: remote pawns, hit markers and observer cues show no bursts after a loss.
- [ ] Press Off on the Network tab mid-session: the link recovers with no lasting correction.

## Path
- Channels and pins: `connection_config`, `RELIABLE_RESEND`. handshake.rs `PROTOCOL_ID`, `WIRE_VERSION` and their pin tests. The transport channel-list test.
- Check renet's per-channel `max_memory_usage_bytes` and `available_bytes_per_tick` against the two-second outage: about 120 retained commands resent every tick.
- Switch routing: `send_switch_declaration` (Control today) moves onto the framed `send_input` path; host switch-lane intake from `drain_input`; `resolve_switch_outcome`, whose comment cites Control ordering, and the outcome sends in `host_switch.rs`.
- Offset: `ClientPrediction::seed_presentation_offset`, called from `reconcile_local_pawn_with_mover_history`.
- Harness: `predict_reconcile_harness_test_fixtures.rs` (raw unreliable input through `PacketConditioner`; instant `apply_acks`). Relay precedent: `e18_timed_reaction_replication_harness_test.rs`.
- Chosen shape: keep the four channels; one-tick resend on Input, ~100 ms on Control.
  - A separate bulk channel for tuning was rejected: it breaks the tuning-versus-demotion ordering on Control.
  - Strongest rival: a custom unreliable redundant command stream. It avoids blocking during long outages, but it makes edges, starts and switches deliverable only within the redundancy window, and it needs an edge-identity rework. Rejected.
- First slice: the faithful harness and the failing steady-run row. Its result gates the transport change (Acceptance).
- Files past ~800 lines that this extends: transport.rs, wire.rs, lib.rs, client.rs, command_queue.rs, prediction.rs, timesync.rs and the harness fixtures. Split a file first, in its own behaviour-preserving commit, where the change lands inside it.
- At promotion: rewrite `networking.md` §Channel model, §Time sync, §Host input command queue (the switch lane's channel), the "Starts survive playout" rationale in §Combat authority, and §Version gates.

## Open questions
- Gameplay resend interval within 16–20 ms, and the upstream byte bound. — **delegated**: pick from the harness and report the choice in the plan of record.
