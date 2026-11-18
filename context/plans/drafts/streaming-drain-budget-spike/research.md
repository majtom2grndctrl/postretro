# streaming-drain-budget-spike — research

Read at e0d708672. This spike builds on `lightmap-compact-storage`'s decode pool, live-class target set and walk diagnostics, all on `main`.

## Where the budget lives
- `streaming/drain_budget.rs`: `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` (8 MiB) and `SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN` (32 MiB, no world frame drawn). `admit_drain` sorts by `DrainRank` (class, then resource with SH first, then priority, lead, key), always admits the first item, and stops at the first item over budget rather than skipping ahead.
- `session/level_streaming/mod.rs`: the one `admit_within` call picks the settling or play cap for both resources.
- `lightmap_streaming/controller.rs`: `MAX_IN_HAND_PAIR_BYTES = 4 × MAX_INSTALL_DECODED_BYTES_PER_DRAIN`, and `MAX_BAND_IN_HAND_PAIR_BYTES` is half of it. Changing the drain cap moves both unless the sweep decouples them.

## Why the budget is shared (the reasons the spike keeps)
- `plans/done/spatial-residency--lightmap-cell-blocks/index.md`, decision "One read issuer, one budget owner": one owner decides how much work a drain carries across both resources; mandatory reads of both go before optional reads of either; a second cap at a boundary would repeat the retired-cap bug.
- `plans/done/sh-streaming--warm-set-and-io-contract.md`: the retired two-ready-cluster cap survived in `ShDrainBatch::validate_contract` and failed campaign-test's first drain. The same note lists the byte budget among "first guesses to be tuned from the new diagnostics on real content".

## What is measured today
- SH: `MissCause` buckets including `HeldByDrainBudget` (`sh_streaming/targeting.rs`); the walk prints them (`sh_walk.rs`) and writes `sh_late_held_by_drain_budget` (`results.rs`); SH install CPU (`install_cpu_max_drain_micros`, `install_cpu_max_steady_drain_micros`).
- Lightmap: `late_blocks = deferred + left_behind` (`results.rs`); `left_behind` covers undemanded, unread, over the install budget, or refused (`report.rs`) with no split. Drain install bytes p95/max are reported. CPU stage `StreamingStage::LightmapDrain` covers the renderer's lightmap drain.
- The walk harness stands in for the renderer (`sh_walk.rs` header), so its timings never include GPU work or real uploads.
- Renderer-level drain timing exists: the ignored test `lightmap_drain_install_timing` (`renderer/src/lighting/lightmap/stream/install_timing_measurement.rs`). `plans/done/spatial-residency--lightmap-cell-blocks/findings.md` measured it: first drain on a cold staging pool 2.44 ms/MiB, warm drains 0.25 ms/MiB (about 2 ms at the 8 MiB cap); capture preloads run at the cold rate (2.0–3.4 ms/MiB). No SH equivalent exists.
- GPU: the Mac perf target has no timestamp queries; per-pass time comes from Metal System Trace reduced per encoder (`rendering_pipeline.md` §12).

## Prior numbers (from `lightmap-compact-storage`, showcase at 0.0125 m)
- Realtime late blocks 181 far-point, 306 random; SH `sh_late` far-point 44 at default cores, 47 at emulated 1 core.
- Largest drain install 14.7 MB at baseline (the first-item rule lets a drain exceed the cap).
- Completed mode, far-point: SH off 79 late blocks, SH on 78. SH off is the worse one, so it is not evidence of SH crowding lightmaps out; cause not isolated.
