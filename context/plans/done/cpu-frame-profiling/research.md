# CPU Frame Profiling — Research

Derivation behind `index.md`. Read at 987f8c5bb. Symbols, not line numbers; re-verify before relying on any row.

## In-level frame stage map

All stages run in order in the `WindowEvent::RedrawRequested` arm of `App::window_event` (`crates/postretro/src/main.rs`).

| Stage | Entry symbol(s) |
|---|---|
| Frame start / tick budget | `FrameTiming::begin_frame` (`crates/sim/src/sim/frame_timing.rs`) → `FrameTickResult { ticks, alpha, frame_dt }`; `MAX_ACCUMULATOR` 250 ms caps ticks per frame |
| Housekeeping | `advance_seat_hold_clock`, `drain_script_reload_requests`, `drive_boot_state_for_redraw`, scheduler `begin_frame` |
| Input + UI dispatch | gamepad `update`/`tick_rumble`, `input_mode_tracker.update`, `ui_dispatch.take_ready`, `camera.rotate` |
| Client snapshot apply | `frame_order::run_snapshot_apply_stage` |
| Fixed-step loop | host: `host_resolve_remote_commands`, `sim::simulate_tick_with_presentation_aim`, `evaluate_slot_accumulators`, `host_advance_fixed_sim_tick`; client: `simulate_client_wieldable_tick`, `client_predict_*_tick` |
| Post-loop presentation | `net_sample_remote_interpolation`, `update_client_presentation_pose_inputs`, `run_client_fire_path_post_loop` |
| Audio | `sound_events::*`, `audio.set_listener_attached`, `audio.play` |
| Script drain | `drain_named_events_with_sequences`, `dispatch_system_commands`, `frame_order::run_crossing_stage` |
| End-of-frame removal | `impact_effects::run_end_of_frame_removal_pass` |
| Host send | `net_serialize_and_send`, `host_present_client_pawns` |
| Eye | `reconcile_ui_focus`, `frame_eye::assemble_frame_eye` |
| Visibility | `rebuild_blocked_portals`, `VisibleRenderPreparation::for_level` → `determine_visible_cells` (portal walk) + fog/light reach |
| Render prep | `scripting_systems::particle_sim::tick` (render rate), light/fog uploads, `prepare_sh_streaming_drain`, mesh draws, `build_ui_read_snapshot` |
| Render record + submit | `render_frame_indirect` → surface acquire (`get_current_texture` in `renderer_frame.rs`) → pass recording → `queue.submit` |
| Present | `renderer.present`, then title update and `frame_rate_meter.record` |

Sim substages (kinematic movers, movement, AI via `postretro_ai::tick_runner!`, triggers/touch, weapons, deferred impact effects) are interleaved inside `simulate_tick_with_presentation_aim`. Script reactions fire both inside ticks and in the post-loop drain. Frontend frames take `render_frontend_frame` and return before the in-level path.

**Unverified:** where vsync blocks — surface acquire is the likely point under Fifo on Metal, but present is possible. The wait bucket's placement depends on it.

## Existing CPU instruments

| Instrument | Measures | Gate | Output |
|---|---|---|---|
| `FrameRateMeter` (sim `frame_timing.rs`) | Handler entry → after present | always | 120-sample min/avg/max, window title every 250 ms |
| `PoseSampleStats` (`mesh_pass.rs`) | `sample_instance` per skinned resample | `POSTRETRO_GPU_TIMING` | 2 s sum-and-reset log line |
| `compose_planning_cpu_micros` (SH streaming `frame.rs`) | compose planning | always | gauge; debug UI, 5 s log |
| `InstallCpuCounters` (SH `diagnostics.rs`) | install drain | always | cumulative total/max/last |
| `PoolGrowthCounters` (SH `gpu/growth.rs`) | pool growth | always | cumulative |
| `LatencyHistogram` (`sh_async_workers/stats.rs`) | worker read/decode | always | 20-bucket histogram |
| `StartupTimings` (`startup/mod.rs`) | boot/load stage deltas | always | `[Startup]` lines |
| Capture driver | `capture_measurement_frame` incl. device-poll wait | capture scene `measurement` block | JSON median/p95/raw |
| `netdiag` | event counts, not time | `RUST_LOG` debug | 1 s lines |

