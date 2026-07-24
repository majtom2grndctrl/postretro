# Portal Walk CPU Instrumentation

## Goal

Nothing today measures the portal traversal in isolation — the engine reports whole-frame CPU time and per-pass GPU time, and the portal walk falls between them. Add behavior-neutral CPU instrumentation for the walk, surface the traversal counters it already computes, and pin the numeric threshold that would justify parallelizing it. Without this, any decision to parallelize the walk is a guess.

## Scope

### In scope

- Per-frame CPU timing for the portal traversal, averaged over a fixed window and logged.
- Surface the existing internal traversal counters (considered, accepted, per-reason rejections, step-limit trip) on the public visibility stats.
- Record baseline measurements for the stress maps in this plan.
- Pin the promotion gate for a future parallelization plan, and record the constraints that plan must satisfy.

### Out of scope

- Parallelizing the walk. This plan measures and gates; it does not change traversal.
- Adding rayon or any thread pool to the runtime. Rayon is currently a compiler-only dependency and stays that way under this plan.
- Changing traversal semantics, the step budget, the chain-depth limit, or the fallback selection.
- Changing the portal-walk trace capture format.
- Optimizing the walk sequentially — early-outs, memoization, frustum caching. A separate question, not gated by this measurement.

## Acceptance criteria

- [ ] Portal-walk CPU time is logged as an averaged value over a fixed frame window, on the same cadence as the existing GPU pass timing.
- [ ] The averaged line reports considered, accepted, per-reason rejection counts, and whether the step limit tripped during the window.
- [ ] Instrumentation is behavior-neutral: for every checked-in camera probe, the visible-cell set, the fog-reachable set, and the chosen visibility path are identical before and after the change.
- [ ] Frame CPU time on a small representative map does not regress by more than 1% over a 120-frame window.
- [ ] Counters are reported only on the portal path; fallback paths report their absence rather than zeros.
- [ ] Baseline numbers for `stress-warren`, `stress-warren-crates`, and `campaign-test` at their checked-in probe cameras are recorded in this plan.
- [ ] The parallelization gate is stated as a numeric threshold that a later reader can evaluate against a measurement without re-deriving it.

## Tasks

### Task 1: Capture and expose portal-walk timing and counters

`portal_traverse_inner` in `crates/visibility/src/portal_vis.rs` already accumulates a `PortalTraversalStats` — considered, accepted, `rejected_solid`, `rejected_clipped`, `rejected_narrow`, `rejected_invalid`, `rejected_path_cycle`, `rejected_depth_limit`, and `step_limit_hit` — but the struct is `pub(crate)` and the counts are consumed only by the trace-capture summary line. Add a public stats struct mirroring those fields plus the measured wall-clock duration of the `flood` call, and hang it off `VisibilityStats` as an `Option` populated only on the portal path. `VisibilityStats` is constructed at three call sites — `crates/postretro/src/main.rs`, `crates/postretro/src/capture/driver.rs`, and `crates/postretro/src/candidate_cull_probes.rs` — plus the no-level literal inside the render loop; all four need the new field. Time only the traversal itself, not the frustum extraction or the cell-set conversion that surround it, so the number attributes cleanly.

### Task 2: Averaged reporting

Accumulate the per-frame portal-walk duration and counters over a 120-frame window and log one averaged line, mirroring the reporting shape the renderer already uses for GPU passes in `crates/renderer/src/render/frame_timing.rs` — fixed window, averaged value, and a retained snapshot the debug UI can display under `dev-tools`. Gate the logging on an environment variable rather than a cargo feature, matching `POSTRETRO_GPU_TIMING`; the timing capture itself compiles in unconditionally, since two clock reads per frame are noise against the call being measured. Only the debug-UI snapshot stays `dev-tools`-gated, because that UI already is. Report the counters as window averages except `step_limit_hit`, which reports as a count of frames in the window that tripped it, since a single trip is the interesting signal. On frames that took a fallback path, contribute nothing to the window rather than contributing zeros, and report the fallback frame count separately so a window is never silently diluted.

### Task 3: Record baselines and pin the gate

