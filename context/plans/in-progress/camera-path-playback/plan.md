# camera-path-playback — plan of record

mode: resumable
status: approved
read at: 2f1a5d7f4

Source re-read at 2f1a5d7f4 against every symbol Decisions and Path cite. The walk harness (`session/lightmap_residency/`), `camera.rs` and `frame_eye.rs` are unchanged since the brief's read commit ecd9489e0, so a trip record written now is a read-commit record. No Decision premise is false. The Path needs the corrections below.

## Corrections
- **Generator home.** Brief: one function "in a CPU crate the tool reaches". Every piece to share is test-only today (`walk_measurement` is `#[cfg(test)]`; `world_poses` inherits it). `postretro-tool` reaches `level-loader` only through `level-compiler`. Planning around it with a new Layer-2 crate `postretro-camera-route` (deps: `level-loader` with `load-prl`, `glam`, `blake3`, `level-format`, `visibility` for `PORTAL_WALK_EPOCH`). It takes `SplitMix64`, `WalkKind`, `camera_adjacency`, `walk_path`, `Walk::build`, `eye_point` (32-try retry loop kept exact), `FRAME_SECONDS`, the `WALK_SEED ^ 0xE7E5` eye stream, `bake_identity`/`bake_token`, and the pose-file writer and parser. The harness `run`, `bake_gap::materialise` and the tool call it. `render_camera` and `RenderCamera` stay in the binary. The dry run's `camera_walks.rs` (prl-build bin, test-only) is left alone. It walks `PortalGraphInput` and has no eye stream, so calling the shared crate there buys nothing.
- **Pose format.** The bake-gap record (radians, `{}` shortest-exact) is the precedent, as the brief says. `postretro-world-poses 1` stores degrees and is untouched. A walk-written pose carries step and cell, and the header carries the projection, so `pose_fingerprint` reproduces.
- **FOV clamp.** `RenderCamera::new` clamps to 60–130° only when a view-feel FOV offset is non-zero; `HFOV` itself is unclamped and there is no base-FOV parameter. Planning around it by adding a base horizontal FOV to `RenderCamera::new` and `FrameEyeInputs` (the `HFOV` const stays the default) and validating the file's FOV at launch against `MIN_FOV_DEG..=MAX_FOV_DEG` (`camera.rs`).
- **Presented-pose seam.** `place_camera_at_presented_pose` writes `App::presented_pose()` into `app.camera`, which is also the pawn's aim. Overriding `presented_pose()` would move the pawn. Planning around it by overriding at the two readers instead: Settling's `held_view` call (`settling.rs` `run_settling_frame`) and `render_aim_pose` (`main.rs`). `presented_pose()` and the pawn's install stay untouched.
- **View-feel bypass.** No explicit bypass exists; the menu relies on the backdrop having no view-feel pawn. Playback passes `driver: None` to `assemble_frame_eye` (frame_loop `view_feel_driver` call), which the source documents as bit-identical to the no-view-feel matrix. `life_state_eye` runs before `assemble_frame_eye`, not after it. That is moot, because playback refuses `--connect`.
- **Input suspension.** Settling's `suspend_gameplay()` covers Settling frames only. Running frames read the snapshot. Planning around it by OR-ing playback into the `ui_captures_gameplay` gate (`gameplay_capture_gate_for_frame`), which gives the sim neutral input and keeps diagnostic chords (event-driven) and the dev-tools panel live. Esc / gamepad Start would open the pause menu over measured frames, so playback also suppresses that toggle. Window close and the quit chord still exit.
- **Settle outcome.** Settled or timed out is known only inside `reveal_level`; `SettleSummary.timed_out` is private. A playback hook reads it there.
- **Missing-feature warning.** The only precedent (`--capture`, `--headless`) prints to stderr and exits 1. Acceptance asks for warn-once and continue, so this is a new pattern, built as Acceptance states.
- **CPU rows.** `FrameRecord` is `Clone` (a fixed 128-sample array), and `last_record` is test-only. A row copies a fixed selection: total, work, wait (acquire, present), the top-level stages, `fixed_step` and `ticks`, `lightmap_residency`, `lightmap_drain` and `render_sh_drain`. It reads them through a new non-test accessor on the composed record. It does not copy the whole record.
- **GPU time per row.** There is no whole-frame or drain timestamp pair today. Per-pass queries are prefilled and resolved inside the frame encoder, and both drain encoders submit before it. Planning around it with a playback-only query ring (its own query set, one slot group per frame, enough frames in flight that no row is skipped in practice), written inside the lightmap drain encoder, the SH upload-copies encoder and the frame encoder. It is gated by the same `gpu_timing_features_supported` and `POSTRETRO_GPU_TIMING=1` gate per-pass timing uses. Clarification: "an adapter with timestamp queries" means GPU timing is on there. A row whose readback never returned reads absent, and the record counts such rows.
- **SH drawn-not-resident.** Lightmaps expose `last_frame_drawn_not_resident`. SH has only cumulative counters (`ShResidencyCounters`). The per-frame SH count comes from a per-frame delta, or from a new production gauge mirroring the lightmap's if the counter's unit is episodes, not cluster-frames. Task 6 confirms which.
- **Cold and growth flags.** No new pool code is needed. The three drains use separate `StagingPool` instances (lightmap `stream.rs`, SH `sh_streaming/gpu.rs`, per-frame `uploads/mod.rs`). Cold is a delta of `StagingPool::counts().created` across the two drain pools only. Growth is `Executed.grew[kind]` (`stream/execute.rs`).
- **Revision and profile.** Capture's revision shells out to `git` at runtime, which a bundle cannot do. The header records it when available and `null` otherwise. Build profile and features come from the build (a build-script-derived profile name and a `cfg!` feature list), not from runtime state.
- **Hidden window and stall.** No `WindowEvent::Occluded` handler exists. A hidden Windows window gets no `RedrawRequested` at all. Planning around it with an `Occluded` handler and surface-`Occluded` frames for gap marks. The stall timeout is checked in `about_to_wait` under `ControlFlow::WaitUntil(deadline)` while armed, not in `redraw`, which a stalled window never reaches.
- **Join from frontend.** A join never passes through `level_requests`. It is refused once a gameplay level is installed, and the host's level arrives as `LevelRequest::HostLevel`. The abort hook covers `drain_level_requests` and `join_from_frontend_with`.
- **CLI.** Flags are hand-rolled scanners in `startup/session.rs`. Each value-taking flag joins `resolve_map_path`'s skip list.
- **Hot-reload landing.** The commit lands in `poll_staged_manifest_results`, not at the watcher drain. Mid-route marks hook there, at `handle_diagnostic_action(ToggleVsync)`, at `commit_render_extents` and at `finish_window_change`.
- **SDK gating.** `xtask dist.rs` `build_authoring_engine` already builds with `dev-tools`. Playback costs one `Option` check per frame when inactive, so it rides `dev-tools`, as the Decision allows, and dist needs no change. The tool's verb needs no feature.
- **Drain-cap slider.** Absent at the read commit, it landed on `main` with the spike (0c3be3858) and is merged here (4f6573a1a): `session/level_streaming/drain_cap.rs` (`set_play_drain_cap_override`, `drain_cap_lever`, `apply_drain_cap_lever`) and the renderer's `debug_ui/drain_cap_lever.rs`. Task 7 seeds that slider and builds no second one. The unit stays decoded bytes: `streaming-drain-time-budget` (time budget) is still a draft.
- **Preflight.** Preflight runs only `test_telemetry_reduce.py` from `tools/tests`. The new reducer test needs its own line in `.claude/skills/preflight/SKILL.md`.
- **Large files.** `main.rs` is 13,122 lines and `frame_loop/mod.rs` 2,962. Playback, recording and the record live in a new `playback/` module beside `capture/`. Those two files gain call sites only, and `app/render_extents.rs`'s source-order anchors keep their order.

