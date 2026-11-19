# streaming-drain-time-budget

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §12 (CPU Stage Timing, GPU Pass Timing, Machine-state confounders) · `context/lib/testing_guide.md` §Resource bounds · read at 1eebe8b79 · evidence: `research.md`, `plans/done/streaming-drain-budget-spike/findings.md`

## Problem
The `streaming-drain-budget-spike` findings showed that the shared per-drain install budget charges the wrong unit. The budget caps decoded bytes, but a byte does not cost the same for both resources. A warm lightmap pair costs about 15 µs plus 0.2 ms/MiB of frame-thread CPU. An SH chunk costs about 65–70 µs plus 1.0–1.2 ms/MiB, compose included. The 8 MiB cap is therefore too small for lightmaps: their late block-frames on the showcase's far-point walk fall from 180 to 145 at 16 MiB, which still costs only about 3.5 ms. It is too large for SH: the showcase and campaign-test both admit SH drains of 7.9 MiB, about 8–11 ms on one frame. In the slider read, the owner saw pop-in clearly at 4 MiB and some stutter they could not tie to any cap. When this is done, each drain admits work up to an estimated install time instead of a byte count. Lightmap bursts drain faster than today, and no drain's estimated cost passes the budget except a lone first item.

## Decisions
- **The budget's unit is estimated frame-thread install time.** Each ready item is charged a per-resource fixed cost per item plus a cost per MiB of decoded bytes. These coefficients are engine constants taken from the spike's install-cost model: the warm CPU fit, with SH's drain-plus-compose figures, since compose is work SH installs trigger. The one-owner admission contract stays as `plans/done/spatial-residency--lightmap-cell-blocks` set it: one budget, one admission order, the first item always admitted, admission stopping at the first item over budget. The SH-first tie-break at equal class stays (spike findings: keep). Rival: runtime calibration from measured install CPU. It adapts to the machine, but it makes admission depend on timing, so walks and tests stop being deterministic, and a feedback loop on frame time can oscillate. Undo cost is low: the coefficients are constants behind one estimate.
- **CPU, not GPU.** The drain blocks the frame thread for its CPU cost. GPU cost was measured only as idle-clock upper bounds (lightmap about 42 µs/pair + 0.22 ms/MiB). This brief records GPU time per drain on its proof route but does not charge it. If the route shows GPU install time dominating, that is a finding for a follow-up, not a change here.
- **The play budget is 4 ms**, a quarter of a 60 Hz frame: the ceiling the spike judged a drain against. **The settling budget** is also time. Its value is the executor's, with the constraint below: no world frame is drawn while settling, so it only paces the loading tree.
- **The lightmap in-hand caps become their own byte constants**, at today's totals (32 MiB, the band half of it). Today they are defined as four drain caps (`MAX_IN_HAND_PAIR_BYTES`), a unit this brief retires. They bound memory, which is bytes. The spike's sweep ran with them pinned at these totals.
- **The dev-tools drain-cap slider becomes a time slider** with the same rules: dev-tools only, play cap only, unset means production. The walk's test-only override (`POSTRETRO_LIGHTMAP_WALK_DRAIN_CAP`) becomes a time override, and so does the slider's launch seed that camera-path playback sets per run.
- **Diagnostics compare estimate with measurement.** Each drain's estimated cost is reported beside its measured install CPU, per resource, in the log line, the Streaming tab, the walk results and camera-path playback's per-frame record, so model error is visible in play.
- **The proof route is camera-path playback** (`plans/ready/camera-path-playback/`): a repeatable, rendered walk of a pose file with the budget seeded at launch. This brief builds after that tool lands.
- **Non-goals.**
  - SH eviction cost (about 0.4 ms/MiB): evictions are forced, not admitted, so the budget cannot hold them back. It is reported, not charged.
  - Read permits (32) and the decode and completion queues sized to them: the next limit after the budget (spike findings), but changing them changes a resource bound. Own brief.
  - The staging pool's thrash near 32 MiB: a direct fix of its own (spike findings).
  - Outside-reach misses (251 of the random walk's 306): lead distance against walk speed, which no budget moves.
  - Per-machine coefficients, and the Windows perf target's own fit.

## Acceptance
### Automated
- [ ] Two items with equal decoded bytes, one SH chunk and one lightmap pair, are charged different estimated times, each matching its resource's fixed and per-MiB constants.
- [ ] A drain admits in the same order as today. Its first item is admitted whatever its cost. Admission stops at the first item whose cost would pass the budget, even if a later, smaller item would fit.
- [ ] A drain of N lightmap pairs whose total estimated time fits the budget is admitted whole even when its bytes exceed 8 MiB; a drain of SH chunks over the budget is cut.
- [ ] The log line, the Streaming tab, the walk results and the playback per-frame record report each drain's estimated and measured install CPU per resource.
- [ ] Settling admits against the settling budget, and play against the play budget or the dev-tools override; settling never reads the override.
- [ ] The in-hand and band caps keep their byte totals at every budget and override value.
- [ ] Grep gate: the time slider builds only with dev-tools; the walk override lives only in test code; no player setting names the budget.
- [ ] The showcase far-point walk, realtime, SH on, default cores: late lightmap block-frames at most 150 and late SH cluster-frames at most 44 (the spike's 16 MiB and production results). The largest estimated drain is at most 4 ms, apart from a lone first item.
- [ ] Campaign-test walks: late lightmap block-frames stay 0, and late SH cluster-frames are no higher than at the byte cap.
- [ ] The level-entry settle hold on the showcase and campaign-test is no more than 10% longer than at the byte cap.
### Manual
- [ ] On a camera-path playback run of the showcase far-point route (smoothed, wall-clock pacing, vsync on), Mac perf target, budget seeded at launch and the slider left alone: measured install CPU per drain stays within 25% of its estimate at p95, and the drain stage's per-window maximum is at most about 5 ms apart from lone first items. Frame-time p99 and frames over 33 ms are no worse than at the 8 MiB byte cap. GPU install time per drain is recorded beside them, with the GPU clock.
- [ ] Owner read with the time slider at a showcase burst: pop-in no worse than at the byte cap, and no stutter that tracks the drain.

## Path
- Seams: `streaming/drain_budget.rs` (`DrainItem`, `admit_drain`, the two byte constants) and `streaming/shared_drain.rs` (`admit_within`). Item construction for SH and lightmap pairs, and the in-hand cap in `lightmap_streaming/controller.rs`. The lever: `session/level_streaming/drain_cap.rs` and `renderer/src/render/debug_ui/drain_cap_lever.rs`. Walk results: `walk_measurement/results.rs`.
- Shape: an item's cost is an integer in microseconds, computed once when the item is offered. The rival is charging bytes scaled per resource, a weight. It is the same model without a time unit, and it reads worse in diagnostics.
- First slice: the cost estimate and admission against 4 ms, then the showcase far-point walk. A budget that slows SH past 44 late cluster-frames falsifies the 4 ms value before the levers and diagnostics are built.
- Re-run `lightmap_install_cost_model` and `sh_install_cost_model` (release, `--ignored`) to confirm the constants before freezing them.

## Open questions
- If a 4 ms budget raises late SH above 44 cluster-frames on the far-point walk, does 4 ms hold (accept more SH pop-in, relax that row), or does the play budget rise until SH meets it? — owner — **blocks build**
- The settling budget's value — **delegated**: the executor picks it against the settle-hold row and reports it in the plan of record.
