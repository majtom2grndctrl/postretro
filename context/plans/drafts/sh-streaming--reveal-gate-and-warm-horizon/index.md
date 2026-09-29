# sh-streaming--reveal-gate-and-warm-horizon

Brief · compact · **gated** · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, `context/lib/boot_sequence.md` §1, §3, §4, `context/lib/networking.md` §Two-gate handshake, §Admission and content parity, §Slot lifecycle, §Session-state ledger, `context/lib/testing_guide.md` §Resource bounds · read at 3f070e32d (`feat/preferences-comfort-floor`)

**Sequence: 3 of 4.** Build after `E23--preferences-comfort-floor` lands. That branch edits `startup/`, which this brief also edits, and two of its rules must extend to Settling (see Decisions). No dependency on `animated-lightmap-compact-atlas` (1 of 4). It shares install and host-net files with `coop--spawn-placement-occupancy` (2 of 4) but not behavior. Rebase on it if it lands first. `coop--client-spawn-view-handoff` (4 of 4) depends on this brief's Settling state, eye-pose chokepoint and revealed hold.

## Problem
The owner saw SH lighting fill in on screen, both on level entry and in play, while VRAM use stayed low. Basis: an observed defect. On entry, the splash clears as soon as CPU install finishes, and SH residency starts empty, so the first frames show the ambient-floor fallback. In play, the warm horizon is a small fixed count of clusters, so walking a few rooms ahead, or returning after the hysteresis window, brings a cluster into view cold. When done: a level is not revealed until the SH around the spawn view is resident, or until a generous timeout runs out, and in co-op a client's pawn spawns only once both it and the host have revealed the level. In play, a much wider horizon is prefetched, and the existing budget trims it.

