# coop--lossy-link-delivery

Brief · resumable · reads: `context/lib/networking.md` §Channel model, §Wire/codec invariants, §Time sync, §Host input command queue, §Combat authority, §Version gates, §Testing the conditioned link · read at b0a5c0f

## Problem
Owner playtest under the dev lag proxy (45 ms + 0–60 ms jitter + 5% loss each way) after PR #601: a connected client's own pawn rubber-bands, worst in slides and vaults, and its reload meter fills in steps. Basis: observed defect. Cause: the client receives host-owned state in two ways that break under loss. Some rides reliable-ordered channels, where one lost datagram stalls everything behind it for at least the 300 ms resend. The rest arrives as unanchored snapshot samples, which the client displays unchanged. For movement this means the host holds, neutral-walks, then stale-drops or trims the stalled commands, so the host never simulates a client's real input across a stall. The test harnesses hide it because they send input unordered and apply acks instantly. When done, under the dev profile a client's own movement, including slide and vault entry, matches the host closely enough that only small smoothed corrections occur. A single lost datagram costs gameplay traffic about one tick of delay, not a resend interval. Host-projected timed values advance smoothly on the client. The harness reproduces production channel semantics, so a regression fails a test.

## Decisions
- **Input stays reliable-ordered, with a resend of about one tick.** renet 2.0 resends each unacked message on its own timer, packs the resends into the next packet and dedupes by message id on receipt. At a 16–20 ms resend, every Input datagram therefore carries every command the peer has not acked: Quake-3-style redundancy that keeps exactly-once, in-order delivery.
  - The edge lanes, activation starts and catch-up keep their guarantees unchanged, including "a trim cannot lose a door press or a weapon drop" (`networking.md` §Host input command queue).
  - The channel M15 phase 3 chose stays (`plans/done/M15--p3-movement-prediction-reconciliation`). Only its resend changes: that phase assumed the gap policy absorbs a 300 ms stall, and it does not absorb a stall followed by a burst.
- **Every gameplay family rides Input at that resend.**
  - Client→host: input commands, acks, refresh requests, time-sync probes, HIT declarations, and weapon-switch declarations, which move from Control.
  - Host→client: shot verdicts, HIT refusals, activation outcomes, observer weapon cues, time-sync echoes, and switch outcomes, which move from Control.

  One ordered stream for input and switches closes the Control-versus-Input ordering gaps that `networking.md` §Combat authority names. At a one-tick resend, head-of-line blocking costs about a tick, so splitting ordering domains across channels buys nothing.
- **Join and bulk traffic leave the gameplay stream.** Control keeps admission, parity, join seed, revealed, participation, divergence, relevel and roster, at about a 100 ms resend. The tuning payload moves to its own bulk channel at 150–200 ms, so its slice resends never block Control. Neither needs sub-100 ms delivery, and per-tick resend of a sliced payload would waste bandwidth.
- **No renet fork, no own reliability layer.** Channel configuration delivers the policy. Adaptive RTO gains nothing at one-tick resend. `research.md` §Resend policy has the arithmetic and the sources.
- **Time sync keeps its min-RTT window gate.** The gate becomes a jitter filter rather than a stall filter.
- **Host-projected timed values animate on the client from a host-time anchor.**
  - Scope: every host-projected value that advances on a timer. Reload progress is the confirmed case. The executor enumerates the rest from the state-slot table and reports the list in the plan of record.
  - Mechanism: the client takes the latest sample at its snapshot's host time and advances it at a replicated rate, using the existing presentation host clock. The rate is replicated with the sample, never recomputed from local tuning (`networking.md` §What gates).
  - Display rules: monotonic within one run. Snaps on a new sample, a cancel, a slot change or the active flag falling. Never reaches completion before the host reports it.
  - The authoritative slot keeps carrying the host sample, so state crossings keep their meaning.
  - Display only: client reload prediction stays out of scope (`networking.md` §Combat authority).
- **A new reconcile correction composes with the presentation offset still in flight.** Today it replaces that offset, so a correction landing before the previous one decays makes the camera jump.
- **The harness reproduces production channel semantics.** The predict/reconcile harness family carries input, acks and time sync over the production channel configuration. Its gate counts corrections above the ordinary band, replacing a 3 m ceiling that allows every symptom above. The state-slot loss harness applies production's newest-snapshot gate. The root cause is the one #601 fixed for deltas: the test link differed from the shipped one.
- **One protocol bump covers the change.** The application protocol id moves for the channel layout and the families that change channel. `WIRE_VERSION` moves for layout. `SNAPSHOT_VERSION` moves only if the replicated rate changes a snapshot record. The previous peer is refused at the handshake.
- **Placement.** Channel layout and resend policy go in `postretro-net`. Send-site routing, switch-lane intake from Input, and the timed-value display go in `postretro-netcode` and the client HUD seam. Host gap policy and catch-up stay as they are; they now handle only genuine outages.
- **Non-goals.**
  - Client inventory composition before the tuning payload: `coop--client-inventory-awaits-tuning` owns it.
  - Pawns embedded in static geometry after a vault: `movement--static-overlap-invariant` owns it. The defect is in the movement substrate and occurs in every role.
  - Clipping the smoothed camera path against geometry. Corrections become small, and real embeds belong to the sibling brief.
  - Lag compensation and time dilation (`networking.md` §Non-goals).

