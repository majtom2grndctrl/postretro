# coop--lossy-link-delivery — plan of record

mode: resumable
status: approved
read at: 6c3adca4e

Owner approved the plan and both restatements (rows 20, 21) on 2026-10-21; the brief carries the new wording.

## Corrections
- Brief: switch variants live in "the Control enums" in wire.rs → they live in `crates/net/src/wire/control.rs` (`ClientControlMessage::SwitchDeclaration` at index 2, `ServerControlMessage::SwitchRefused`/`SwitchAccepted` at 3/4); wire.rs re-exports. Planning around it by moving them out of control.rs and appending `SwitchDeclaration` to `ClientMessage` and `SwitchAccepted`/`SwitchRefused` to `ServerMessage` (appends are safe under `observer_cue_append_preserves_shipped_server_message_tags`).
- Brief: "the transport sends once per frame" → true, but the poll runs at frame *start* (`net_poll_and_apply` before the fixed-tick loop), so a message sent this frame leaves at the next frame's poll, and renet's resend clock advances by summed frame `dt`. A resend of exactly one tick (16.67 ms) misses on any frame shorter than a tick and falls back to every second datagram (P2's failure). Planning around it by choosing the resend with margin below a tick (Delegated answers).
- Brief: "the transport channel-list test" pins channels → `presentation_channel_is_the_fourth_dedicated_unreliable_channel` (transport.rs) pins only order and Presentation's send type, server config only. Extending it to pin all four delivery types and both resends on both server and client configs.
- Brief: "Files past ~800 lines … lib.rs" → the net crate's lib.rs is 21 lines; the large one is `crates/netcode/src/lib.rs` (8265). Its host Input dispatch (`host_handle_client_messages`/`_message`/`_inner`, `send_shot_verdict`) and client switch chain (`resolve_switch_outcome` + tests, with endpoint.rs's switch types/API) are split out first in a behaviour-preserving commit, into `host_input.rs` and `client_switch.rs`.
- Brief: harness "raw unreliable input through `PacketConditioner`; instant `apply_acks`" → also: no participation framing (host ingest bypasses `NetServer::drain_input`), no time sync at all (the client is handed the host tick), Control delay-only, a harness-only `PLAYOUT_WARMUP_TICKS = 7` standing buffer (production starts at the depth-2 buildup latch, so ~100 ms stalls are masked), and the latency gate reads correction magnitude through a duplicate receive path (`step_and_watch_correction`). Planning around it by building a relay link layer (renet `NetServer`/`NetClient` pair over `PacketConditioner`, production `connection_config`, `drain_input`, framed Input, acks/refresh/time sync over Input, no warmup) following `e18_timed_reaction_replication_harness_test.rs` / `host_activations/conditioned_tests.rs`, then moving the `LoopbackHarness` family onto it.
- Brief: switch intake from `drain_input` → `host_handle_client_messages` lacks the `MovementOwners` that `host_retain_switch_declaration` needs (4 callers), and client outcomes would arrive inside `client_drive_time_sync`, which returns `Vec<ClientShotFact>`. Adding the parameter and an outcome route back to `apply_client_switch_resolution`.
- `poll_world_less` discards client Input (endpoint.rs), which will now include switch outcomes. Safe because level unload and Holding clear the chain; covered by a regression in task 5's P7/P9 tests.
- Trim is not test-observable outside log-gated `netdiag`. AC 4's "no hold, neutral tick or trim" is proven from `HostTickTrace.resolved`: every entry in the window after buildup is `ResolutionSource::Real` with `client_tick` advancing by exactly 1. `HostTickTrace` gains the host pawn's movement state for AC 3.
- Production decays the presentation offset once per predicted fixed tick; `LoopbackHarness` never decays it, and several tests read `presentation_offset().length()` as "this correction's magnitude". Under composition both change: the harness decays per step as production does, and those tests read `latest_local_corrections`.
- Composed-offset snap (Decision "snaps, as a single correction there does: one rule for when the eye cuts") → clarification: a composed offset at or past `TELEPORT_CORRECTION_MIN_M` clears the offset and emits the camera-cut adoption a teleport-class correction does; prediction history is kept, since the reconcile that produced it was ordinary.
- Upstream-bytes AC (row 19) names no counter → clarification: measured as renet connection-level packet bytes the client emits (what the relay sees: renet payload plus renet headers), with netcode/UDP overhead reported beside it as an estimate. Pre-measurement estimate at the profile's mean RTT: ~37 KB/s at that counter, ~42 KB/s on the wire. Close to the bound; the measurement decides.
- Outage AC (row 6) "each start reaches one terminal: delivered, or refused for lag" → the code has further terminals (expiry, client-half cadence refusal, silent intake drop at 64). The test uses a weapon whose cadence keeps under 64 starts per 2 s and spaces starts past recovery, so only the named terminals can arise; the assertion still fails on any start with no terminal or two.
- Time-sync RTT row (row 22) "true RTT within the existing estimator tolerance" → clarification: true RTT is the probe round trip the harness realizes on the relay (conditioner arrival times plus poll quantization), compared at the window minimum; tolerance is the estimator's gate tolerance, `max(RTT_GATE_FLOOR_US, RTT_GATE_FRACTION × window_min)`.