## Decisions
- **Settling boot state.** A new state sits between Loading and Running. The splash stays painted, the sim does not tick, no world frame is presented, and no sound plays. Level sounds start at reveal. Settling frames run visibility from the eye pose the first Running frame will present, then the SH drain, compose and submit, so clusters reach Sampleable. That pose comes from one chokepoint. Today install places the camera at the spawn origin and the first tick adds eye height, so Settling must apply the same eye offset. Precedent: the first-launch hold is also its own boot state, where no level runs and no sound plays (`boot_sequence.md`); it is not a pause flag. The sim-not-ticking rule is load-bearing, because nothing may act on the player while they can't see.
- **One settle chokepoint.** Whether Settling may end is decided in one named lifecycle check. It asks each streamed resource whether it is settled. SH is the only one today. Lightmap-shaped streaming (`context/plans/large-map-spatial-residency.md` stage 5) joins the same check.
- **Settling is a splash frame for E23.** The rule that UI input pressed on a splash or Loading frame is dropped, never delivered later (`done/E23--preferences-comfort-floor` Decisions), treats Settling frames as splash frames. E23 withdrew the flash limiter's splash hand-off (owner decision C). The channel clamp needs only presented-frame time, which runs between resolve frames, so a Settling stretch ages its flash window by the stretch's length, as a splash stretch does.
- **Entry settle set.** Settling waits for every Visible and Pinned target with their owner closure, and for the nearest warm clusters up to a fixed entry count equal to today's warm count. While Settling, those entry warm clusters are mandatory: they may push past the budget and are never refused for pressure, or a map near its budget would wait out the timeout on every entry. At reveal they become optional prefetch again, which keeps entry hold time independent of the horizon width.
- **Every level entry settles.** Boot, `loadLevel` and `restartLevel` all go through Settling. The frontend background level does not settle: its menu already covers the world, and delaying the menu costs more than a brief SH fill behind it.
- **Timeout, never hang.** Settling releases after a fixed timeout, default 10 s. On release it logs a warning naming the unsettled target count and reveals with the existing ambient-floor miss fallback. The timer starts fresh on each entry to Settling. A load request arriving during Settling follows the ordinary Unload→Loading path.
- **In-play misses still never stall.** The gate applies only at level entry. It keeps the `sh-probe-streaming` rule that doors never wait on residency and in-play misses fall back to ambient floor (`plans/done/sh-probe-streaming` §Open questions).
- **Co-op: Settling polls; parity is content identity.** The endpoint keeps polling during Settling the way the world-less transport does (`networking.md`), so a long hold drops no peer. Level parity, Relevel and the join seed publish at install, as today, so a content-divergent client learns its cause during its settle.
- **Revealed is a participation term.** A slot participates iff parity matches and both peers have revealed the host's installed level. The client declares on Control the level identity it has revealed, none at unload; the host records its own reveal locally, and an entry that skips Settling reveals at install. The name is "revealed", never "ready", which `context/research/coop-session-lobby.md` §2 reserves for mod-authored meaning.
- **The host half is load-bearing.** A Settling poll's promotion handler spawns no pawn, so a restarted host must not promote a revealed client before it reveals itself.
- **The pawn hold is admitted, not a new stage.** A parity-matched slot that has not revealed stays admitted, held with a revealed holding cause that, like any hold, retires the client's epoch on demotion. `networking.md` §Admission and content parity and §Slot lifecycle record the revealed term at promotion; the four stages and "any entry to participating spawns its pawn" stand. Rival shapes: `research.md`.
- **Revealed lifetime.** A slot's revealed record survives parity changes and host level changes; only the client's own declaration or the slot closing changes it. So a mod-digest demotion re-promotes without a fresh reveal, and a restarted host re-promotes a Running client at its own reveal. No timer on either peer: the Settling timeout bounds a live client, and a silent one is held like any parity hold, bounded by keepalive.
- **Reveal at settle, not at arm.** The client declares revealed on its Settling→Running frame, so the pawnless window after reveal is about one round trip plus a snapshot interval, in which it sends no input and nothing acts on its player. Holding the splash until arm would need snapshot apply during Settling, which the world-less poll excludes.
- **Co-op layer placement.** The net crate owns the revealed record and the predicate term, carrying level identities as opaque strings; it stays registry-blind. The engine publishes each peer's reveal from the Settling→Running edge. The host's per-slot revealed record survives a level change, so `networking.md` §Session-state ledger lists it beside the connection's last parity declaration, at promotion.
- **Co-op client view: accepted limit.** A client learns its real spawn only from the first baseline after promotion, so it settles the view at the first `player_spawn`. After arming it may jump to a cold spot, which the ambient-floor fallback covers. `coop--client-spawn-view-handoff` retargets that stand-in spawn view.
- **Wider warm horizon, budget as the limiter.** `WARM_SET_CLUSTERS` rises to a value chosen by measurement; the io-contract plan declared it "tuning, not contract" (`plans/done/sh-streaming--warm-set-and-io-contract.md` §Open questions). The horizon stays a cluster count, an owner experiment; a path-distance horizon is the fallback if the count keeps misbehaving (`large-map-spatial-residency.md` §Lessons from SH residency). Budget pressure trims it farthest first, and hysteresis-departure eviction stays, by owner choice, though on a map that fits it frees no VRAM: backtracking is uncommon in some retro-FPS styles.
- **Pop-in evidence.** Diagnostics count clusters read again after being evicted. The miss counter separates a visible cluster never targeted, one targeted but in flight, and one held back by the install cap, showing which lever a later change should pull.
- **Non-goals.** Budget tiers and laptop defaults, which wait for a resource-neutral budget covering lightmap-shaped data (`large-map-spatial-residency.md`, Decisions still open). Keeping departed clusters until pressure. Streaming lightmap-shaped data. Changing the per-drain install cap or read coalescing, unless measurement implicates them. A waiting overlay for a held co-op client: UI policy; the holding diagnostic names the cause.