## Delegated answers
- **Frame-mark mechanism** — **proven on the Mac** (branch `mac/camera-path-checkpoint` 159f47d64, `mac-results/README.md`, `nesting-join.txt`): debug groups, no label fallback. In an MST export, `pr-frame <k>` appears only on CPU encoding rows. It is in `metal-application-intervals` (as a suffix on the child encoder's label, `Blit Command 0  -  :pr-frame <k>`, depth + 1 under its command-buffer row, same `cmdbuffer-id`, own `encoder-id`), in `metal-object-label` and in `kdebug-strings`. It is not in `metal-gpu-intervals`, `metal-application-encoders-list` or `metal-command-buffer-completed`. Attribution: join a `metal-gpu-intervals` row (SDMA0/GFX channels) to the child encoder row by `encoder-id`, then read k from its label. Every drain, SH-copies and Frame Encoder command buffer in the 20 s trace had exactly one marked child (frames 724–1970 contiguous). xctrace's own "Frame N" is not k.
- **GPU clock source** — **proven on the Mac**: the attaching script polls `ioreg -r -c IOAccelerator` `PerformanceStatistics` beside `xctrace` into a sidecar the reducer joins by time. The AMD accelerator reported a non-zero `Core Clock(MHz)` on all 220 route polls (min 55, median 733, max 1222 MHz; utilization median 56%). It read zero on some polls at idle after exit, so zero reads as unrecorded. The engine records `gpu_clock: unrecorded` in-process. The clock was not pinned in that run (vsync 60, scene 1280×720), so its GPU figures are upper bounds.
- **Names and defaults:**
  - `--camera-route <poses>` plays a route.
  - `--camera-route-pacing frame-locked|wall-clock` sets pacing (default frame-locked).
  - `--camera-route-hold <frames>` sets the hold (default 600, about 10 s at 60 Hz to attach `xctrace`).
  - `--camera-route-report <path>` names the record (default `<poses stem>.playback.json` in the working directory).
  - `--record-camera-route <path>` records a route.
  - `--present-mode vsync|no-vsync` sets the present mode (default vsync).
  - `--camera-route-stall-timeout <seconds>` sets the stall timeout (default 300 s: campaign-test loads in about 72 s in a `dev` build, and the timeout must outlast a load plus Settling's 10 s on bigger levels).
  - Cap seed: `POSTRETRO_PLAY_DRAIN_CAP_MIB`, in decoded MiB, the slider's unit.
  - Playback window: 120 rows, matching the CPU window.
  - Tool verb: `postretro-tool generate-camera-route <prl> --kind far-point|random|smoothed --seed --steps --pitch --hfov [--speed --turn-cap] -o <file>`.
- **Smoothed defaults** — 8 m/s (the walk's `RUN_SPEED`, a run pace) and a 270°/s turn cap. Reported with the first smoothed run.
- **Over-budget charge** — the reducer reports both readings: a frame's own drain only, and its own plus drains still in flight. The run's findings say which holds.

## Contracts

**Pose file** (`postretro-camera-route` owns the writer and the strict parser). It is text, one record per line, and every float is written `{}` (shortest round-trip), so a parse is bit-equal. Angles are engine radians and positions are eye points in world metres.

```
postretro-camera-route 1
origin generated far-point|random|smoothed  |  origin recorded
params key=value ...        # generator: seed steps pitch hfov [speed turn_cap]; recorder: source_hz frames
projection hfov_degrees=<f32> aspect=<f32>
bake id51=<hex|none> walk_epoch=<u32> size=<u64> mtime=<u64> prl=<path as typed>
cadence_hz 60
spawn <x> <y> <z>           # exact kinds only: the walk's spawn eye (drawn first from the stream)
pose <x> <y> <z> <yaw> <pitch> [walk <step> <cell>] [cut]
```

- **Order:** the header lines appear once, in that order. `spawn` is optional. One or more `pose` lines follow.
- **`walk <step> <cell>`:** present on walk-made poses.
- **`cut`:** marks a pose reached by a teleport (smoothed kind). Interpolation never crosses it.
- **Header match:** the bake line is `bake_identity`'s string. A file matches a level on `id51` and `walk_epoch` only.
- **Fingerprint:** a walk pose's fingerprint is `pose_fingerprint(step, cell, eye, yaw, pitch, (hfov_degrees, aspect))`.

**Shared generator** (`postretro-camera-route`). Its free functions take a `&LevelWorld` and return poses in memory. The tool writes them, and the harness consumes them without a file. `exact_route(world, kind, steps, pitch) -> ExactRoute { spawn, poses }` reproduces the harness `run`'s draw order: one `SplitMix64(WALK_SEED ^ 0xE7E5)` stream, the spawn eye first, then one `eye_point` per dwell frame.

## AC-to-proof

Rows number top to bottom through Acceptance (Automated 1–48, Manual 49–51). "Lifecycle fixture" means `startup/lifecycle_test_fixtures.rs` driving install, Settling and reveal. Task 5 confirms how far the fixture reaches `redraw`. Where it stops, the proof is the playback state machine plus a source-order test at the call site, and the row's result says so.

| AC | Proof | Status |
|---|---|---|
| 1 exact file reproduces walk fingerprints | `camera_route` on-demand test vs the harness's in-memory walk, campaign-test, both walks (`POSTRETRO_LIGHTMAP_WALK_PRL`); synthetic-world unit test always on | achievable as stated |
| 2 pre/post-move trip records pair | baseline `POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_OUT` written before the move (Task 1), replayed with `_IN` after it, both walks, default steps | achievable as stated |
| 3 smoothed route obeys cells, speed, turn cap | `postretro-camera-route` unit tests on a synthetic world at two speed/turn-cap sets; on-demand campaign-test check | achievable as stated |
| 4 frame-locked bit-equal, view feel and mover | `playback` pacing tests; frame-eye test with a view-feel driver present; lifecycle fixture with a view-feel pawn on a mover | achievable as stated |
| 5 wall-clock pacing (P16) | pacing tests with injected elapsed times, slowed frames, last pose | achievable as stated |
| 6 file FOV at window aspect; header both projections | frame-eye test + record header test | achievable as stated |
| 7 Settling held view and reveal use first pose (P5) | lifecycle fixture with and without a pawn | achievable as stated |
| 8 no-draw rows (P9) | state-machine test | achievable as stated |
| 9 one-pose file; zero hold | state-machine tests, both pacings | achievable as stated |
| 10 pawn at spawn | lifecycle fixture | achievable as stated |
| 11 held key leaves pawn and portal mask; chord acts and marks (P12) | capture-gate test + lifecycle fixture + mark test | achievable as stated |
| 12 death/respawn logged (P11) | life-state observer test | achievable as stated |
| 13 CPU split carries `fixed_step` and ticks; sim never paused | row-fill test from a composed record | achievable as stated |
| 14 bad file / identity / FOV refusals | parser + validation tests with log capture | achievable as stated |
| 15 identity match from another path / copy; no id-51 warns once | validation tests | achievable as stated |
| 16 `--connect` and record+play refuse | arg tests | achievable as stated |
| 17 aborts in Settling / hold / route (P3, P4) | state tests + abort-hook test at the request drain and frontend join | achievable as stated |
| 18 level change on last-pose frame completes (P2) | state test | achievable as stated |
| 19 change marks and hidden gaps (P10) | state tests + hook tests at vsync, extents, window mode, hot reload, occlusion | achievable as stated |
| 20 stall timeout (P17) | state tests with injected clock | achievable as stated |
| 21 missing-feature warn-once | default-feature `cargo test -p postretro` with log capture | achievable as stated |
| 22 cap seed, slider shows it, settling unaffected | drain-budget tests + slider seed test (`dev-tools`) | achievable as stated |
| 23 present and pacing modes from first frame | renderer present-mode init test + header test | achievable as stated |
| 24 rows in order; full header; settle outcome | record tests | achievable as stated |
| 25 GPU time per route row on timestamp adapters | query-ring bookkeeping test; owner-free manual run on the Windows 1660 Super (has timestamps) | achievable with the clarification in Corrections |
| 26 stage fields absent with CPU timing off | row tests | achievable as stated |
| 27 cold flag | renderer GPU test (`POSTRETRO_REQUIRE_GPU=1` here) on pool `created` deltas | achievable as stated |
| 28 pool-growth flag, reducer counts apart | renderer GPU test + reducer unittest | achievable as stated |
| 29 row fill allocates nothing; one write | `alloc_probe` test + I/O-seam test | achievable as stated |
| 30 inactive: no allocation, no marks, labels unchanged | `alloc_probe` test + renderer label test | achievable as stated |
| 31 window event equals record rows; `null` CPU max when off | telemetry hub test | achievable as stated |
| 32 final partial window; `run_start` names producer (P18) | telemetry hub test | achievable as stated |
| 33 GPU figures name the clock | reducer unittest | achievable as stated |
| 34 reducer sample from a real run | reducer unittest on the Task 4 owner export | achievable; sample needs the owner's Mac run |
| 35 preflight runs reducer test | `SKILL.md` line + a deliberately failing run | achievable as stated |
| 36 P6 attribution | reducer unittest | achievable as stated |
| 37 P7 partial trace | reducer unittest | achievable as stated |
| 38 P8/P10 route-only figures | reducer unittest | achievable as stated |
| 39 SDK and player gating | xtask dist feature test + one `sdk-dist` build | achievable as stated |
| 40 grep gate | `xtask` surface gate (precedent `surface_gates.rs`) | achievable as stated |
| 41 generate determinism; path/copy header-only diff | tool tests on a fixture PRL | achievable as stated |
| 42 no GPU; parameter and level failures | tool tests | achievable as stated |
| 43 SDK bundle generates a valid file | one `sdk-dist` build, then the bundle's verb | achievable as stated |
| 44 recording is pre-feel; replay has no roll / FOV offset | recorder tests + frame-eye test | achievable as stated |
| 45 30 vs 60 fps resample; frame-locked replay; hitch | recorder resampling tests | achievable as stated |
| 46 recording skips Settling and no-world frames; header | recorder tests | achievable as stated |
| 47 recording scope: backdrop, level exits, no world frame (P13, P15) | recorder lifecycle hook tests | achievable as stated |
| 48 written on every exit route, same file (P14) | exit-hook test + source test that `exiting` flushes | achievable as stated |
| 49 Mac consumer run | owner, Mac | manual |
| 50 showcase copy run | owner, Mac | manual |
| 51 SDK bundle record / generate / play | owner | manual |

## Tasks

The thin slice is Tasks 1–4. It ends at the riskiest premise, that a Metal System Trace export attributes the drain encoders to a playback frame index. Task 1 comes first only because the baseline must predate the move. Tasks 5–10 continue while the owner's Mac export is pending. Task 11's sample waits on it.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Baseline: on-demand walk on campaign-test writes trip records for both walks at default steps, before any code moves; store under the brief | integrating executor | — | done: `baseline/trip-record-pre-move.txt` (gitignored, 5.9 MB, sha256 f7809c4a…5f0ae5). Random 93,326 frames, far-point 97,308 frames, bake id51=4b7255ff5bdf714c walk_epoch=4. Written at d129730c5 (walk code unchanged since ecd9489e0). Command: `POSTRETRO_LIGHTMAP_WALK_PRL=<abs>/content/dev/maps/campaign-test.prl POSTRETRO_LIGHTMAP_WALK_RUNS=default POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_OUT=<abs path> cargo test -p postretro --bin postretro lightmap_residency_walks_from_prl -- --ignored --nocapture` (paths must be absolute; the test's cwd is the crate). Replay: swap `_OUT` for `_IN`. |
| 2 | `postretro-camera-route` crate: move path, walk, eye stream and bake identity out of test code; harness `run` and `materialise` call it; radian pose format writer and parser; ACs 1–2 | worker, sequential | 1 | done: crate `postretro-camera-route` (Layer 3, depends on visibility); `paths.rs`/`walk.rs` deleted from the harness; `run` and `materialise` call `ExactRoute::of_walk`. `cargo test -p postretro-camera-route` 15 passed; `walk_measurement` 38 passed. AC 1: `camera_route_file_reproduces_the_walk_fingerprints_from_prl` passed on campaign-test, both walks. AC 2: replay with `_IN` against the baseline paired both walks with no pose mismatch. |
| 3 | `postretro-tool generate-camera-route` (exact kinds) with tests; ACs 41–42 | worker, sequential | 2 | done: `crates/tool/src/camera_route/`; `cargo test -p postretro-tool` 170 passed, including 6 `generate_camera_route_*` (determinism, path/copy header-only diff on `sh-streaming-hinted-door.prl`, failure cases). Hand run on campaign-test far-point wrote 97,308 poses. |
| 4 | Thin playback slice: flags, file validation, frame-locked override with base FOV and no view feel, hold, exit 0 at end, minimal record, debug-group frame marks. Then **owner Mac checkpoint**: campaign-test far-point, `xctrace` attached in the hold, export and `ioreg` poll returned | integrating executor | 3 | slice done: `playback/` module (dev-tools), `RenderCamera::with_base_hfov`, `FrameEyeInputs.base_hfov`, renderer frame marks on the upload queue (`uploads/frame_mark.rs`). Tests: `playback` 22 passed; camera/frame_eye/render_extents/resolve_map_path/client_weapon filters 157 passed (133 without dev-tools); renderer `frame_mark` 3 passed (`POSTRETRO_REQUIRE_GPU=1`, 1660 Super); clippy clean. Windows smoke run: 20-step far-point (821 poses) with hold 60 exited 0, 881 rows in order, identity proven. Bake mismatch found at install, refused on the first Settling frame (before any world frame). **Mac checkpoint done** (159f47d64 on `mac/camera-path-checkpoint`, not merged: 36 MB of xz tables stay on that branch). Engine exit 0, 2,646 frames (900 hold + 1,746 poses); both delegated questions answered under Delegated answers. One shutdown warning, `[candidate-cull] counter map failed: BufferAsyncError`, fires once per run after the record is written and before `[Engine] Exited`. It is pre-existing: `candidate_cull.rs` warns when a counter `map_async` is still pending at device teardown, and playback's exit right after a frame makes that race deterministic. It is outside this brief; the PR names it as a follow-up candidate. |
| 5 | Full lifecycle: Settling held view, input gate and menu suppression, no-draw rows, aborts, change and gap marks, stall timeout, refusals, `--connect`, missing-feature warning, life-state log, wall-clock pacing | worker, sequential | 4 | done: `playback/{lifecycle,run,pacing,marks,stall}.rs`, ungated `startup/camera_route_flags.rs`. Tests: `playback` 57 passed (dev-tools); no-feature `camera_route_flags`/`resolve_map_path`/`render_extents`/`settling` 48 passed; related dev-tools filters 203 passed + 1 ignored; clippy clean both ways. Proof kinds: `test_app` has no `ActiveEventLoop`, so app-level rows call the hooks in frame order on `test_app()`, with source-order tests pinning the call sites in `redraw` (ACs 4, 7, 10, 11, 13 rest partly on these). Smoke: frame-locked hold 60 → exit 0, 881 rows; wall-clock hold 0 → exit 0, 823 rows, last `route_time_s` 13.682 of 13.683; record+play refused before the window; stall 3 s on a fresh profile → exit 1 naming the accessibility panel, record aborted with 0 rows. Rulings: wall-clock route frame 0 is the first route frame that presents; any frame that presents nothing is a gap; mid-run changes count only once rows are written; the stall default raised to 300 s. |
| 6 | Record: full header, CPU rows, per-resource drain counts, cold and growth flags, both drawn-not-resident counts, GPU query ring, preallocated rows, one write | worker, sequential | 5 | done: record split into `playback/record/{mod,header,row}.rs`; renderer `uploads/playback_timing.rs` + `playback_ring.rs` (own query set, 8 groups × 6 slots, resolved in the frame encoder, readbacks patched by frame index, 2 s bounded wait at run end); `CpuFrameTimer::committed_record()`; `LevelStreamingStep.admission`; cold/growth as deltas of `PlaybackDrainCounters` across the frame; new production SH gauge `last_frame_drawn_not_resident` (`misses` counts episodes); build profile from `build.rs` (`OUT_DIR` profile dir). Whole-frame GPU span = first command of the frame's earliest marked encoder → last command of the frame encoder. Tests: `playback` 65 passed; `cpu_timing` 22; renderer `playback`+`frame_mark` 11 passed (`POSTRETRO_REQUIRE_GPU=1`); `staging_pool` 3; clippy clean in all four feature sets. Smoke (campaign-test 821 poses, hold 60, vsync): timing on → 881 rows, CPU and GPU fields on every route row, 0 GPU rows missing, 2 cold frames, lightmap drain GPU median 60 µs / max 291 µs, SH upload median 1,841 µs / max 3,891 µs; timing unset → stage and GPU fields null, drain and late columns present. Growth and late counts were 0 on this route; unit tests prove them. A timed frame makes one small allocation inside wgpu's `map_async` (as per-pass timing does); inactive playback allocates nothing. |
| 7 | Cap seed for the spike's dev-tools drain-cap slider; `--present-mode` | worker, sequential | 4 | done: `POSTRETRO_PLAY_DRAIN_CAP_MIB` read once per session (`LevelStreaming::at_launch`, holds across levels until the slider moves), parsed by the walk lever's shared `parse_cap_mib`; malformed warns once and keeps 8 MiB; a build without dev-tools never reads it (like `POSTRETRO_NET_LAG`). Header `cap {source: production|seed|slider, decoded_bytes_per_drain}` at route frame 0; a slider move calls `note_change(DrainCap)`. `--present-mode vsync|no-vsync` is `PresentChoice` passed to `Renderer::new` for the first frame, in any dev-tools session; without dev-tools it warns once and uses vsync. |
| 8 | Telemetry: playback window kind, producer state, final partial | worker, sequential | 6 | done: `Kind::PlaybackWindow` (`playback_window`), `playback` producer in `run_start` (enabled / disabled naming `--camera-route` / unsupported). A window is 120 consecutive record rows; figures cover measured rows only (route rows with no change or gap mark) and the payload names `rows`, `route_rows`, `measured_rows`, `excluded_rows`. Final partial emitted in `end_playback_run` before the record write and `run_end`. Tests (with Task 7): dev-tools filters 137 passed, no-feature 51, telemetry crate 35, renderer `present_choice` 1; clippy clean in five configurations. Smoke: seed 4 MiB + no-vsync → exit 0, 881 rows, header `cap.source=seed`, route intervals median 4.24 ms; 8 windows (7 full + 1 partial) before `run_end`, all 8 equal an independent reduction of the record. Seed `abc` → one warning, `production`. |
| 9 | Smoothed generator kind and verb parameters; AC 3 | worker, worktree | 2 | done (10bc3bdf2): `camera-route/src/smoothed/` — legs end at portal centroids nudged inward, each segment proven against the cell-locator plane tree to touch only {current, next}; seeded Dijkstra detours otherwise; walk teleports cut. Tool `--kind smoothed --walk --speed --turn-cap` (defaults 8 m/s, 270°/s). camera-route 21 passed + 1 ignored; tool `camera_route` 8 passed; clippy clean. campaign-test on-demand: far-point 132,126 poses, random 96,907, 0 locate cuts, max 0.13334 m and 4.5° per pose, every straight pair at full stride (far-point 80.4% of poses at full stride, random 66.3%: U-turns turn in place). |
| 10 | Recording: pre-feel recorder, resampling, first-level scope, exit flush | integrating executor | 5 | |
| 11 | MST reducer in `tools/` with unittest from the Task 4 export; preflight line | worker, sequential | 4 (owner export), 6 | |
| 12 | Grep gate, gating test, `docs/` page, one `sdk-dist` bundle check | integrating executor | all | |
