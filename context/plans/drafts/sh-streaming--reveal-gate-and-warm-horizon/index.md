# sh-streaming--reveal-gate-and-warm-horizon

Brief · compact · **gated** · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, `context/lib/boot_sequence.md` §1, §3, §4, `context/lib/networking.md` §Admission and content parity, `context/lib/testing_guide.md` §Resource bounds · read at 0a7352039 (`feat/preferences-comfort-floor`)

Build after `E23--preferences-comfort-floor` lands. That branch edits `startup/`, which this brief also edits, and two of its rules must extend to Settling (see Decisions).

## Problem
The owner saw SH lighting fill in on screen, both on level entry and in play, while VRAM use stayed low. Basis: an observed defect. On entry, the splash clears as soon as CPU install finishes, and SH residency starts empty, so the first frames show the ambient-floor fallback. In play, the warm horizon is a small fixed count of clusters, so walking a few rooms ahead, or returning after the hysteresis window, brings a cluster into view cold. When done: a level is not revealed until the SH around the spawn view is resident, or until a generous timeout runs out. In play, a much wider horizon is prefetched, and the existing budget trims it.

## Decisions
- **Settling boot state.** A new state sits between Loading and Running. The splash stays painted, the sim does not tick, no world frame is presented, and no sound plays. Level sounds start at reveal. Settling frames run visibility from the spawn camera, then the SH drain, compose and submit, so clusters reach Sampleable. Precedent: the first-launch hold is also its own boot state, where no level runs and no sound plays (`boot_sequence.md`); it is not a pause flag. The sim-not-ticking rule is load-bearing, because nothing may act on the player while they can't see.
- **One settle chokepoint.** Whether Settling may end is decided in one named lifecycle check. It asks each streamed resource whether it is settled. SH is the only one today. Lightmap-shaped streaming (`context/plans/large-map-spatial-residency.md` stage 5) joins the same check.
- **Settling is a splash frame for E23.** The flash limiter's splash hand-off and the rule that global input never latches across a splash or Loading frame (`in-progress/E23--preferences-comfort-floor` Decisions) both treat Settling frames as splash frames.
- **Entry settle set.** Settling waits for:
  - every Visible and Pinned target, with their owner closure;
  - the nearest warm clusters, up to a fixed entry count equal to today's warm count.
  While Settling, those entry warm clusters are mandatory. They may push past the budget and are never refused for pressure. Otherwise a map near its budget would wait out the timeout on every entry. At reveal they become optional prefetch again, and pressure trims them normally. Wider prefetch and seam warm-up keep filling after reveal. This keeps entry hold time independent of the horizon width.
- **Every level entry settles.** Boot, `loadLevel` and `restartLevel` all go through Settling. The frontend background level does not settle: its menu already covers the world, and delaying the menu costs more than a brief SH fill behind it.
- **Co-op parity at reveal.** Level parity is published on the Settling→Running switch, not at install. The endpoint keeps polling during Settling the same way the world-less transport does (`networking.md`), so a long hold neither promotes a pawn nor drops a peer. Today install publishes parity, which is before a player can see.
- **Co-op client view: accepted limit.** A client learns its real spawn only after it publishes parity and is promoted, so it settles the view at the first `player_spawn`. After promotion it may jump to a cold spot, which the ambient-floor fallback covers. Parity now also means "revealed", on top of its content-identity meaning in `networking.md`. A future peer-readiness signal replaces that double meaning; it does not sit beside it. The follow-up that smooths the client's entry is investigated separately.
- **Timeout, never hang.** Settling releases after a fixed timeout, default 10 s. On release it logs a warning naming the unsettled target count and reveals with the existing ambient-floor miss fallback. The timer starts fresh on each entry to Settling. A load request arriving during Settling follows the ordinary Unload→Loading path.
- **In-play misses still never stall.** The gate applies only at level entry. It leaves in place the `sh-probe-streaming` rule that doors never wait on residency and in-play misses fall back to ambient floor (`plans/done/sh-probe-streaming` §Open questions).
- **Wider warm horizon, budget as the limiter.**
  - `WARM_SET_CLUSTERS` rises to a value chosen by measurement.
  - The horizon stays a cluster count. This is an owner experiment. Reach varies with cluster size (`large-map-spatial-residency.md` §Lessons from SH residency). A path-distance horizon is the fallback if the count keeps misbehaving.
  - Budget pressure trims the horizon, farthest first.
  - Hysteresis-departure eviction stays, so residency stays proximity-driven. The owner chose this knowingly: on a map that fits, evicting a departed cluster frees no VRAM. The reason is that backtracking is uncommon in some retro-FPS styles.
  - The io-contract plan declared the warm count "tuning, not contract" (`plans/done/sh-streaming--warm-set-and-io-contract.md` §Open questions).
- **Pop-in evidence.** Diagnostics count clusters read again after being evicted. The miss counter also separates three cases: a visible cluster that was never targeted, one targeted but still in flight, and one held back by the install cap. Together these show which lever a later change should pull.
- **Non-goals.**
  - Budget tiers and laptop defaults. They wait for a resource-neutral budget that covers lightmap-shaped data (`context/plans/large-map-spatial-residency.md`, Decisions still open). This brief keeps today's single budget.
  - Keeping departed clusters until pressure (owner decision above).
  - Streaming lightmap-shaped data.
  - Changing the per-drain install cap or read coalescing, unless measurement shows they cause pop-in.

