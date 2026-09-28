# sh-streaming--reveal-gate-and-warm-horizon

Brief · compact · **gated** · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, `context/lib/boot_sequence.md` §1, §3, §4, `context/lib/networking.md` §Admission and content parity, `context/lib/testing_guide.md` §Resource bounds · read at 0a7352039 (`feat/preferences-comfort-floor`)

Build after `E23--preferences-comfort-floor` lands. That branch edits `startup/`, which this brief also edits.

## Problem
The owner saw SH lighting fill in on screen, both on level entry and in play, while VRAM use stayed low. Basis: an observed defect. On entry, the splash clears as soon as CPU install finishes, and SH residency starts empty, so the first frames show the ambient-floor fallback. In play, the warm horizon is a small fixed count of clusters, so walking a few rooms ahead, or returning after the hysteresis window, brings a cluster into view cold. When done: a level is not revealed until the SH around the spawn view is resident, or until a generous timeout runs out. In play, a much wider horizon is prefetched, and the existing budget trims it.

## Decisions
- **Settling boot state.** A new state sits between Loading and Running. The splash stays painted, the sim does not tick, and no world frame is presented. Settling frames run visibility from the spawn camera, then the SH drain, compose and submit, so clusters reach Sampleable. Precedent: the first-launch hold is also its own boot state (`boot_sequence.md`), not a pause flag. The sim-not-ticking rule is load-bearing, because nothing may act on the player while they can't see.
- **Entry settle set.** Settling waits for:
  - every Visible and Pinned target, with their owner closure;
  - the nearest warm clusters, up to a fixed entry count equal to today's warm count.
  Wider prefetch and seam warm-up keep filling after reveal. This keeps entry hold time independent of the horizon width.
- **Every level entry settles.** Boot, `loadLevel` and `restartLevel` all go through Settling. The frontend background level does not settle: its menu already covers the world, and delaying the menu costs more than a brief SH fill behind it.
- **Co-op parity at reveal.** Level parity is published on the Settling→Running switch, not at install. The endpoint keeps polling during Settling the same way the world-less transport does (`networking.md`), so a long hold neither promotes a pawn nor drops a peer. Today install publishes parity, which is before a player can see.
- **Timeout, never hang.** Settling releases after a fixed timeout, default 10 s. On release it logs a warning naming the unsettled target count and reveals with the existing ambient-floor miss fallback. The timer starts fresh on each entry to Settling. A load request arriving during Settling follows the ordinary Unload→Loading path.
- **In-play misses still never stall.** The gate applies only at level entry. It leaves in place the `sh-probe-streaming` rule that doors never wait on residency and in-play misses fall back to ambient floor (`plans/done/sh-probe-streaming` §Open questions).
- **Wider warm horizon, budget as the limiter.**
  - `WARM_SET_CLUSTERS` rises to a value chosen by measurement.
  - Budget pressure trims the horizon, farthest first.
  - Hysteresis-departure eviction stays, so residency stays proximity-driven. The owner chose this knowingly: on a map that fits, evicting a departed cluster frees no VRAM. The reason is that backtracking is uncommon in some retro-FPS styles.
  - The io-contract plan declared the warm count "tuning, not contract" (`plans/done/sh-streaming--warm-set-and-io-contract.md` §Open questions).
- **Re-read evidence.** A diagnostic counts clusters read again after being evicted. If retaining departed clusters ever needs arguing, this is the evidence.
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

**Settling lifecycle**
- [ ] The sim tick count and game time do not advance during Settling. The first tick happens after the switch to Running.
- [ ] No world frame is presented during Settling. The splash is painted on each redraw.
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

### Manual
- [ ] Visual: entering campaign-test and the largest available stress map, including by `restartLevel`, never shows a frame of ambient-floor SH in the spawn view.
- [ ] Resource run (`testing_guide.md` §Resource bounds): a pinned walking route on the largest stress map. Record misses, evictions, re-reads, peak resident SH bytes and Settling hold duration, before and after. Pin the fixture, route, machine class, cache mode and baseline (main before this brief). Rebuild stale stress PRLs first; some predate ids 49/50.
- [ ] The chosen warm count cuts in-play misses on that route compared with baseline, and peak resident bytes stay within the budget, apart from mandatory overshoot.
- [ ] Co-op: a host and a client load the same level. Both settle, and neither sees the other's pawn move before its own reveal.

## Path
- Settling needs visibility and SH pumping without the sim tick or world draw. Today all of them sit in main.rs's Running-only per-frame block, reached through `drive_boot_state_for_redraw`. The sim's camera-follow feeds visibility, and the SH drain runs inside `render_frame_indirect`. The first slice isolates visibility → `prepare_sh_streaming_drain` → drain, compose and submit as a call Settling can make from the spawn camera. It proves the split before the state exists. Rival: enter Running and suppress the sim tick and world draw behind a flag. It was rejected because it spreads a boot concern through the frame loop.
- Settle predicate: `ShResidencyController::all_targets_sampleable` / `ShStreamingSession::all_targets_sampleable` exist behind `cfg(feature = "capture")`. `capture/prepared.rs` `preload_visible_sh` is already a bounded settle loop. Narrow the predicate to the entry settle set and lift the gate.
- Parity: `install_level_payload` calls `set_level_parity`. Move the call to the Settling→Running edge. For polling, see the world-less transport in `networking.md`.
- `WARM_SET_CLUSTERS` is asserted in `controller_warm_and_budget_tests.rs`. The eviction path is `take_async_drain_batch` → `departed_eviction_order`. The pressure comparator is `compare_pressure_keys`.
- Diagnostics live in `ShStreamingLiveDiagnostics`. Add the re-read counter there, with its log line and dev-tools Streaming tab entry.
- Derivation and pop-in source analysis: `research.md`.

## Open questions
- New `WARM_SET_CLUSTERS` value — **delegated**: pick from the resource run and report misses against resident bytes.
- Whether Settling's frames lift the per-drain install cap, since no world frame is at stake — **delegated**. Report the effect on hold duration.
