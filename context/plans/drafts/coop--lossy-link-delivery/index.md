# coop--lossy-link-delivery

Brief · resumable · reads: `context/lib/networking.md` §Channel model, §Wire/codec invariants, §Time sync, §Host input command queue, §Combat authority, §Version gates, §Testing the conditioned link · read at b0a5c0f

## Problem
Owner playtest under the dev lag proxy (45 ms + 0–60 ms jitter + 5% loss each way) after PR #601: a connected client's own pawn rubber-bands, worst in slides and vaults, and its reload meter fills in steps. Basis: observed defect. Cause: the client receives host-owned state either through reliable-ordered channels, where one lost datagram stalls everything behind it for at least the 300 ms resend, or as unanchored snapshot samples it displays unchanged. For movement this means the host holds, neutral-walks, then stale-drops or trims the stalled commands, so the host never simulates a client's real input across a stall. The test harnesses hide it because they send input unordered and apply acks instantly. When done, under the dev profile a client's own movement, including slide and vault entry, matches the host closely enough that only small smoothed corrections occur. A single lost datagram costs latency-critical reliable traffic roughly one tick of delay, not a resend interval. Host-projected timed meters advance smoothly on the client. The harness reproduces production channel semantics, so a regression fails a test.

## Decisions
- **Movement input is an unreliable, redundant command stream.** Each client tick sends one unreliable message carrying every command the host has not yet resolved, read from the snapshot's resolved cursor, with a count cap and an unsliced datagram (under 1200 B). The host already collapses duplicate ticks at intake, so redundancy needs no new identity. This reverses M15 phase 3's choice to keep input on reliable-ordered Input (`plans/done/M15--p3-movement-prediction-reconciliation`). That choice assumed the gap policy absorbs resend stalls, and it does not absorb a stall followed by a burst.
- **Input edges become per-tick events delivered once, whatever the arrival order.** Reload, use, drop and the other press-type edges are judged per client tick, never by comparing against the newest level observed, so a redundant copy delivers a lost edge and a reordered older copy delivers nothing twice. Activation tokens keep their token identity.
- **Weapon-switch declarations join the command stream.** They keep `declaration_id` and their client-tick stamp, and the host lane keeps ordering by tick. One stream for input and switches closes the Control-versus-Input ordering gaps `networking.md` §Combat authority names. Outcomes stay a strictly ordered chain.
- **Time sync rides the command stream, unreliably.** Probes and echoes are already matched by sample id. The min-RTT window gate stays, as a jitter filter.
- **Acks are self-healing and unreliable.** The client re-acks a held baseline id whenever the host resends it, so a lost ack heals on the next snapshot. Host ack apply stays monotonic and additive (`networking.md` §Wire/codec invariants). Refresh requests stay idempotent. Movement authority keeps riding snapshot records.
- **Remaining reliable traffic gets a resend policy per family, not one global interval.** Ordering domains get separate channels.
  - Latency-critical small families resend every tick, about 16–20 ms, until acked:
    - switch outcomes, ordered;
    - activation outcomes, ordered per token;
    - HIT declarations, shot verdicts and HIT refusals, unordered and deduped by shot id;
    - observer weapon cues, unordered and deduped.
  - Join, admission, parity and level change: about 100 ms.
  - The tuning payload moves to its own bulk channel at 150–200 ms, so its slice resends never block gameplay traffic.
  - Every hot-channel message stays under one unsliced datagram.
- **The policy is first built as renet channel configuration.** A hot family moves into the command stream's own reliability only where measurement shows the extra datagrams cost more than the move. No renet fork: renet is exact-pinned, and RTT-adaptive resend gains nothing for traffic that does not need sub-100 ms delivery. `research.md` §Resend policy has the arithmetic and the sources.
- **Host-projected timed meters animate on the client from a host-time anchor.** The client takes the latest sample at its snapshot's host time and advances it at a replicated rate using the existing presentation host clock. The rate is replicated with the sample, never recomputed from local tuning (`networking.md` §What gates). The displayed value is monotonic within one run. It snaps on a new sample, a cancel, a slot change or the active flag falling. It never reaches completion before the host reports completion. The authoritative slot keeps carrying the host sample, so state crossings keep their meaning. Reload progress is the confirmed case. The executor applies the same mechanism to every other host-projected value that advances on a timer. This is display only: client reload prediction stays out of scope (`networking.md` §Combat authority).
- **A new reconcile correction composes with the presentation offset still in flight.** Today it replaces that offset, so a correction landing before the previous one has decayed makes the camera jump.
- **The harness reproduces production channel semantics.** The predict/reconcile harness family carries input, acks and time sync over the production channel configuration. Its gate counts corrections above the ordinary band, replacing a 3 m ceiling that allows every symptom above. The state-slot loss harness applies production's newest-snapshot gate. This is the same root cause #601 fixed for deltas: the test link differed from the shipped one.
- **One protocol bump covers the whole change.** The application protocol id moves for the channel layout and vocabulary. `WIRE_VERSION` moves for layout. `SNAPSHOT_VERSION` moves only if the meter's replicated rate changes a snapshot record. The previous peer is refused at the handshake.
- **Placement.** Transport layout and resend policy go in `postretro-net`. Command-stream framing, edge identity, ack healing and meter display go in `postretro-netcode` and the client HUD seam. Host gap policy and catch-up stay as they are: they now handle only genuine stalls.
- **Non-goals.**
  - Client inventory composition before the tuning payload: `coop--client-inventory-awaits-tuning` owns it.
  - Pawns embedded in static geometry, which the owner saw after a vault: `movement--static-overlap-invariant` owns it. That defect is in the movement substrate and occurs in every role.
  - Clipping the smoothed camera path against geometry. Corrections become small, and real embeds belong to the sibling brief.
  - Lag compensation and time dilation stay non-goals (`networking.md` §Non-goals).