## Acceptance
### Automated
**Own-pawn movement under the dev profile** (production channel semantics, seeded)
- [ ] Reproduces first, and gates the rest. At b0a5c0f a steady-run scenario records at least one correction above the ordinary band, and the test is committed failing. If it records none, stop and report before changing the transport.
- [ ] Steady run, sustained strafe and repeated slides over a seeded minute: no snap-class correction, and none above the ordinary band.
- [ ] A slide or vault the client enters starts on the host on the same client tick.
- [ ] A single lost Input datagram causes no host hold, neutral tick or trim.

**Outage longer than the redundancy**
- [ ] A two-second total outage, followed by recovery: no disconnect. Every edge, activation start and switch sent during it is delivered exactly once, in client-tick order. Movement falls back to the existing gap and catch-up policy.

**Ordering and exactly-once** (regression guards)
- [ ] A switch and a start stamped after it, sent across a loss, resolve in client-tick order.
- [ ] Switch outcomes arrive in declaration order under loss and jitter.
- [ ] Two edges on one client tick both deliver, and no edge delivers twice under loss.

**Latency and cost**
- [ ] On a relay pair under the profile, a single loss on Input in either direction is delivered within one-way delay plus two ticks.
- [ ] A newly spawned entity's first baseline is not frozen more than one snapshot past a single lost ack.
- [ ] A tuning payload larger than one slice, sent under loss, delays no Control or Input message behind it.
- [ ] Client upstream bytes under the profile stay within the bound the executor sets, and the measurement is reported.

**Time sync**
- [ ] The RTT estimate tracks the true RTT within the existing estimator tolerance under the profile.

**Timed values**
- [ ] Permit: under loss and jitter, each enumerated value advances monotonically through lost and reordered snapshots. Each frame's change is at most the replicated rate × frame time plus a small epsilon.
- [ ] Refuse: the value falls on a host cancel or slot change, never passes completion before the host completes, and the reload meter disappears when its active flag falls.
- [ ] Host and single-player displays are unchanged.

**Presentation offset**
- [ ] Two corrections landing before the first has decayed leave the presented eye position continuous.

**Version gate**
- [ ] A peer on the previous protocol id or wire version is refused at the handshake.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned at 45/60/5, then at 10% loss: a run, slide and vault course with no visible pull-back. Timed meters move smoothly. Spawned projectiles and enemies don't freeze. Weapon switches land promptly. The Network tab RTT is plausible.
- [ ] Two-client session at 45/60/5: remote pawns, hit markers and observer cues show no bursts after a loss.
- [ ] Press Off on the Network tab mid-session: the link recovers with no lasting correction.

## Path
- Channels and pins: `connection_config`, `RELIABLE_RESEND`. handshake.rs `PROTOCOL_ID`, `WIRE_VERSION` and their pin tests. The transport channel-list test.
- Check renet's per-channel `max_memory_usage_bytes` and `available_bytes_per_tick` against the two-second outage: about 120 retained commands resent every tick.
- Switch routing: `send_switch_declaration` (Control today); host switch-lane intake; `resolve_switch_outcome` and the outcome sends.
- Meter: hud.ts `reloadMeter`, `PlayerHudStatePublisher` (its connected-client branch skips reload), `WeaponComponent::reload_feedback_sample`, `ReplicatedWireShape::WieldableSlotNumber` (reload progress and weapon cooldown share it), `ClientStateApply`, and `PresentationBindings::presentation_host_time` with its consumer in mesh_anim.rs.
- Offset: `ClientPrediction::seed_presentation_offset`, called from `reconcile_local_pawn_with_mover_history`.
- Harness: `predict_reconcile_harness_test_fixtures.rs` (raw unreliable input through `PacketConditioner`; instant `apply_acks`). Relay precedent: `e18_timed_reaction_replication_harness_test.rs`. `state_slot_loss_harness_test.rs` lacks the newest-snapshot gate.
- Chosen shape: keep the reliable-ordered channels; one-tick resend for gameplay, ~100 ms for Control, a bulk channel for tuning.
  - Strongest rival: a custom unreliable redundant command stream. It avoids blocking during long outages, but it makes edges, starts and switches deliverable only within the redundancy window, and it needs an edge-identity rework. Rejected.
- First slice: the faithful harness and the failing steady-run row. Its result gates the transport change (Acceptance).
- Files past ~800 lines that this extends: transport.rs, wire.rs, lib.rs, client.rs, command_queue.rs, state_slots.rs, prediction.rs, timesync.rs and the harness fixtures. Split a file first, in its own behaviour-preserving commit, where the change lands inside it.
- At promotion: rewrite `networking.md` §Channel model, §Time sync, §Host input command queue (the switch lane's channel), the "Starts survive playout" rationale in §Combat authority, and §Version gates.

## Open questions
- Gameplay resend interval within 16–20 ms, and the upstream byte bound. — **delegated**: pick from the harness and report the choice in the plan of record.
- Replicated rate shape for timed values: total duration, remaining time or rate. — **delegated**.
