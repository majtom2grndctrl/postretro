# CPU Frame Profiling

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §12, `context/lib/networking.md` §Not netcode: the live introspection channel, `context/lib/development_guide.md` §1.4, `context/lib/testing_guide.md` §Resource bounds · read at 987f8c5bb

## Problem

A developer need, and the prerequisite for every CPU perf decision. The engine attributes GPU time per pass, but CPU time is one whole-frame number in the window title. Stages that plausibly cost milliseconds — the portal walk, the fixed-step sim (movers, movement, AI, triggers, weapons), script drains, render prep, per-pass command recording — have no individual measurement. That frame number likely includes the vsync wait too, so it cannot separate load from blocking. The CPU timers that do exist are bespoke and scattered. So the open portal-walk parallelization question, and any future AI or sim perf work, would be decided by guesswork. When done, a developer or agent runs any map with one env var and reads the result windowed per stage, with avg and max. Work and wait are separated, and an unattributed remainder exposes gaps. The same numbers appear in the log, the debug UI, over observe-live and in capture reports, and can stream to Tracy. Baselines for `stress-warren`, `stress-warren-crates` and `campaign-test` are recorded, and the parallelization question is answered against them.

## Decisions

- **Built-in stage timer is the floor.** Its scopes, per-frame fold and window-close arithmetic are allocation-free. Surfaces (log line, debug UI, observe-live reply, capture report) may allocate once per window. It windows over 120 in-level frames with avg and max per stage (max catches hitches that averages hide), matching GPU `FrameTiming`'s window length (`rendering_pipeline.md` §12). The two windows never align, because the GPU window counts completed readbacks. Stage labels are engine-closed; it is not a general profiler.
- **Stage tree.** Stages are hierarchical: a substage's time is inside its parent's, never added to it. The frame total splits into top-level stages, a **wait** bucket (the vsync/acquire/present block), and **unattributed** (total minus top-level stages and wait). Work CPU is total minus wait. The renderer times surface acquire and returns it upward, and the binary moves it and present's block out of their enclosing stages into wait, so the block counts exactly once whichever call blocks. A stage entered more than once in a frame sums its entries.
- **Windowing divides by frames run.** Each stage's avg and max cover only the frames it ran in, and each stage reports that frame count. Unattributed, work CPU and tick count are computed per frame, then windowed. "Inside its parent" is a per-frame property.
- **Window lifecycle.** A vsync toggle, level install, level unload or hot reload discards the partial window, as the frame meter already does on a vsync toggle. The install frame does not count, and a new level clears the previous level's last window from every surface. Frontend frames and early-returned frames never count; `FrameRateMeter` still covers frontend frames. A frame whose surface acquire yields no surface is discarded like an early return, so a timed-out acquire never inflates wait.
- **Depth.** Top-level frame stages follow the in-level frame order. Sim substages (movers, movement, AI, triggers/scripts, weapons) are summed across the frame's ticks, with ticks-this-frame reported beside them; client prediction gets its own labels. Portal walk: time plus traversal counters (considered, accepted, per-reason rejections, step-limit trips), portal path only. One CPU recording scope per renderer pass, owned by the renderer (renderer owns GPU). SH worker threads are out of scope; their `LatencyHistogram` stays.
- **Walk frames.** A portal-fallback frame contributes nothing to walk averages and increments a fallback count. A step-limit trip is a walk frame, not a fallback frame: its full walk enters the walk window and raises the step-limit count. The walk is the cost the gate exists to find, and the limit trips on the costliest walks.
- **Gate: `POSTRETRO_CPU_TIMING`, compiled in unconditionally.** This follows `POSTRETRO_GPU_TIMING`: runtime instrumentation behind env vars in every build, surfaces behind features. When the variable is off, the built-in timer accumulates, logs and allocates nothing. A clock read around a millisecond-scale stage is noise, and a diagnostic that only exists in some builds is worse for modders than one that is uniformly available.
- **Placement: the shared crate is mechanism only.** A leaf crate below `postretro-sim`, `postretro-visibility`, `postretro-renderer` and `postretro` holds the scope guard, the Tracy bridge and the window fold. It names no stage (`development_guide.md` §Layering invariants), so a new pass or substage recompiles its owning crate and that crate's dependents, not every crate above the leaf. Each crate owns its stage set and per-frame storage and returns stats upward through its existing outputs. The binary owns the top-level stages, wait, unattributed and every window. It places each crate's stats in the tree, but folds and displays their stages by label, never matching on each one, so a new stage inside an existing crate's set needs no binary edit. There is no shared global recording state.
- **Tracy behind a cargo feature.** The feature alone drives Tracy: scopes reach it whether or not `POSTRETRO_CPU_TIMING` is set. `observability`, `observe-live` and `capture` add no dependencies (`plans/done/agentic-observability`); Tracy stays out of all three, and release and dist builds never enable it (`build_pipeline.md` release stage 1). The shared guard earns its crate because one annotation per site feeds both the built-in timer and Tracy.
- **Surfaces.** An env-gated `[CpuTiming]` log line per window. A CPU block beside the GPU block on the debug UI Performance tab (`dev-tools`), which joins BIS's egui-retirement checklist (`context/plans/roadmap.md`). `cpu_stages` in the capture measurement report, following `gpu_timing`'s `availability`/`reason` pattern.
- **Dump timing is live-only.** Observe-live exposes the latest window as the first live-only section of the shared dump vocabulary. `plans/done/E20--live-socket-channel` anticipated live-only enrichments but built none. Wall-clock time cannot meet the batch dump's byte-identical rule, and batch runs have no frames to window, so timing is exempt from byte-identity. A batch runspec that requests it is rejected with an error that names the flag.
- **Absent is not zero.** On every surface, a stage that did not run in the window reports absent. Timing that is off reports unavailable with a reason. This matters most in capture, where no tick or walk runs per sample, so only render-recording stages carry numbers (`rendering_pipeline.md` §12).
- **Existing instruments.** Mesh pose sampling moves off `POSTRETRO_GPU_TIMING` onto the timer, as a renderer-owned substage of render recording, where it runs. SH streaming counters and `StartupTimings` stay as they are: they serve streaming diagnostics and boot, not frame cost.
- **Portal-walk parallelization gate.** Promote a parallelization plan only when both hold on `stress-warren`, at a new checked-in probe chosen for walk reach (its reach recorded beside the baseline), in a release build, over a 120-frame window: averaged walk time exceeds **0.5 ms**, and it is at least **5% of work CPU** over the same walk frames. The second condition keeps a regression elsewhere from making the walk look cheap, and keeps an absolutely-slow but pacing-irrelevant walk from triggering work. `plans/done/perf-visible-cell-candidate-cull` measured the walk as negligible on its map; the gate exists so the question is closed by a number. Constraints for that plan are in `research.md`.
- **Non-goals.** No parallelizing or optimizing anything measured, including the walk. No per-tick capture: making capture run ticks belongs to a future scripted-run capture plan (`E20--scripted-run-capture` is research only today). No per-script or per-reaction profiling: closed stage labels time script dispatch as a whole.