No shared helper, window constant, or gate. Units are mostly µs `u64` saturating. Test-only timing asserts in `particle_sim.rs`, `particle_render.rs`, `registry.rs` are not instruments.

## Surfaces already shaped for this

- GPU `FrameTiming` (`crates/renderer/src/render/frame_timing.rs`): 120-sample windows, `(label, avg_ms, skip_count)`, `last_window` for the debug UI, `completed_window` consumed once by capture.
- Capture report (`capture/report.rs`, `postretro.capture.measurement.v1`): `gpu_timing` carries `availability` + `reason` (`not-requested` / `env-disabled`) and omits `windows` when unavailable. `cpu_stages` should follow that exact pattern.
- Capture resolves visibility once, during preparation (`PreparedCapture::prepare`, which runs the walk), and re-submits a prepared render per sample; no tick or walk runs per sample. `CAPTURE_PORTAL_WALK` only toggles the walk's trace capture. Only render-record stages produce numbers there.
- Observability `DumpSpec` (`observability/runspec.rs`) is a flag/filter struct (`events`, `cell_visibility`). A timing flag fits its shape. `networking.md` live-channel section: one vocabulary for batch dump and observe-live.
- Debug UI `draw_performance_tab` (`debug_ui/mod.rs`) shows the GPU block only.

## Commitments touched

- Diagnostic gating is unwritten but consistent: surfaces (UI, sockets, modules) behind cargo features; runtime instrumentation behind `POSTRETRO_*` env vars in any build.
- `observability`, `observe-live`, `capture` features "add no dependencies" (`crates/postretro/Cargo.toml`; `plans/done/agentic-observability`). `dev-tools` already adds one (egui). Tracy must stay out of the three dependency-free features.
- Dump byte-identity: two identical batch runs produce byte-identical output (`plans/done/agentic-observability`, `plans/done/E20--live-socket-channel`). Live-only enrichments such as the sim-tick clock stay out of batch rather than overloading it.
- `plans/done/perf-visible-cell-candidate-cull`: portal traversal "has negligible measured cost" on its map.
- `context/plans/roadmap.md` BIS: the egui-retirement checklist must replicate the egui overlay's diagnostics before removal.
- `development_guide.md` pinned-crates table: dependency rationale lives in the owning `Cargo.toml`. §1.4: build time is a budget; profiling before optimizing is endorsed.
- `build_pipeline.md` release stage 1: payload builds use no non-default features.
- `rendering_pipeline.md` §12: measurement keeps disabled / unsupported / not-yet-windowed distinct from a zero-cost pass.
- `perf-per-region-bvh` (done) established "Pre-work — gating measurement" baselines recorded in the plan with adapter and build.

## Consumers

- Portal-walk parallelization gate (this brief).
- `ai_pathfinding_mt_readiness.md` names a scheduling blocker but no measured cost; AI tick CPU is unmeasured today.
- `movement-tick-component-clone-alloc` gates on allocations, not time — no dependency.
- `shadow-cone-cull-parallel-dispatch`, `bvh-leaf-clustering` gate on GPU time — no dependency.
- `E20--scripted-run-capture` is the path by which capture reports would carry tick and walk stages; it does not plan for it today.

## Portal-walk parallelization constraints

Carried from the prior draft, for a future parallelization plan. `flood` in `crates/visibility/src/portal_vis.rs` is a recursive per-chain DFS mirroring id Tech 4's `FloodViewThroughArea_r`: a cell is re-entered through different chains under different narrowed frusta; the chain path is the cycle guard.

| State | Under parallel expansion | A plan must |
|---|---|---|
| Visible-cell array | Safe — monotone union | Per-thread bitsets OR'd at join |
| Traversal counters | Safe — sums | Reduce at join |
| Chain path (cycle guard) | Per-chain push/pop | Clone into each fork; chains are shallow |
| Polygon clip scratch | Per-chain reuse | Thread-local pair per worker |
| Trace capture string | **Unsafe** — ordering is the content | Per-subtree buffers concatenated deterministically, or no parallel expansion while armed |
| Step-limit fuse | **Unsafe** — shared budget; cut chains become scheduling-dependent | Deterministic per-subtree split, or explicitly accept nondeterminism on the degenerate fallback path |

