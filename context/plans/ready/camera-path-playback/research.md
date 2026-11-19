# camera-path-playback — research

Read at ecd9489e0. Derivation behind `index.md`; nothing here is a decision.

## The spike rows the request cites
`streaming-drain-budget-spike/index.md` has 14 Automated and 8 Manual rows, numbered top to bottom.
- Row 14 is the last Automated row, the grep gate: the cap slider builds only with dev-tools; the walk override, the walk tallies and any test-only lever live only in test code; no player setting names the drain cap.
- Row 18 is the fourth Manual row: "If the gate opens: install cost per resource, CPU and GPU, as fixed cost per item and cost per MiB". It is conditional on the gate. Its only measured input today is the lightmap's warm CPU install rate, 0.25 ms/MiB, from `lightmap_drain_install_timing` (`plans/done/spatial-residency--lightmap-cell-blocks/findings.md`). SH install cost and both resources' GPU cost are unmeasured. The spike's planned GPU source is an offline renderer drain test reduced from a Metal System Trace (MST). That test drains on an otherwise idle GPU, so it cannot show the cost of a drain landing in a busy frame.
- No spike row reports frame time per cap. Fixtures say "Frame and GPU time are reported, not gated", and row 17 reports drain CPU p95/max only.

## Where the late-block numbers come from
The request cites 180 / 145 / 322 late blocks. None of these appear as late-block counts anywhere in `context/` or in git history. The nearest recorded figures (`plans/done/lightmap-compact-storage/plan.md`, Tasks 6 and 7, showcase at 0.0125 m):
- realtime medians: far-point 180–182 (median 181), random 306;
- completed mode: far-point 79 (SH on 78), random 279;
- campaign-test realtime far-point 47.

Likely source: the spike's own first-slice runs, not yet committed. The brief cites no count.

## Near items, checked
- **Scripted-run capture** (`ready/E20--scripted-run-capture/`, roadmap Epic 20). Runs scripted ticks offscreen and captures at chosen ticks. Its streamed continuous mode streams every tick but times nothing and promises no frame pacing; its other modes preload or settle each view. Capture is synchronous and offscreen (`capture/driver.rs`). Its lightmap modes are placeholder, all-resident, or stream with a preload at the capture pose (`capture/lightmap.rs`), so one pose is made resident before its frames and nothing streams after. Its measurement mode times warmup and sample frames at that one pose. Capture waits for the GPU after every submit (`rendering_pipeline.md` §12), so its frame times do not reflect a pipelined windowed frame.
- **Record / replay** (roadmap Epic 20, candidate). Records a command and input trace and replays it "on the shipped determinism foundation". So determinism itself is shipped; the missing pieces are a recording format and a replay driver through the sim. Replayed input moves a pawn through movement, so the camera is a sim output, not a fixed route.
- **`drafts/gpu-pass-paired-ab`**. Pairs a pass's WGSL against a B source within one launch. Not relevant to drains. Relevant precedents: its dependency-free cargo feature outside dev-tools; its stdlib Python MST reducer in `tools/`, which reduces per encoder and takes per-frame figures only from a caller-named once-per-frame encoder; its rule that labels stay unchanged when the mode is off.
- **`in-progress/E20--streaming-telemetry`**. Its events are per load, per window and per stage. Its file sink flushes every line on the warrant that those rates are low. One event per frame would void that warrant. Its non-goals leave new kinds to later subsystems. Its leaf crate is not in source at the read commit, although `development_guide.md` §6.5 already describes it.

## The walk's frames
- Placement: `walk_measurement` is `#[cfg(test)] mod walk_measurement` inside `session/lightmap_residency/mod.rs`. It is unreachable from any non-test build.
- Path: `walk_path` (`paths.rs`), a port of the dry run's `camera_walks.rs`, walks portal adjacency over camera cells (non-solid, non-exterior, ascending id). `WalkKind::Random` picks a uniform neighbour and teleports after 1,000 steps without a new cell. `WalkKind::FarPointTour` takes BFS hops to the farthest unvisited cell, teleporting when none is reachable. The seed is `WALK_SEED`.
- Frames: `Walk::build` (`walk.rs`) gives each step a dwell in frames: distance to the next cell centre at `RUN_SPEED` 8 m/s at 60 Hz, capped at 60 frames. The run loop (`walk_measurement/mod.rs` `run`) draws a fresh `eye_point` inside the cell on every frame, from a second seeded stream (`WALK_SEED ^ 0xE7E5`). Yaw is `travel_yaw + frame × 90°`, at a fixed pitch. The camera therefore jumps within the cell and turns a quarter turn every frame. On screen that is a strobe, not a walk.
- Projection: `render_camera` (`modes.rs`). HFOV default 100°, 16:9, near 0.1, far 4096, from `POSTRETRO_LIGHTMAP_WALK_HFOV` / `_ASPECT` / `_PITCH`. The walk takes any HFOV; the engine's `RenderCamera::new` clamps to 60–130° (`camera.rs`).
- Frame identity: `pose_fingerprint` (`trip_record.rs`) hashes step, cell, eye, yaw, pitch and projection bits.
- Determinism: frame i is the same pose in every run over one cell layout. A rebake that changes cells changes the path.