## Acceptance

### Automated
Behavior neutrality
- [ ] Routine suite: on a small portal fixture, visible cells, fog-reachable cells and the chosen visibility path are identical with timing on and off in one process. The checked-in stress probes repeat the check on demand. (P-gate)
- [ ] With timing off, a steady-state frame performs no allocation attributable to the timer, and no window accumulates.
- [ ] With timing on and Tracy off, a steady-state frame after the first window allocates nothing in the timer's scopes, per-frame fold or window-close arithmetic, including across a window close.
- [ ] The allocation proofs run where allocations are actually counted. Each includes a control that sees a deliberate allocation in the same window. Each measures only the timer's own work: opening and closing every stage scope, ending the frame, and closing at least two windows. (P-alloc)

Window accounting
- [ ] A frame with zero ticks records a zero tick count and no sim-substage time, and still counts toward the window.
- [ ] A frame with several ticks reports the tick count and sim substages summed across all of them.
- [ ] In every frame, no substage's time exceeds its parent's in that frame; unattributed is never negative.
- [ ] A stage that ran in some frames of a window averages over those frames only and reports how many it ran in. Sixty fallback frames and sixty walk frames yield a walk average over the sixty walk frames, never exceeding visibility's per-frame time in those frames.
- [ ] A frame whose walk trips the step limit adds its walk time and counters to the walk window, raises the step-limit count, and leaves the fallback count unchanged. (P-steplimit)
- [ ] With known stage and wait durations fed in, unattributed equals the frame total minus the top-level stages minus wait, with no clamping. A frame whose acquire or present blocks counts that block in wait and in no top-level stage. (P-wait)
- [ ] Frame 120 closes window one; frame 121 opens window two. Nothing carries across the boundary. Only counted in-level frames advance the window: 120 in-level frames close it, whatever number of frontend or early-returned frames fall between them. (P-close, P-count)
- [ ] An early-returned frame changes no window. A frame that took the portal fallback and then returned early changes neither the walk window nor the fallback count. (P-early-fallback)
- [ ] A portal-path frame adds its walk time and its counters (considered, accepted, and each rejection reason) to the window.
- [ ] A portal-path frame whose walk considers no portal records its walk time and zero counters; the walk is present, not absent. (P-zero-walk)
- [ ] A portal-fallback frame leaves walk averages unchanged and increments the fallback count. A window with only fallback frames reports the walk absent.
- [ ] A frontend frame changes no stage window.
- [ ] Step-limit trips report as a count of frames in the window, not an average.
- [ ] Two timers in one process never see each other's samples.
- [ ] A stage set defined only in a test, nested under an existing parent, has its labels appear in the window, the log line and the capture report, with no binary code naming them.
- [ ] A stage entered twice in one frame reports the sum of both entries for that frame.
- [ ] A vsync toggle, level install, level unload or hot reload mid-window discards the partial window; the next closed window contains only frames after it, and the install frame is not among them. (P-reset)
- [ ] After a new level installs, no surface shows a window measured in the previous level.
- [ ] A frame whose surface acquire yields no surface changes no window. (P-surface-skip)