Fork granularity is one polygon clip plus one frustum narrow — hundreds of nanoseconds, below task-spawn cost — so forking must be depth-bounded. Rayon is a `prl-build`-only dependency; a work-stealing pool in the frame path is an architectural change to be argued as one.

## Pin table

Orderings the Acceptance rows cite.

| ID | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P-close | 120th counted in-level frame | Window closes at frame end, after present | Window one holds exactly frames 1–120; frame 121 starts window two with nothing carried over. |
| P-count | Frontend or early-returned frames interleaved | They return before any stage records | They do not advance the frame count or change any window. |
| P-early-fallback | Portal fallback, then an early return on a fatal error | Visibility runs before the fatal returns | Neither the walk window nor the fallback count changes. |
| P-zero-tick | Accumulator yields zero ticks | The fixed-step loop body does not run | Tick count zero, sim substages absent that frame, frame still counts. Sim substage averages cover only frames with ticks. |
| P-many-tick | Accumulator at the 250 ms cap | Ticks run back to back within one frame | Substages sum over every tick; tick count is the executed count. |
| P-zero-walk | Portal path, no portal considered | Walk runs and expands nothing | Walk time recorded, counters zero, walk present. |
| P-steplimit | Walk trips the step limit | Full walk runs, then the frame falls back to the AABB cull (`VisibilityPath::PortalStepLimitFallback`) | Walk frame: walk time and counters enter the walk window; step-limit count rises; fallback count unchanged. |
| P-surface-skip | Acquire returns Timeout, Occluded, Outdated or Lost | Acquire precedes any pass; present is skipped; the handler continues | Discarded like an early return: no window changes, and the timed-out acquire never lands in wait. |
| P-wait | Vsync block in acquire or present | The block sits inside the render or present call | Counted once, in wait: the renderer returns acquire time and the binary moves it and present's block out of their enclosing stages. Unattributed = total − top-level stages − wait, exactly, no clamp. |
| P-acquire | Acquire with a surface reconfigure pending (for example after a Suboptimal acquire) | The reconfigure runs inside the acquire call, before the surface texture is requested | The acquire timer wraps only the texture request; reconfigure work stays in render recording, never in wait. |
| P-reset | Vsync toggle, level install/unload, hot reload mid-window | Toggle arrives between redraws; install runs inside a redraw that continues in-level | The partial window is discarded; the install frame does not count; a new level clears the previous level's last window from every surface. |
| P-reload | Hot reload commits a staged manifest mid-frame | The commit (`poll_staged_manifest_results` returning committed) runs after present and before the frame-end fold; queueing a reload build is not the event, since a build can be discarded as stale or fail | The reset takes effect at frame end: the commit frame is discarded and the next window starts with the following frame. |
| P-live | Observe-live request while a window closes | Served at the head of frame N+1; window closed at the end of frame N | Reply is window N. Reads do not consume it; the log and UI still show it. |
| P-live-nolevel | Request during frontend or loading | Served before the boot-state returns | No-world reply, never an earlier level's window. |
| P-capture | Capture with warmup W and S sample frames | Warmup, reset, then samples | No warmup frame reaches the report; each complete window appears once; a trailing partial window appears as a count; S < 120 reports not yet windowed. |
| P-cpugpu | CPU and GPU windows in one session | The GPU window counts completed readbacks, which lag and skip | The windows close on different frames and cover different frame sets; no surface claims they align, and a baseline records each on its own. |
| P-alloc | Allocation proof for the timer | The probe counts per thread, only in test binaries that install the counting allocator (engine binary, sim); the leaf crate cannot install one without new `unsafe` | The proof lives in the engine binary's tests, drives the timer's operations directly, and counts a zero only when a control allocation was seen. |
| P-gate | On/off neutrality test | The env var is read once at startup; changing the environment in a test is `unsafe` under edition 2024 | The on/off state reaches each timed crate as a value; no test touches the environment. |