## Acceptance
### Automated
**Settle predicate**
- [ ] Not settled while any Visible or Pinned target, or a cluster in their owner closure, is below Sampleable.
- [ ] Not settled while any of the nearest warm clusters, up to the entry count, is below Sampleable.
- [ ] Settles with a farther warm cluster or a seam-warm target still cold.
- [ ] Settled immediately when there are zero targets: streaming off, or a level without a valid id-49/id-50 pair.
- [ ] Raising the warm count does not change which clusters the entry settle set contains.
- [ ] With a budget below the entry settle set's bytes, Settling still settles before the timeout. The entry warm clusters are admitted past the budget.
- [ ] On the reveal frame, entry warm clusters become optional again. Under pressure they yield farthest-first on the next drain.

**Settling lifecycle**
- [ ] The sim tick count and game time do not advance during Settling. The first tick happens after the switch to Running.
- [ ] No world frame is presented during Settling. The splash is painted on each redraw.
- [ ] No sound plays during Settling. Level sounds queued by install begin on the reveal frame.
- [ ] A global input pressed during Settling does not latch into the first Running frame.
- [ ] The flash limiter treats the Settling→Running reveal as it treats the splash hand-off today.
- [ ] A cluster requested during Settling reaches Sampleable without a switch to Running, which proves the drain, compose and submit all run.
- [ ] The timeout releases to Running and emits exactly one warning, verified by log capture. A settle just before the deadline releases with no warning.
- [ ] `restartLevel` after a timed-out entry gets a full, fresh timeout.
- [ ] A load request during Settling unloads and reloads without a stale settle or stale timer carrying over.
- [ ] The frontend background level never enters Settling.
- [ ] Regression guard: after Running, a visible cluster that is not resident renders the ambient-floor fallback on that same frame. No frame waits for it.

**Co-op**
- [ ] Level parity is not published during Settling. It is published on the frame of the switch to Running.
- [ ] A peer connected across a Settling hold of at least the timeout stays connected, and its pawn is not ticked before the switch.

**Warm horizon and diagnostics**
- [ ] The warm walk stops at the new cluster count, with the camera's own cluster included.
- [ ] Over budget, optional prefetch yields farthest-first while Visible and Pinned targets are still admitted.
- [ ] The re-read counter increments when an evicted cluster is read again, and does not increment on a cluster's first read.
- [ ] A visible miss is counted in exactly one of three buckets: never targeted, in flight, or held by the install cap.

### Manual
- [ ] Visual: entering campaign-test and the largest available stress map, including by `restartLevel`, never shows a frame of ambient-floor SH in the spawn view.
- [ ] Resource run (`testing_guide.md` §Resource bounds): a pinned walking route on the largest stress map. Record misses, evictions, re-reads, peak resident SH bytes and Settling hold duration, before and after. Pin the fixture, route, machine class, cache mode and baseline (main before this brief). Rebuild stale stress PRLs first; some predate ids 49/50. The fixture must have more playable clusters than the chosen warm count, so the budget and departure eviction are exercised. Neither `campaign-test` nor `stress-warren-mini` qualifies once the count reaches their playable cluster totals.
- [ ] The chosen warm count cuts in-play misses on that route compared with baseline, and peak resident bytes stay within the budget, apart from mandatory overshoot.
- [ ] Co-op: a host and a client load the same level. Both settle, and neither sees the other's pawn move before its own reveal.

## Path
- Settling needs visibility and SH pumping without the sim tick or world draw. Today all of them sit in main.rs's Running-only per-frame block, reached through `drive_boot_state_for_redraw`. The sim's camera-follow feeds visibility, and the SH drain runs inside `render_frame_indirect`. The first slice isolates visibility → `prepare_sh_streaming_drain` → drain, compose and submit as a call Settling can make from the spawn camera. It proves the split before the state exists. Rejected rivals:
  - Enter Running and suppress the sim tick and world draw behind a flag. This spreads a boot concern through the frame loop.
  - A post-install sub-phase of Loading. This is the strongest rival. It loses because once install has run, a world exists, and request draining, parity and dev tooling must see that world as installed.
  - A synchronous preload at install, reusing capture's `preload_visible_sh`. It blocks the event loop and the transport poll.
  - Per-cluster SH fade-in. It breaks the invariant that a sampled slot matches what a full-resident compose would write.
- Settle predicate: `ShResidencyController::all_targets_sampleable` / `ShStreamingSession::all_targets_sampleable` exist behind `cfg(feature = "capture")`. `capture/prepared.rs` `preload_visible_sh` is already a bounded settle loop. Narrow the predicate to the entry settle set and lift the gate.
- Parity: `install_level_payload` calls `set_level_parity`. Move the call to the Settling→Running edge. For polling, see the world-less transport in `networking.md`.
- `WARM_SET_CLUSTERS` is asserted in `controller_warm_and_budget_tests.rs`. The eviction path is `take_async_drain_batch` → `departed_eviction_order`. The pressure comparator is `compare_pressure_keys`.
- Diagnostics live in `ShStreamingLiveDiagnostics`. Add the re-read counter there, with its log line and dev-tools Streaming tab entry.
- Derivation and pop-in source analysis: `research.md`.

## Open questions
- New `WARM_SET_CLUSTERS` value — **delegated**: pick from the resource run and report misses against resident bytes.
- Whether Settling's frames lift the per-drain install cap, since no world frame is at stake — **delegated**. Report the effect on hold duration.