## Serialize poses or regenerate in-engine
- **Precedent for serializing.** `world_poses/bake_gap.rs` already materialises both walks' frames exactly as `run` does and writes each frame's eye, yaw and pitch, with the spawn eye, to a text record (`postretro-bake-gap-world-record 2`). A later build replays those frames. `world_poses/poses.rs` keeps a pose file (`postretro-world-poses 1`, `pose label x y z yaw pitch`). It writes floats in their shortest exact form, so a replay is bit-equal. Both carry the bake identity (`baked_set.rs` `bake_identity`: id-51 hash, walk epoch, PRL size, mtime and path as typed). The walk's own same-bake check compares only the id-51 hash and walk epoch (`walk_measurement/mod.rs`).
- **Regenerating in-engine** would move `paths.rs` and `walk.rs` into production code and tie the engine's route to the walk's RNG streams and the bake's cell ids. A change to either generator would silently change the measured route. A hand-made or recorded route would need a second path. The spike's row 14 keeps the walk machinery in test code.

## The windowed camera
- Per frame, `redraw` (`frame_loop/mod.rs`) takes `presented_aim_pose(alpha)` and passes it through `render_aim_pose`. That function already replaces the gameplay pose with the held menu camera whenever the frontend menu is present. `apply_menu_camera_pose` (`main.rs`) holds `frame_timing` at that pose. `frame_eye::assemble_frame_eye` then folds view feel from the followed pawn into one `RenderCamera`. Visibility, render, the audio listener and streaming all read that one eye.
- Streaming reads the camera only through visibility. `run_level_streaming_step` receives `StreamingFrame { visible_cells, sh_visible_cells, camera_cell, path, monotonic_seconds: script_time, settling }`. It reads no sim state. The sim does not need to tick for streaming, but the frame loop ticks it regardless.
- Engine demand on the walk's poses still differs from the walk's: doors seal portals, aspect follows the window, planar mirror cells join, and Settling streams from the first pose.
- Settling suspends gameplay input on its frames only (`startup/settling.rs`). After the reveal, input drives the pawn as in play, and a door it opens changes the portal mask visibility reads.
- With no pawn, `Camera` is a free-fly camera (`camera.rs`). There is no noclip or spectator camera mode in the binary, and no debug camera beyond the frontend menu hold.
- `--start-pose x,y,z,yaw_deg,pitch_deg` (`startup/start_pose.rs`) places the local pawn's capsule centre, or the fly camera when there is no pawn. It applies on the first gameplay install only and is skipped under the frontend menu (`install_level_camera_pose`). The presented pose then settles (`boot_sequence.md` §Settling). Walk and world poses are eyes. `poses.rs` names the 0.5 m capsule-centre-to-eye offset.