Surfaces
- [ ] The capture report serializes stages that ran with values, omits stages that did not, and reports `not-requested` with a reason when the env var is off. The document round-trips. Warmup frames never reach its CPU stages. Only complete windows report values, and each appears exactly once. A trailing partial window appears only as a frame count. With timing on and fewer sample frames than one window, the report says not yet windowed rather than reporting zeros. (P-capture)
- [ ] An observe-live dump that requests timing returns the latest window when timing is on and reports unavailable, with a reason, when it is off. Before the first window closes, it reports not yet windowed, not zeros. Two requests between the same pair of window closes return the same window, and neither stops the log line or the debug UI from showing it. (P-live)
- [ ] A timing request served while no level is installed (frontend or loading) returns the no-world reply and never a window measured in an earlier level. (P-live-nolevel)
- [ ] A batch runspec that requests timing is rejected with an error naming the flag. Two identical batch runs without it still produce byte-identical dumps, including when `POSTRETRO_CPU_TIMING` is set.
- [ ] With timing off, no `[CpuTiming]` line appears across several windows' worth of frames. With it on, exactly one appears per closed window.
- [ ] A stage that ran in no frame of a closed window shows as absent, not zero, in the log line, the debug UI and observe-live.
- [ ] The shared timing crate depends on no engine crate (dependency-tree check) and names no stage (source check).
- [ ] Mesh pose sampling appears as a substage of render recording and no longer reads `POSTRETRO_GPU_TIMING` (source check).
- [ ] Tracy is absent from the engine's dependency tree under default features, under `observability` + `observe-live` + `capture`, and under the dist build's feature set (dependency-tree check).