## Acceptance
### Automated
**Own-pawn movement under the dev profile** (production channel semantics, seeded)
- [ ] Reproduces first: at b0a5c0f a steady-run scenario records at least one correction above the ordinary band. The test is committed failing, before the fix.
- [ ] Steady run, sustained strafe and repeated slides over a seeded minute: no snap-class correction, and no correction above the ordinary band except after a loss run longer than the redundancy window.
- [ ] A slide or vault the client enters starts on the host on the same client tick.
- [ ] A lost datagram followed by a redundant copy produces no host hold, neutral tick or trim.
- [ ] A loss run longer than the redundancy window falls back to the existing gap policy, without a panic or an unbounded queue.

**Edges delivered exactly once**
- [ ] Permit: a jump, reload, use, drop or activation start whose first datagram is lost is delivered once, from a redundant copy.
- [ ] Refuse: a duplicated or reordered older datagram adds no second edge and re-applies no movement.
- [ ] Two edges on one client tick both deliver.
- [ ] A switch and a start stamped after it, arriving in reverse order, resolve in client-tick order.

**Acks and replication**
- [ ] After a lost ack, the host stops resending the held baseline after the client's next re-ack.
- [ ] A newly spawned entity's first baseline is not frozen more than one snapshot past a single lost ack.
- [ ] A tombstone retires after a lost ack.
- [ ] Refuse: a stale or duplicated re-ack never moves host ack state backward.

**Reliable families**
- [ ] On a relay pair under the profile, a single loss on a hot family is delivered within one-way delay plus two ticks.
- [ ] Switch outcomes stay in declaration order under loss and reorder.
- [ ] A duplicated HIT declaration is accepted once. A duplicated cue plays once.
- [ ] Every hot-family message fits one unsliced datagram; asserted at the send site.
- [ ] A tuning payload larger than one slice, sent under loss, delays no gameplay message behind it.

**Time sync**
- [ ] Over the unreliable stream, the RTT estimate tracks the true RTT within the existing estimator tolerance under the profile.

**Timed meters**
- [ ] Permit: under loss and jitter a client reload meter rises monotonically, and each frame's change is at most the replicated rate × frame time plus a small epsilon, through lost and reordered snapshots.
- [ ] Refuse: it falls on a host cancel or slot change, never passes full before the host completes, and disappears when the active flag falls.
- [ ] Host and single-player meters are unchanged.

**Presentation offset**
- [ ] Two corrections landing before the first has decayed leave the presented eye position continuous.

**Version gate**
- [ ] A peer on the previous protocol id or wire version is refused at the handshake.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned 45/60/5, then 10% loss: a run, slide and vault course shows no visible pull-back. Reload meter is smooth. Spawned projectiles and enemies don't freeze. Weapon switches land promptly. Network tab RTT is plausible.
- [ ] Two-client session at 45/60/5: remote pawns, hit markers and observer cues show no bursts after loss.
- [ ] Off on the Network tab mid-session: the link recovers with no lasting correction.

## Path
- Send side: `client_predict_tick_with_sender` → `NetClient::send_input`. The rare zero-tick cancel send (`client_send_input_command_in_registry`) allocates its own neutral tick; fold it into the stream.
- Host intake: `ClientCommandState::enqueue`, `observe_reload_level`, the press-edge lanes in `command_queue/press_edges.rs`, `ActivationEdges`. The switch lane receives from Control today.
- Acks: `encode_client_messages`, `ServerReplication::apply_ack`, the in-flight skip in `replication/encode.rs`, and the held-id branch of the client's `apply_delta` that does not re-ack.
- Channels and pins: `connection_config`; handshake.rs `PROTOCOL_ID`, `WIRE_VERSION` and their pin tests; the transport channel-list test.
- Meter: hud.ts `reloadMeter`, `PlayerHudStatePublisher` (the connected-client branch skips reload), `WeaponComponent::reload_feedback_sample`, `ReplicatedWireShape::WieldableSlotNumber`, `ClientStateApply`, `PresentationBindings::presentation_host_time` and its consumer in mesh_anim.rs.
- Offset: `ClientPrediction::seed_presentation_offset`, called from `reconcile_local_pawn_with_mover_history`.
- Harness: `predict_reconcile_harness_test_fixtures.rs` (raw unreliable input through `PacketConditioner`, instant `apply_acks`). The renet relay precedent is `e18_timed_reaction_replication_harness_test.rs`. `state_slot_loss_harness_test.rs` lacks the newest-snapshot gate.
- Chosen shape: Quake-3-style redundant input over unreliable, plus renet channels split by ordering domain at per-family resend times. Strongest rival: reliable-unordered input. It removes the head-of-line block, but a lost tick still waits a full resend and its edge is lost. Rejected.
- First slice: the faithful harness and the failing steady-run row. It falsifies the claim that input stalls dominate the rubber-band before any transport code changes.
- Files past ~800 lines that this extends: transport.rs, wire.rs, lib.rs, client.rs, command_queue.rs, state_slots.rs, prediction.rs, timesync.rs and the harness fixtures. Add the command stream and resend policy as new modules. Split a file first, in its own behaviour-preserving commit, where the change lands inside it.
- At promotion, rewrite `networking.md` §Channel model, §Time sync, §Host input command queue, the "Starts survive playout" rationale in §Combat authority, and §Version gates.

## Open questions
- Redundancy window cap (count and bytes) and hot-family resend interval within 16–20 ms. — **delegated**: the executor picks from the harness and reports it in the plan of record.
- Replicated meter rate shape: total duration, remaining time or rate. — **delegated**.
- Whether any hot family moves into the command stream's own reliability after measurement. — **delegated**: report the measurement either way.