## Launch-time surfaces
- CLI: `--start-pose`, `--capture`, `--headless`, `--observe-live`, `--windowed`, among others. There is no exit-after-N-frames flag.
- Env: `POSTRETRO_CPU_TIMING` and `POSTRETRO_GPU_TIMING` in every build. Launch seeds for dev-tools panels are `POSTRETRO_SH_PROBE_OVERLAY` and `POSTRETRO_NET_LAG`, and `development_guide.md` §6.4 requires a seed to ship with its surface. The walk's `POSTRETRO_LIGHTMAP_WALK_*` variables, including `_POOL_LAYERS` (`levers.rs`), are read only by test code.
- Features: `dev-tools` (egui; an SDK bundle's debug authoring engine carries it, its release engine has default features), `observability`, `observe-live` and `capture` (dependency-free), `tracy`, `test-support`.
- Drain cap: `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` is a `const` (`streaming/drain_budget.rs`), read by the one `admit_within` call in `session/level_streaming/mod.rs`. Neither spike lever is in source yet.

## Per-frame timing that exists
- CPU: `CpuFrameTimer` composes a per-frame `FrameRecord` before folding it into a 120-frame window (`cpu_timing/frame_timer.rs` `compose`). The record holds total, work, wait (acquire, present), the top-level stages and the nested sets, including `StreamingStage::LightmapResidency`, `LightmapDrain` and the renderer's `render_sh_drain`. Only windows leave the timer; `last_record` is test-only.
- GPU: timestamp windows of 120 readbacks. A readback is not a frame. The Mac target has no timestamp queries, so GPU time comes from MST (§12).
- Tracy: the `tracy` feature makes each stage scope a zone and emits a frame mark (`stage-timing` `lib.rs`).
- Precedents for a report: the capture `MeasurementReport` JSON (`capture/report.rs`: schema, revision, map, adapter, camera, CPU samples, timing availability); the walk's `walk-result {json}` lines.

## Frame marks in a trace
- No code emits `push_debug_group`, `insert_debug_marker` or a signpost. The wgpu version is 30.0.1.
- Drain GPU work has labelled encoders: "Streamed Lightmap Drain" (`lighting/lightmap/stream/execute.rs`) and "Streamed SH Upload Copies" (`render/uploads/staged_uploads.rs`), beside "Per-frame Upload Copies" and "Renderer Upload Staging".
- §12: wgpu forwards pass labels into MST; reduce per encoder; divide by a once-per-frame encoder; never count frames from render-pass encoder counts. Not verified: whether MST's exported tables carry wgpu debug groups or markers, and whether `Instant` and MST share a timebase on macOS.
- Candidates: a frame-indexed label on one once-per-frame encoder during playback only; debug groups; time alignment against the record. The first slice picks one by proving it on a real export.

## Frame pacing
- Present mode is `AutoVsync` at init (`renderer_init.rs`). `Renderer::toggle_vsync` switches to `AutoNoVsync` from the Alt+Shift+V diagnostic chord, which is not feature-gated. No launch-time knob exists.
- With vsync on, `wait_acquire` absorbs GPU-bound time, and frame intervals quantize to the display refresh. A drain shows as a stutter only when it pushes a frame past the budget, which is what a player sees. With vsync off, frame time tracks work. §12: CPU windows reset on a vsync toggle.
- §12 confounders apply to every run: pin the GPU clock by saturating it; record surface and scene extents, render divisor and effective window mode; keep the window in front; attach `xctrace` to a windowed run rather than `--launch`.

## Pins
Orderings the Acceptance rows cite. A route frame is a world frame after the hold; route frame k draws pose k.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Hold of N ≥ 1 frames, then the route | Hold frames 1..N draw the first pose; the next world frame is route frame 0 | Route frame 0 draws the first pose and route frame k draws pose k. The first pose is drawn N + 1 times, once as a route frame. Every other pose is drawn once |
| P2 | A level request queued during the frame that draws the last pose | The frame presents and commits its CPU record; the run completes before the next redraw drains requests | The record is complete and not aborted, its last row holds that frame's own CPU split and drain counts, and the process exits 0 |
| P3 | A level change, unload, reload or frontend join requested during route frame k | The request drains at the top of frame k + 1, before any world draw | Rows through frame k stay. No row k + 1. The record is marked aborted with its reason, and the process exits non-zero |
| P4 | A level request or frontend join drains during Settling or the hold | The held level unloads before the route starts | The run aborts as one mid-route does. The record holds the hold rows written so far, none if Settling had not revealed |
| P5 | First gameplay install with a local pawn, then Settling, then the reveal | Install places the pawn at the map's spawn; the camera takes the first pose; Settling streams from that camera | Every Settling frame's held view and the reveal frame use the first pose's eye, yaw and pitch bit-equal, at the file's FOV, with or without a pawn. The pawn stands at the spawn |
| P6 | Frame k's lightmap drain submits before frame k's render work; the GPU runs a frame-k drain after frame k + 1's work begins | Encoders are recorded on frame k and executed later | Both drains are attributed to frame k, never to k − 1 or k + 1 |
| P7 | Trace attached mid-hold, ending before the route ends, or missing rows from part of the run | Marks cover a subset of rows | Per-run figures cover matched route frames only. The count of unmatched route frames is named. An unmatched frame is never zero drain GPU time. Hold frames before the attach are not an error |
| P8 | Hold rows, then route rows, in one record | The reducer summarizes the run | Per-run interval p50, p95, max and frames over budget cover route frames only. Hold rows are reported apart |
| P9 | A frame streams and drains from pose k, then gets no surface | The streaming step and the SH drain run before surface acquire | One row marked no-draw carries that frame's admitted bytes, items and drain flags. The route does not advance; the next world frame draws pose k |
| P10 | A vsync chord, resize, window-mode change, cap-slider move or script hot reload during route frame k; or world frames stop while the window is hidden, then resume | The change lands between frames and persists, or the next world frame's interval spans the gap | A persistent change marks every row from the first one it touches to the run's end. A gap marks only the rows whose interval spans it. Marked rows stay in the record and leave per-run figures. The run continues to its end and exits 0 |
| P11 | The pawn dies or respawns during route frame k | The sim ticks before the presented pose is taken | The record logs the life-state change on frame k. The camera stays on the route, and the run continues |
| P12 | A movement key held from Settling through route frame k; a vsync chord pressed on frame k | Settling's input suspension ends at the reveal; playback's covers the reveal frame onward | On every frame the pawn's position and the portal mask equal those of the same run without the key. The chord switches the present mode and marks rows from k on |