## Acceptance
### Automated
**Settle predicate**
- [ ] Not settled while any Visible or Pinned target, any cluster in their owner closure, or any of the nearest warm clusters up to the entry count, is below Sampleable.
- [ ] Settles with a farther warm cluster or a seam-warm target still cold.
- [ ] Settled immediately when there are zero targets: streaming off, or a level without a valid id-49/id-50 pair.
- [ ] Raising the warm count does not change which clusters the entry settle set contains.
- [ ] With a budget below the entry settle set's bytes, Settling still settles before the timeout. The entry warm clusters are admitted past the budget.
- [ ] On the reveal frame, entry warm clusters become optional again. Under pressure they yield farthest-first on the next drain.

**Settling lifecycle**
- [ ] The sim tick count and game time do not advance during Settling. The first tick happens after the switch to Running.
- [ ] No world frame is presented and no sound plays during Settling. The splash is painted on each redraw, and level sounds queued by install begin on the reveal frame.
- [ ] A UI input pressed during Settling activates nothing on the first Running frame, and the channel clamp ages its flash window across Settling by the stretch's length, as it does across the splash.
- [ ] A cluster requested during Settling reaches Sampleable without a switch to Running, which proves the drain, compose and submit all run.
- [ ] The timeout releases to Running and emits exactly one warning, verified by log capture. A settle just before the deadline releases with no warning.
- [ ] `restartLevel` after a timed-out entry gets a full, fresh timeout.
- [ ] A load request during Settling unloads and reloads without a stale settle or stale timer carrying over.
- [ ] The frontend background level never enters Settling, and counts as revealed from its install frame.
- [ ] Regression guard: after Running, a visible cluster that is not resident renders the ambient-floor fallback on that same frame. No frame waits for it.

**Co-op: a pawn spawns only once both peers have revealed**
- [ ] Both peers publish level parity on the install frame. A content-divergent client receives its divergence cause while still in Settling.
- [ ] A parity-matched client in Settling is held: no pawn, no tuning, no snapshot, no epoch marker. Its pawn spawns on the host poll that drains its revealed declaration.
- [ ] A client whose Settling times out still declares revealed on its reveal frame.
- [ ] Revealed before promotion: a client that declares revealed while parity mismatches is promoted the moment parity matches, with no second declaration.
- [ ] Revealed never arrives: the slot stays held with the revealed cause, stays connected past the Settling timeout, and receives no entity state.
- [ ] A duplicate revealed declaration for the current level causes no second participation entry, pawn or epoch. A declaration naming a level other than the host's installed one does not promote.
- [ ] Client unload retracts revealed. A re-install of the same level is held until its own reveal, even when the host's Control drain collapses the unload and install into one batch.
- [ ] While the host is in Settling, a parity-matched revealed client is not promoted, no pawn spawns for it, and it stays connected across a hold of at least the timeout. It is promoted on the host's reveal frame, exactly once.
- [ ] Host restart: a Running client is re-promoted at the host's reveal without re-declaring. A client still settling is promoted exactly once, after both reveals, in either order.
- [ ] A mod-digest demotion and recovery re-promotes a Running client without a new revealed declaration.
- [ ] A client that disconnects while held spawns no pawn, and its revealed record is gone. Its rejoin is held until it declares revealed again.
- [ ] A revealed-only demotion sends a Holding diagnostic and retires the client's epoch.
- [ ] The revealed declaration and both revealed holding causes survive an encode/decode round trip, and a peer on the previous wire constants is refused at gate 1.

**Warm horizon and diagnostics**
- [ ] The warm walk stops at the new cluster count, with the camera's own cluster included.
- [ ] Over budget, optional prefetch yields farthest-first while Visible and Pinned targets are still admitted.
- [ ] The re-read counter increments when an evicted cluster is read again, and does not increment on a cluster's first read.
- [ ] A visible miss is counted in exactly one of three buckets: never targeted, in flight, or held by the install cap.