## Delegated answers
- Gameplay (Input) resend — **10 ms**. Any value at or below a 60 Hz frame resends on the next poll in the harness; 10 ms keeps that true for real frames up to ~6 ms shorter than a tick (vsync jitter, a short frame after a long one), and costs the same as 15 ms at 60 Hz (one copy per poll). At 120/144 Hz it resends every second poll (16.7 / 13.9 ms), still within a tick.
- Control resend — **100 ms**, the brief's "about 100 ms".

## AC-to-proof

| AC | Row | Proof | Status |
|---|---|---|---|
| 1 | Reproduces first; loss-off control | `steady_run_stays_in_ordinary_band_under_dev_profile` committed failing at base resend (148 above band, max 5.3 m Teleport; host Neutral 1365/3600, seeds 0x1502/0x1503); `steady_run_loss_off_records_no_oversized_correction` (P1) passes (all 0.000 m, 3600 Real) | stop gate passed |
| 2 | Seeded-minute steady run, strafe, slides: nothing above Ordinary | `steady_run_stays_in_ordinary_band_under_dev_profile` (relay harness; same test as row 1, passes after task 2) | achievable as stated |
| 3 | Slide/vault entry on the same client tick, incl. lost entry datagram (P3) | `slide_entry_lost_datagram_enters_on_host_same_client_tick`, `vault_entry_lost_datagram_enters_on_host_same_client_tick`, no-loss twins | passes (task 2) |
| 4 | Fixed delay, one scripted lost Input datagram: no hold/neutral/trim, Real on its own tick (P2) | `single_lost_input_datagram_resolves_real_on_its_own_tick` | passes (task 2) |
| 5 | Grep gate: hold/target/max stay 3/2/8 | `gap_policy_keeps_hold_3_buffer_target_2_and_buffer_max_8` + grep in preflight | passes (task 2) |
| 6 | 2 s outage: no disconnect; edges/switches exactly once in order; starts one terminal; no malformed switch (P12) | `two_second_outage_delivers_lanes_once_in_client_tick_order` (relay, scripted window drop) | achievable as stated (test design per Corrections) |
| 7 | Switch then later start, loss on switch datagram (P4); start on switch's own command (P5) | `switch_order` relay tests migrated to Input | achievable as stated |
| 8 | Switch outcomes in declaration order under loss and jitter | `switch_outcomes_arrive_in_declaration_order_under_profile` | achievable as stated |
| 9 | Two edges on one tick both deliver; none twice (P11) | `two_edges_on_one_tick_deliver_once_across_resend` | achievable as stated |
| 10 | Refuse stale outcome across demote/relevel; permit current (P9, P10) | client_switch chain tests over relay | achievable as stated |
| 11 | Declaration across demote→re-promote never reaches lane; permit post-re-promotion (P6) | relay test on `drain_input` epoch drop | achievable as stated |
| 12 | Host-dropped declaration never leaves chain waiting; Holding + marker in one drain (P7) | relay test, incl. world-less poll regression | achievable as stated |
| 13 | Switch retained in lane at demotion never applied after re-promotion (P8) | command-queue / relay test | achievable as stated |
| 14 | Grep gate: no switch on Control | grep in preflight + compile-time removal of Control variants | achievable as stated |
| 15 | Relay pair, scripted single drop either direction ≤ largest one-way + 2 ticks (P2) | `single_input_drop_delivers_within_one_way_plus_two_ticks_both_directions` | passes (task 2): 116.0 / 127.0 ms at conditioner arrival (116.7 / 133.3 frame-quantized) vs 138.3 ms bound |
| 16 | New entity's baseline frozen ≤ one snapshot past a lost ack, ack on Input (P15) | relay test with scripted ack drop | achievable as stated |
| 17 | Sliced tuning on Control under loss delays no Input message (P16) | relay test: scripted slice drops, Input latency bound | achievable as stated |
| 18 | Tuning sent before demote/relevel never applies after; permit post-re-promotion (P17) | relay test | achievable as stated |
| 19 | Client upstream ≤ 40 KB/s at 60 Hz under profile, reported | `client_upstream_bytes_under_dev_profile` measurement (counter per Corrections) | achievable; measurement may fail — reported either way |
| 20 | Outage: offset within one tick, **no outage probe folds**, no reseed (P13) | relay time-sync outage test | achievable as restated |
| 21 | Probe lost once: one echo, RTT **≤ one resend** above unlost (P14) | relay time-sync single-drop test | achievable as restated |
| 22 | RTT estimate tracks true RTT within estimator tolerance | relay time-sync profile test (clarification per Corrections) | achievable as stated |
| 23 | Join: lost tuning slice delays inventory ≤ one Control resend + largest one-way | relay join test, delay measured against the same seed without the drop | achievable as stated |
| 24 | Two corrections before decay / same frame: presented eye continuous (P18) | `correction_before_previous_decayed_composes_and_presented_eye_stays_continuous`, `two_corrections_in_one_frame_leave_presented_eye_continuous` | passes (task 3) |
| 25 | Refuse: teleport-class, first-record, level reset clear offset; composed ≥ floor snaps (P18) | `teleport_class_correction_clears_in_flight_offset_and_next_correction_does_not_compose`, `first_record_adoption_clears_…`, `level_unload_clears_…`, `composed_offset_at_teleport_floor_snaps_with_camera_cut_and_keeps_history` | passes (task 3) |
| 26 | Previous protocol id or wire version refused; new pair admitted; snapshot version unmoved | handshake pin tests at PRLC / 33 / 21 | achievable as stated |
| 27 | Channel list: four in order, Control RO ~100 ms, Snapshot/Presentation unreliable, Input RO at gameplay resend; renet `=2.0.0` | `connection_config_pins_four_channels_with_gameplay_resend_under_one_tick`, `renet_stays_pinned_at_2_0_0` | passes (task 2) |
| 28 | Manual: loopback recipe at 45/60/5 and 10% loss | owner, in-engine | manual |
| 29 | Manual: two-client session, no bursts after loss | owner, in-engine | manual |
| 30 | Manual: Network tab Off mid-session recovers | owner, in-engine | manual |

