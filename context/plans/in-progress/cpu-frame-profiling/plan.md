# CPU Frame Profiling — plan of record

mode: resumable
status: proposed
read at: 683e363ba

## Corrections

- Capture schema is `postretro.capture.measurement.v2` (`capture/report.rs` `MEASUREMENT_SCHEMA`), not v1; the report types derive `Serialize` only. → Add `cpu_stages`, bump to v3, and add `Deserialize` to the report types so the round-trip AC has something to round-trip.
- No automated batch-dump byte-identity test exists; `agentic-observability` proved it by manual smoke. The unit tests only cover `to_deterministic_json`. → Add an in-process test that runs the headless driver (`observability/driver.rs` `run_headless_inner`) twice and compares bytes. Per P-gate, "when `POSTRETRO_CPU_TIMING` is set" is proven by handing the batch path a timing-on gate value, never by touching the environment. Same meaning, restated as a clarification.
- `DumpSpec` has no flag validation hook; `parse_runspec` validates only commands and the tick cap. → Add a `RunSpecError` variant raised from `parse_runspec` when `dump.cpu_timing` is true; its message names `dump.cpu_timing`.
- `poll_staged_manifest_results` (`startup/staged_manifest_lifecycle.rs`) returns `()`; a commit is visible only as a local `committed`. → Return whether any result committed, so the frame-end fold can apply the P-reload reset.
- `FrameRateMeter` is cleared only on the vsync toggle (`main.rs` `ToggleVsync` → `frame_rate_meter.clear()`). The CPU window resets hook the same site plus install, unload and reload commit, which the binary sees in `drive_boot_state_for_redraw` / `drain_level_requests` / `unload_level` and the corrected poll above.
- `PortalTraversalStats` is `pub(crate)`. Its counters are published only as `considered`/`accepted` on `PortalStepLimitFallback`, and as a trace string while capture is armed. → Carry a public walk record (time + all counters + step-limit flag) as `Option` on `VisibilityStats`, per the brief's Carriers.
- A step-limit trip aborts the walk at 20 000 considerations (`MAX_PORTAL_WALK_STEPS`), then runs two AABB passes. "Full walk" in P-steplimit means the walk up to the trip. The walk timer wraps `flood` only, so the fallback AABB passes land in visibility's parent time, not in walk.
- Acquire, including `surface_reconfigure_pending` → `reconfigure_surface()`, runs inside `acquire_present_handle` (`renderer_frame.rs`) before recording. A failed acquire returns `Ok(None)` without recording. This matches P-acquire and P-surface-skip. The acquire timer wraps only `get_current_texture()`.
- `render_debug_ui` makes a second `queue.submit` between the frame submit and present (dev-tools). → It becomes its own render-recording substage, never wait.
- Script reactions do not run inside `simulate_tick_with_presentation_aim`. Triggers (with in-tick script dispatch) do; post-tick named-event drains, residuals, landings and `dispatch_system_commands` run app-side in `main.rs`. → Sim substage "triggers/scripts" covers the in-sim trigger/touch dispatch. The app-side drains form the binary's top-level **script drain** stage, as the research stage map already says.
- Walk-reach probe placement: a windowed run has no way to start at an arbitrary pose. The camera follows the pawn spawned at the map's single `player_spawn`, and the existing probe table (`candidate_cull_probes.rs`) is `#[cfg(test)]` data at the spawns. → Add a launch argument `--start-pose x,y,z,yaw,pitch` that overrides the local player's first spawn pose, so the checked-in walk-reach probe is reproducible. It is dev-facing, but the brief puts diagnostics in every build, so it is not feature-gated. Flagged for the owner's skim: this is new surface the brief implies but does not name.
- `stress-warren.prl` and `stress-warren-crates.prl` are not checked in (~1 h cold bake each). The manual baselines include a compile step; disk checks apply.

## Delegated answers