### Manual
- [ ] Visual: entering campaign-test and the largest available stress map, including by `restartLevel`, never shows a frame of ambient-floor SH in the spawn view.
- [ ] Resource run (`testing_guide.md` §Resource bounds): a pinned walking route on the largest stress map. Record misses, evictions, re-reads, peak resident SH bytes and Settling hold duration, before and after. Pin the fixture, route, machine class, cache mode and baseline (main before this brief). Rebuild stale stress PRLs first; some predate ids 49/50. The fixture must have more playable clusters than the chosen warm count, so the budget and departure eviction are exercised. Neither `campaign-test` nor `stress-warren-mini` qualifies once the count reaches their playable cluster totals.
- [ ] The chosen warm count cuts in-play misses on that route compared with baseline, and peak resident bytes stay within the budget, apart from mandatory overshoot.
- [ ] Co-op loopback: a host and a client load the same level. Both settle, and neither sees the other's pawn move before its own reveal. Across a host `restartLevel` with a long settle, the client never sees its own pawn act before the host reveals.

## Path
- Settling needs visibility and SH pumping without the sim tick or world draw. Today all of them sit in main.rs's Running-only per-frame block, reached through `drive_boot_state_for_redraw`. The sim's camera-follow feeds visibility, and the SH drain runs inside `render_frame_indirect`. The first slice isolates visibility → `prepare_sh_streaming_drain` → drain, compose and submit as a call Settling can make from the spawn camera, proving the split before the state exists. Strongest rival: a post-install sub-phase of Loading; it loses because once install has run a world exists, and request draining, parity and dev tooling must see it as installed. Other rivals: `research.md`.
- Settle predicate: `ShResidencyController::all_targets_sampleable` / `ShStreamingSession::all_targets_sampleable` exist behind `cfg(feature = "capture")`. `capture/prepared.rs` `preload_visible_sh` is already a bounded settle loop. Narrow the predicate to the entry settle set and lift the gate.
- Revealed: the term joins `parity_cause` / `NetServer::reevaluate_parity`, after the parity checks. The client record mirrors `NetClient::set_level_parity` / `parity_sent` in `queue_control_messages`; `close_slot` clears the host record. `NetEndpoint::set_level_parity` is the dual-role precedent for the reveal setter, and `clear_net_level_parity` retracts both. `client_drain_control` logs content-parity holds at warn; revealed holds should not. The net half is provable in the in-memory harness before Settling exists.
- `WARM_SET_CLUSTERS` is asserted in `controller_warm_and_budget_tests.rs`. The eviction path is `take_async_drain_batch` → `departed_eviction_order`; the pressure comparator is `compare_pressure_keys`. The re-read counter joins `ShStreamingLiveDiagnostics`, with its log line and dev-tools Streaming tab entry.
- Derivation, pop-in source analysis and the co-op participation trace: `research.md`.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants); no manual endianness, prefixes or counts. Every addition is appended, following the `JoinSeed` and `SessionRoster` precedent.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Revealed-level declaration | `ClientControlMessage`, after `JoinSeed` | Control, client → host | revealed level identity `Option<String>` (the parity string) | `None` = not revealed |
| Host-not-revealed hold | `HoldingCause`, after `LevelDigest` | inside `Divergence`, host → client | host level identity `String` | — |
| Client-not-revealed hold | `HoldingCause`, after the host variant | inside `Divergence`, host → client | host level identity `String` | — |

The declaration is re-sent on change, like parity, and the host keeps the last one per slot. Holding-cause order is diagnostic precedence, so both revealed causes rank below every content cause.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Revealed-level declaration | new `ClientControlMessage` variant; `NetClient` record and sent flag; `NetEndpoint` reveal setter for both roles | appended variant, Control | none | n/a |
| Revealed holding causes | two appended `HoldingCause` variants | inside `Divergence` | none | n/a |

## Open questions
- New `WARM_SET_CLUSTERS` value — **delegated**: pick from the resource run and report misses against resident bytes.
- Whether Settling's frames lift the per-drain install cap, since no world frame is at stake — **delegated**. Report the effect on hold duration.
- Whether the new variants bump `WIRE_VERSION`, as the `JoinSeed` precedent did, or also `PROTOCOL_ID`, as `networking.md` §Two-gate handshake words a vocabulary change — **delegated**