### Restatements (owner-approved 2026-10-21)

**Row 20 (P13).** The min-RTT gate admits any sample within its tolerance
(`max(20 ms, 0.5 × window_min)`, ~45–60 ms at this profile). A probe sent within that
tolerance before the outage ends is lengthened by less than the tolerance and folds by
design, so "no outage probe folds" depends on the outage's phase against the 200 ms
probe cadence. Proposed:

> Across the two-second outage the server-tick offset stays within one tick, no probe whose round trip the outage lengthened by more than the gate tolerance folds into the estimate, and no reseed fires (P13).

**Row 21 (P14).** With the 10 ms resend, a lost probe's copy leaves on the next poll, one
fixed tick after the first copy. Its RTT comes out about one tick high, more than one
10 ms resend. Proposed:

> A probe whose datagram is lost once yields one echo, and its RTT sample is at most one fixed tick above the unlost round trip (P14).

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Relay link layer (`PacketConditioner` scripted drop: Nth flush / virtual window, applied after the seeded draw; renet pair on production `connection_config`; framed Input; acks/refresh/time sync over Input; no warmup; `HostTickTrace` movement state) as a new fixture module beside the existing fixtures; reproduce-first steady-run test (committed failing at the 300 ms base) and P1 loss-off control. **Stop and report** if no reproduction, or if loss-off also records one. | delegated worker | — | done: relay fixture `predict_reconcile_relay_test_fixtures.rs` (`RelayHarness`), `ScriptedDrop` in net harness.rs; repro fails on base, P1 passes; net 295 pass, netcode lib 833 pass + 1 expected fail |
| 2 | Transport resend: Input 10 ms, Control 100 ms; extend channel-list test; bump `PROTOCOL_ID` PRLB→PRLC and `WIRE_VERSION` 32→33, update the hard-coded pin tests; AC 2–5, 15, 27 green on the relay harness | integrating executor | 1 |done: steady run max 0.18 m (Real 3599/3600); PRLC/33 pins + `gameplay_channel_layout_refuses_prlb_and_wire_32_peers`, `admission_refuses_prlb_or_wire_32_and_admits_prlc_33`; engine trim cells in `host_activations/conditioned_tests.rs` now force the trim with a 255 ms uplink cut (`TRIM_OUTAGE`) instead of 300 ms-era loss seeds; net 298, netcode 841 + 1 ignored, host_activations 66 |
| 3 | Presentation offset composition in `seed_presentation_offset`/reconcile: compose, composed-floor snap with camera cut, existing clears; harness decays per step; tests read `latest_local_corrections`; AC 24–25 | delegated worker (prediction.rs, reconcile.rs, offset tests) | — | done: `seed_presentation_offset` composes and returns whether the composed offset snapped; reconcile reports a composed snap as `CorrectionClass::Teleport` with an eye-cut-only `LocalTeleportAdoption` (yaw 0, no identity change, no cue), history kept; `LoopbackHarness` decays per step; netcode 847 + 1 ignored. Follow-up in task 5: steady-run gate also asserts zero Teleport-class records, so a composed snap fails it |
| 4a | Behaviour-preserving split of netcode lib.rs: `host_input.rs` (host Input dispatch), `client_switch.rs` (switch chain + endpoint switch API) | integrating executor | 2 | |
| 4b | Move switches to Input: wire variants (control.rs → `ClientMessage`/`ServerMessage`), retire Control arms, `ServerPoll.switch_declarations`, and the append-layout guards that mirror removed variants (rewrite against the new layout); host intake from `drain_input` with `MovementOwners`; outcome sends via `send_input`; client outcome route from the Input drain; stale Control-citing comments; migrate `conditioned_tests`/`switch_order`/`late_join`/roster harness; AC 7, 10–14 | integrating executor | 4a | |
| 5 | Outage and ordering rows: AC 6, 8, 9, 12's world-less regression; time-sync rows AC 20–22 | integrating executor | 4b | |
| 6 | Latency and cost rows: AC 16–19, 23 (bandwidth measurement reported) | integrating executor | 4b | |
| 7 | Move the `LoopbackHarness` family (swim, vault, immersion, fluid tables, pinned lift, teleport rig/teleport/join, boot spawn gate) onto the relay link; retire raw input/instant `apply_acks`/warmup and the 3 m ceiling gate (`latency_harness_converges_within_tolerance_under_mandated_profile`) and `step_and_watch_correction` | integrating executor (may delegate per-file migrations after the link API is fixed) | 2, 3 | |
| 8 | Version-gate rows AC 26; durable doc reconciliation (networking.md already describes the target; confirm values, resend figures, switch lane channel, `Starts survive playout`, §Version gates PRLC/33) | integrating executor | 4b–7 | |
