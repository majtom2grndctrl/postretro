# sh-streaming--reveal-gate-and-budget-tiers

Brief · compact · **gated** · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, `context/lib/boot_sequence.md` §1, §3, §4, `context/lib/player_options.md`, `context/lib/testing_guide.md` §Resource bounds · read at 0a7352039 (`feat/preferences-comfort-floor`)

Build after `E23--preferences-comfort-floor` lands. That branch edits `options/` and `startup/`, both of which this brief touches.

## Problem
The owner saw SH lighting fill in on screen, both on level entry and in play, while VRAM use stayed low. Basis: an observed defect. On entry, the splash clears as soon as CPU install finishes, and SH residency starts empty, so the first frames show the ambient-floor fallback. In play, the async drain evicts every cluster that leaves visible ∪ warm set ∪ hysteresis tail, even when the pool has free space. The warm horizon is a small fixed count, so a cluster just past it, or one left more than the hysteresis window ago, needs a cold read in view. The single budget was sized for a 6 GB desktop GPU and has no laptop setting. When done: a level is not revealed until its SH targets are resident, or until a generous timeout runs out. In play, a much wider horizon stays resident, capped only by a budget whose tier suits the machine, down to integrated laptop GPUs.

## Decisions
- **Settling boot state.** A new state sits between Loading and Running. The splash stays painted, the sim does not tick, and no world frame is presented. Visibility runs from the spawn camera and SH streaming is pumped until every current target is Sampleable. "Current target" means every class: Visible, Pinned, SeamWarm, Prefetch, and Hysteresis at entry. Precedent: the first-launch hold is also its own boot state (`boot_sequence.md`), not a pause flag. The sim-not-ticking rule is load-bearing, because nothing may act on the player while they can't see.
- **Every level entry settles.** Boot, `loadLevel` and `restartLevel` all go through Settling. The frontend background level does not settle: its menu already covers the world, and delaying the menu costs more than a brief SH fill behind it.
- **Timeout, never hang.** Settling releases after a fixed timeout, default 10 s. On release it logs a warning naming the unsettled target count and reveals with the existing ambient-floor miss fallback. The timer starts fresh on each entry to Settling. A load request arriving during Settling follows the ordinary Unload→Loading path.
- **In-play misses still never stall.** The gate applies only at level entry. It leaves in place the `sh-probe-streaming` rule that doors never wait on residency and in-play misses fall back to ambient floor (`plans/done/sh-probe-streaming` §Open questions).
- **Wider warm horizon, budget as the limiter.** `WARM_SET_CLUSTERS` rises to a value chosen by measurement. Budget pressure is the only thing that trims it. Pressure already yields optional work farthest-first. Hysteresis-departure eviction stays. The io-contract plan declared the warm count "tuning, not contract" (`plans/done/sh-streaming--warm-set-and-io-contract.md` §Open questions).
- **No preference for previously visited clusters.** Backtracking is uncommon in some retro-FPS styles, so retention stays proximity-driven. A diagnostic counts clusters read again after being evicted, so a later change can argue from evidence.
- **Budget tiers.** A settings-file option: Auto, Low, Medium, High. Default is Auto. Auto resolves from the adapter's device type at apply time: discrete → High, everything else → Low. Auto persists as Auto, so saving never pins the resolved tier. High keeps today's 256 MiB budget. Low is sized for integrated laptop GPUs. The tier applies at full-init and at the next level install, never mid-level, the same as `ShadowQuality` (`player_options.md`). The one resolved byte budget feeds both the renderer's initial pool floor and the planner's accounting. Placement: engine policy with a player override. No menu entry and no scripting surface, which matches `ShadowQuality`.
- **Non-goals.**
  - Streaming lightmaps, shadowmasks, direction or animated-lightmap data: that is stage 5 of `context/plans/large-map-spatial-residency.md`, a separate plan.
  - A tier above today's 256 MiB.
  - A cross-resource VRAM budget. It waits for a second streamed resource.
  - Changing the per-drain install cap or read coalescing, unless measurement shows they cause pop-in.

## Acceptance
### Automated
**Settle predicate**
- [ ] Not settled while any current target (any class) is below Sampleable. Settled once all are Sampleable.
- [ ] Settled immediately when there are zero targets: streaming off, or a level without a valid id-49/id-50 pair.
- [ ] A target added during Settling, such as a seam warm-up, holds the gate until it is Sampleable.