Run the three probe maps at their checked-in probe cameras, capture the averaged portal-walk time and counters, and write the numbers into this plan alongside the adapter and build profile — following the "Pre-work — gating measurement" convention that `context/plans/done/perf-per-region-bvh/index.md` established. Then state the promotion gate for a future parallelization plan and the constraints table below as that plan's starting requirements.

## Sequencing

**Phase 1 (sequential):** Task 1 — Task 2 consumes the stats struct it adds.
**Phase 2 (sequential):** Task 2 — Task 3 reads the averaged output it produces.
**Phase 3 (sequential):** Task 3 — records results, cannot start before there is output to record.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Instrumentation is behavior-neutral | Task 1 (timing and counters are read-only side outputs) | Task 1 changes a public struct with four construction sites; a missed site is a compile error, a wrong default is a silent stats bug | AC 3 |
| Counters describe one traversal, not a blend of paths | Task 1 (`Option`, populated on the portal path only), Task 2 (fallback frames excluded from the window) | Task 2's averaging must not treat a fallback frame as a zero-cost portal frame | AC 5 |
| Hot path allocates nothing when diagnostics are off | Pre-existing: trace sites check `Option` before every write | Task 1 must not allocate per frame to carry the counters | AC 4 |

## Parallelization gate

Promote a parallelization plan only when both hold, measured on `stress-warren` at a high-reach camera, in a release build:

1. Averaged portal-walk CPU time exceeds **0.5 ms** over a 120-frame window.
2. That time is at least **5%** of averaged frame CPU time over the same window.

The second condition exists so that a CPU regression elsewhere in the frame cannot make the walk look cheap, and so a walk that is slow in absolute terms but irrelevant to frame pacing does not trigger work. Below both thresholds the sequential walk is paying its way and parallelizing it is premature.

## Parallelization constraints

Recorded now so a future plan starts from the analysis rather than repeating it. `flood` in `crates/visibility/src/portal_vis.rs` is a recursive per-chain DFS mirroring id Tech 4's `FloodViewThroughArea_r`: the same cell is re-entered through different portal chains under different narrowed frusta, and the chain path is the cycle guard. Each portal expansion is an independent subtree, which is the shape that fork-joins well — but the shared state is not uniformly safe.

| State | Under parallel expansion | What a plan must do |
|---|---|---|
| Visible-cell array | Safe — a monotone union, order-independent | Per-thread bitsets OR'd at join |
| Traversal counters | Safe — sums, order-independent | Reduce at join |
| Chain path (cycle guard) | Per-chain, currently pushed and popped along the stack | Clone into each forked task; chains are shallow, so the copy is cheap |
| Polygon clip scratch buffers | Per-chain reuse, currently threaded through the recursion | Thread-local, one pair per worker |
| Trace capture string | **Unsafe** — event ordering is the diagnostic's content | Per-subtree buffers concatenated in deterministic order, or disable parallel expansion while capture is armed |
| Step-limit fuse | **Unsafe** — a shared budget checked before each expansion; which chains get cut becomes scheduling-dependent, so the visible set and the step-limit fallback both turn nondeterministic | Deterministic per-subtree budget split, or accept nondeterminism confined to the already-degenerate fallback path and say so explicitly |

Two further constraints on any such plan. Fork granularity is one polygon clip plus one frustum narrow — hundreds of nanoseconds, well below task-spawn cost, so forking must be depth-bounded: parallel for the first levels, sequential below. And rayon is a workspace dependency used only by `prl-build`; introducing a work-stealing pool into the frame path is an architectural change to the runtime, not a local optimization, and needs to be argued as such.

## Open questions

None.

Gating was the one real question, and it resolves against the existing convention: the timing is **env-gated, not `dev-tools`-gated**, matching how the renderer's GPU pass timing already works. The cost of compiling it in is two clock reads per frame around a call that is being measured precisely because it may cost hundreds of microseconds; that is noise. The "tiny binary" goal is about what ships in the payload, not about two timestamps, and a diagnostic surface that behaves differently depending on build features is worse for a modder-facing engine than one that is uniformly available behind an environment variable. Task 2 states this directly; the debug-UI snapshot stays `dev-tools`-gated, since the UI itself already is.