- Which crate hosts the helper — **new leaf crate `postretro-stage-timing`** (`crates/stage-timing`). `foundation` would add a visibility → foundation edge and put timing churn at an 11-dependent chokepoint, and `level-loader` is a data crate. A new leaf has zero workspace deps, so the "depends on no engine crate" check is trivial to keep true.
- Tracy bridge — decided in task 7, preferring fewer transitive crates. `profiling` already sits in the lock tree through wgpu, which favors it. Its Tracy backend may require literal span names, though, and our guard carries `&'static str` labels chosen at runtime. If so, use `tracy-client` directly with allocated spans. Allocation under Tracy is outside every allocation AC, which are all "Tracy off".

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 Neutrality on small portal fixture, on/off in one process | visibility test on `portal_chain_world`: identical `visible_cells`, `fog_reachable`, `path` with gate on vs off; on-demand `#[ignore]` repeat in `candidate_cull_probes` | achievable as stated |
| A2 Timing off: no timer allocation, no window accumulates | binary test (counting allocator in `main.rs` tests) driving every scope, frame end, 2+ windows with gate off; control allocation seen | achievable as stated |
| A3 Timing on, Tracy off: steady-state zero alloc incl. window close | same harness, gate on, after the first window | achievable as stated |
| A4 Proofs run where allocations are counted, with control, timer-only | binary tests only (P-alloc); each asserts a deliberate control alloc is counted | achievable as stated |
| A5 Zero-tick frame: tick count 0, no sim time, frame counts | fold test (P-zero-tick) | achievable as stated |
| A6 Multi-tick frame sums substages | fold test (P-many-tick) + sim test that each tick returns its stage frame | achievable as stated |
| A7 Substage ≤ parent per frame; unattributed ≥ 0 | fold/property test over recorded frames; debug check in fold | achievable as stated |
| A8 Stage averages over frames it ran; 60/60 walk/fallback | fold test | achievable as stated |
| A9 Step-limit frame enters walk window, raises count, not fallback | visibility test (`portal_traverse_reports_step_limit…` fixture) + fold test (P-steplimit) | achievable as stated |
| A10 Unattributed exact; acquire/present block in wait only | frame accountant test with injected durations (P-wait) | achievable as stated |
| A11 Frame 120 closes; 121 opens; only counted frames advance | accountant test (P-close, P-count) | achievable as stated |
| A12 Early return changes no window; fallback-then-early-return changes nothing | accountant test (P-early-fallback): frame staged, then discarded | achievable as stated |
| A13 Portal frame adds walk time + all counters | visibility test + fold test | achievable as stated |
| A14 Zero-portal walk present with zero counters | visibility test (P-zero-walk) | achievable as stated |
| A15 Fallback frame: walk unchanged, fallback count +1; fallback-only window → walk absent | fold test | achievable as stated |
| A16 Frontend frame changes no window | accountant test (frame never begun/committed) | achievable as stated |
| A17 Step-limit trips reported as count | fold/log-line test | achievable as stated |
| A18 Two timers isolated | leaf test: two instances interleaved | achievable as stated |
| A19 Test-only stage set appears in window, log, capture report | binary test with a test enum nested under a top-level parent | achievable as stated |
| A20 Stage entered twice sums | leaf test | achievable as stated |
| A21 Reset on vsync/install/unload/reload; install frame excluded | accountant test (P-reset, P-reload) | achievable as stated |
| A22 New level: no surface shows prior level's window | accountant test: install clears `last_window` read by log/UI/live | achievable as stated |
| A23 No-surface frame changes no window | accountant test (P-surface-skip) | achievable as stated |
| A24 Capture `cpu_stages`: present/omitted/not-requested, round-trip, warmup excluded, complete windows once, partial count, not-yet-windowed | `capture/report.rs` tests mirroring the `gpu_timing` suite + `Deserialize` round-trip | achievable as stated |
| A25 Observe-live timing: window / unavailable / not-yet-windowed / idempotent reads | `observe_live/ingress.rs` tests | achievable as stated |
| A26 Live timing with no level → no-world reply | ingress test beside `service_returns_no_world_…` | achievable as stated |
| A27 Batch rejects flag by name; two batch runs byte-identical incl. gate on | `parse_runspec` test + new in-process headless double-run test (see Corrections) | achievable (clarified) |
| A28 No `[CpuTiming]` line when off; exactly one per window when on | `test-log-capture` test on the log surface | achievable as stated |
| A29 Absent-not-zero in log line, debug UI, observe-live | log-line test, live test, debug-UI row formatter unit test | achievable as stated |
| A30 Leaf crate depends on no engine crate, names no stage | dependency test (cargo metadata) + source check that no crate's stage label appears in leaf source | achievable as stated |
| A31 Mesh pose sampling under render recording; no `POSTRETRO_GPU_TIMING` read | renderer stage-tree test + source check on `mesh_pass.rs` | achievable as stated |
| A32 Tracy absent under default / observability+observe-live+capture / dist features | xtask test running `cargo tree -e normal` per feature set | achievable as stated |
| M1 Overhead within 1% median over 10 windows | owner, in-engine, release, vsync off | manual |
| M2 Wait absorbs vsync block; near zero with vsync off | owner/executor, in-engine | manual |
| M3 Unattributed < 5% work CPU on campaign-test | executor first measurement (task 2), owner confirms | manual |
| M4 Baselines for three maps + walk-reach probe recorded | executor/owner, release, in-engine | manual |
| M5 Parallelization gate verdict on release stress-warren | executor/owner | manual |
| M6 Tracy capture shows scopes, gate set and unset | owner, Tracy GUI | manual |
| M7 Debug UI CPU block updates each window beside GPU | owner, `dev-tools` build | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Leaf crate `postretro-stage-timing`: `Stage` label trait (index, label, parent), per-frame fixed storage with summed re-entry, gate-aware RAII scope, label-keyed window fold (120 frames, avg/max/ran-count, absent), frame counters. Leaf tests (A18, A20). Regenerate `crate-graph.md`. | integrating executor | — | |
| 2 | **Riskiest-premise slice:** env gate read once in the binary; top-level frame stage enum; scopes over the research stage map in the redraw arm; renderer returns acquire time (texture request only); present timed in binary; wait + unattributed; frame accountant (exclusions, resets incl. reload-commit return, install frame); `[CpuTiming]` log line. Accountant tests (A5, A10–A12, A16, A21–A23, A28); allocation proofs (A2–A4). Measure `campaign-test`: where vsync blocks and unattributed size. | integrating executor | 1 | |
| 3 | Visibility: gate parameter into `determine_visible_cells`; time `flood` only; publish walk record (time, all counters, step-limit) as `Option` on `VisibilityStats`; binary folds walk/fallback/step-limit. Tests A1, A9, A13–A15, A17. | worker (visibility crate) + integrator folds | 1, 2 | |
| 4 | Sim substages: sim stage enum; scopes in `simulate_tick_with_presentation_aim` (movers, movement, triggers/scripts, AI around `run_ai`, weapons); stage frame on `TickEvents`; binary sums across ticks + tick count; client-prediction labels in binary. Tests A5–A8. | worker (sim crate) + integrator | 1, 2 | |
| 5 | Renderer: render stage enum; one CPU scope per pass in `record_scene_passes` + `submit_windowed_frame` + `render_debug_ui`; mesh pose sampling as a substage, `PoseSampleStats` retired; stage frame returned beside GPU timing, capture path included. Tests A31. | worker (renderer crate) + integrator | 1, 2 | |
| 6 | Surfaces: debug UI CPU block (A29 formatter); observe-live `cpu_timing` live-only section + batch rejection + double-run byte test (A25–A27); capture `cpu_stages`, v3, round-trip (A24); test-only stage set end to end (A19). | integrating executor | 3, 4, 5 | |
| 7 | Tracy feature bridge in the leaf crate; feature wiring through binary; dependency-tree and source checks (A30, A32). | integrating executor | 1, 5 | |
| 8 | `--start-pose` launch arg; checked-in walk-reach probe for `stress-warren` (added to the probe table + README); release baselines for three maps + probe; gate verdict recorded in brief (M3–M5). Manual rows M1, M2, M6, M7 handed to owner. | integrating executor, owner | 2–7 | |

Tasks 3, 4 and 5 touch disjoint crates, so they can run as concurrent workers once 2 fixes the fold contract. The integrator owns every `main.rs` edit and all Cargo runs.

## Follow-ups (not in scope)

- `postretro-ai` (`graph_eval.rs`, `brain_scope.rs`, `candidate_scope.rs`, `targeting.rs`) and `postretro-physics` (`movement/mod.rs`) arm `AllocSnapshot` without installing a `#[global_allocator]`, so their zero-allocation assertions likely pass vacuously. Unverified by run.
- `main.rs` diagnostics build `walk_reach_col` with `format!` every portal frame, a per-frame allocation outside the timer.