### Manual
- [ ] Overhead: on a small representative map with vsync off, in a release build, the always-on frame meter's average over 10 windows with timing on has a median within 1% of the same measurement with timing off. Machine class recorded.
- [ ] Wait: with vsync on at a light scene, the wait bucket absorbs the block and work CPU sits well under the frame period. With vsync off, wait falls to near zero.
- [ ] Unattributed stays under 5% of work CPU on `campaign-test`. Larger means a stage is missing.
- [ ] Baselines for `stress-warren`, `stress-warren-crates` and `campaign-test` at their checked-in probe cameras, plus the new `stress-warren` walk-reach probe, are recorded in this brief. They cover every top-level stage, the sim substages, the walk and its counters, and the top renderer passes by CPU. Each records the adapter, build profile, machine class, cache mode and vsync state (`testing_guide.md` §Resource bounds).
- [ ] The parallelization gate is evaluated against a release-build `stress-warren` baseline, with the verdict and the camera's walk reach recorded.
- [ ] With the Tracy feature on, the same named scopes appear in a Tracy capture, with `POSTRETRO_CPU_TIMING` both set and unset.
- [ ] The debug UI CPU block updates each window, next to the GPU block.

## Path

- Precedents: GPU `FrameTiming` (window and snapshot shape: `last_window` for the UI, `completed_window` consumed once by capture); `FrameRateMeter` (allocation-free ring); `capture/report.rs` `gpu_timing_report` (availability/reason); `DumpSpec` flag fields; `debug_ui` `draw_performance_tab`.
- Shape: an RAII guard over each crate's own stage enum writes into that crate's per-frame stats, which travel upward in the existing return values. The binary folds them into windows at frame end. Rival 1: per-crate stats with bare `Instant` pairs and no shared guard. That is the repo's existing convention and needs no new crate, but it would mean annotating every site twice once Tracy is on. Rival 2: a string-keyed global recorder. Rejected because it allocates, lets labels drift between surfaces, and adds shared global state.
- First slice: top-level stages plus wait and unattributed in `main.rs`, with the log line only, measured on `campaign-test`. This settles where vsync blocks and whether unattributed is small before the substage work goes in. The frame stage map is in `research.md`.
- Carriers: walk time and counters on an `Option` in `VisibilityStats`, publishing `PortalTraversalStats` fields and timing only `flood`; sim substage times in the tick output; renderer per-pass CPU labels and acquire time beside GPU `FrameTiming`; client prediction timed in the binary around its calls. `VisibilityStats` is built inside the visibility crate plus an empty-world literal in `render_preparation.rs`; `determine_visible_cells` is called from `main.rs`, `capture/driver.rs` and `candidate_cull_probes.rs`.
- Sim substages: scopes go inside `simulate_tick_with_presentation_aim` and around the `tick_runner!` AI closure. The client-prediction branch gets its own stage labels rather than sharing host labels.
- Large files touched: `main.rs`, `sim/mod.rs`, `portal_vis.rs`, `renderer_render_frame.rs` and `debug_ui/mod.rs` are all past ~800 lines. Edits there are scope insertions only, so no split is owed. New logic lands in new modules.
- The Tracy bridge: `tracy-client` directly vs. the `profiling` facade is the executor's call; prefer the fewer transitive crates.
- Disk: check free space before starting and after each build step. Under 10 GB, `cargo clean -p` the churned PostRetro crates, or remove entries older than a day under `/private/tmp`. Never wipe the whole `target/`.

## Open questions

- Which crate hosts the helper: an existing leaf, or a new one — **delegated**: the executor decides against `crate-graph.md` and reports it in the plan of record.
