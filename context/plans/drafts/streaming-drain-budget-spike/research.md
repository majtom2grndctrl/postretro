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
- Campaign-test has 0 lightmap late blocks in every walk cell, realtime included (`lightmap-compact-storage/plan.md`, Task 6 follow-up). Its lightmap side cannot open the gate.
- The 0.0125 m showcase is the project's 0.1 m source (sha256 `8f40118e…`) with worldspawn `_lightmap_density` set to 0.0125, kept as a frozen scratchpad copy (`.map` sha256 `6f937774…`). It is not in this repository, and a scratchpad does not outlive its session.

## Where SH and lightmap misses are classified
- SH counts a visible miss once per continuous episode and takes its cause from the episode's first frame (`sh_streaming/targeting.rs` `record_visible_misses`, `miss_bucket`). It records misses in `update_targets`, before the frame's completions are admitted (`session/sh_residency/frame_drain.rs`).
- SH's held-by-drain-budget bucket is the `Ready` state. A chunk the renderer defers under retirement pressure returns to `Ready` and lands there (`sh_streaming/lifecycle.rs`). The walk accepts every SH chunk (`sh_walk.rs` header), so walks never produce that case.
- Lightmap: the controller counts drawn-not-resident block-frames after the frame's drain, for blocks it holds (`lightmap_streaming/controller.rs`). The walk's late-block tally covers every drawn block (`walk_measurement/frame_driver.rs`), so on a held frame the two differ.
- The in-hand cap counts pairs in flight, ready, or in a drain (`lightmap_streaming/controller.rs`). New reads wait once it is full (`lightmap_streaming/reads.rs`), so ready pairs the budget leaves behind can stall the next demanded read.
- The SH-first tie at equal class also orders decode pickup (`streaming/decode_pool.rs`; `rendering_pipeline.md` §4).

## Pins
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Drain at 8 MiB offers SH Visible 3 MiB ×3 and one lightmap Visible pair | Sort SH, SH, SH, LM; the cut falls after the second SH | Counter +1: an SH Visible item installs while a lightmap Visible item waits, though the cut falls between two SH items |
| P2 | First item an oversized SH Visible chunk, next a lightmap Visible pair | The first-item rule admits the chunk alone | Counter +1. An oversized lightmap first item with no same-class SH item: unchanged |
| P3 | Drains with no items; items of one resource only; a cut at a class boundary; everything admitted | Any | Counter unchanged |
| P4 | Lightmap batch still outstanding (the renderer skipped its drain) while SH is cut inside a class | Lightmap offers nothing that drain | Counter unchanged; a drawn ready block not offered is not held by the drain budget |
| P5 | Settling drain with a same-class cut, either lever set | Settling branch of the one `admit_within` call | Admits against the settling budget; counter unchanged; no miss counted on either side |
| P6 | A pair deferred behind a retiring generation in drain 0, offered past the cut in drain 1 | Retirement advances once per frame, so drain 1 defers it again | Deferred bucket, not held by the budget |
| P7 | The budget leaves ready pairs in hand until the in-hand cap is full; a newly demanded block cannot submit | Cut → ready pairs stay in hand → read waits | Waiting-on-the-in-hand-cap bucket, apart from reads in flight and from the budget bucket |
| P8 | Drawn block whose read, decode or install failed | Any | Failed bucket, never "read in flight" |
| P9 | Held (non-portal) frame draws blocks the controller does not hold | The controller counts only blocks it holds | Buckets sum to the controller's count; the gap is its own reported line |
| P10 | Teleport: a cluster first drawn while its read is outstanding, then Ready past the cut for 5 frames | Episode recorded on frame 1, before completions are admitted | Episode buckets: read in flight 1, held by budget 0. Per-frame tally: 1 in flight, 5 held by budget |
| P11 | In play, the renderer defers an SH chunk under retirement pressure | The chunk returns to Ready before the next miss record | Stays in SH's held-by-budget bucket; walks never produce it; the note says so |
| P12 | Install-cost run whose drain size first exceeds every earlier drain | The staging pool creates a buffer mid-run | That drain is cold: excluded from warm figures, counted apart |
| P13 | Walk spawn step | Step 0 admits at the play cap, never settling | Spawn buckets are not read as level-entry cost and stay out of the gate share |
| P14 | One frame, SH and lightmap both drawn | SH classifies from the prior drain's outcome, before this frame's admission; lightmap classifies after this frame's outcome | Each side's held-by-budget names the drain before its classification point |