**Settling lifecycle**
- [ ] The sim tick count and game time do not advance during Settling. The first tick happens after the switch to Running.
- [ ] No world frame is presented during Settling. The splash is painted on each redraw.
- [ ] The timeout releases to Running and emits exactly one warning, verified by log capture. A settle just before the deadline releases with no warning.
- [ ] `restartLevel` after a timed-out entry gets a full, fresh timeout.
- [ ] A load request during Settling unloads and reloads without a stale settle or stale timer carrying over.
- [ ] The frontend background level never enters Settling.
- [ ] Regression guard: after Running, a visible cluster that is not resident renders the ambient-floor fallback on that same frame. No frame waits for it.

**Budget tiers**
- [ ] Auto resolves to High on a discrete adapter and to Low on integrated, virtual, CPU and other.
- [ ] Auto, Low, Medium and High each round-trip through the settings file. Saving under Auto writes Auto.
- [ ] A tier change during a level does not change the live budget. The next level install uses it.
- [ ] Renderer floor and planner accounting receive the same byte budget for each tier.
- [ ] At Low, optional prefetch yields farthest-first under pressure while Visible and Pinned targets are still admitted. The pool overshoots rather than evicting them.
- [ ] Under budget, no optional target yields.

**Warm horizon and diagnostics**
- [ ] The warm walk stops at the new cluster count, with the camera's own cluster included.
- [ ] The re-read counter increments when an evicted cluster is read again, and does not increment on a cluster's first read.

### Manual
- [ ] Visual: entering campaign-test and the largest available stress map, including by `restartLevel`, never shows a frame of ambient-floor SH. Tested at High and at Low.
- [ ] Resource run (`testing_guide.md` §Resource bounds): a pinned walking route on the largest stress map. Record misses, evictions, re-reads, peak resident SH bytes and Settling hold duration at each tier, before and after. Pin the fixture, route, machine class, tier, cache mode and baseline (main before this brief).
- [ ] The chosen warm count cuts in-play misses on that route compared with baseline, and peak resident bytes stay within the tier budget, apart from mandatory overshoot.
- [ ] Integrated-GPU laptop run at Auto (→ Low): the level settles and plays. Hand off if no hardware is available.

## Path
- Settling needs visibility and SH pumping without the sim tick or world draw. Today all four sit in main.rs's Running-only per-frame block, reached through `drive_boot_state_for_redraw`, with the sim's camera-follow output feeding visibility. The first slice isolates visibility → `prepare_sh_streaming_drain` → renderer drain as a call Settling can make from the spawn camera. It proves the split before the state exists. Rival: enter Running and suppress the sim tick and world draw behind a flag. It was rejected because it spreads a boot concern through the frame loop.
- Settle predicate: `ShResidencyController::all_targets_sampleable` / `ShStreamingSession::all_targets_sampleable` already exist, behind `cfg(feature = "capture")`. Lift that gate and reuse them. `capture/prepared.rs` is the existing consumer.
- Tier plumbing: follow `ShadowQuality` → `configure_player_shadow_quality`. The device type comes from `adapter.get_info()` in renderer init. The budget constants are `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` (`plan_initial_pool_floor`) and `DEFAULT_GPU_FLOOR_BYTES` (`ShResidencyAccounting`). Replace both with the one resolved value.
- `WARM_SET_CLUSTERS` is asserted in `controller_warm_and_budget_tests.rs`. The eviction path is `take_async_drain_batch` → `departed_eviction_order`. The pressure comparator is `compare_pressure_keys`.
- Diagnostics live in `ShStreamingLiveDiagnostics`. Add the re-read counter there, with its log line and dev-tools Streaming tab entry.
- Derivation and pop-in source analysis: `research.md`.

## Open questions
- Low and Medium byte values — **delegated**: pick from the resource run, with Low sized for integrated laptop GPUs, and report the numbers.
- New `WARM_SET_CLUSTERS` value — **delegated**: pick from the resource run at High. At Low the budget trims the horizon.
- Whether Settling's frames lift the per-drain install cap, since no world frame is at stake — **delegated**. Report the effect on hold duration.
