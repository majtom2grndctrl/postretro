# camera-path-playback

Brief · compact · reads: `context/lib/rendering_pipeline.md` §11, §12 (GPU Pass Timing, CPU Stage Timing, Machine-state confounders) · `context/lib/development_guide.md` §6.4, §6.5 · `context/lib/boot_sequence.md` §Settling · `context/plans/in-progress/streaming-drain-budget-spike/` · read at ecd9489e0 · evidence: `research.md`

## Problem
The agent running `streaming-drain-budget-spike` reported gaps the owner relayed. The spike cannot measure what a drain costs the GPU while the GPU is busy rendering: its install-cost row plans an offline drain test on an idle GPU. It also cannot measure frame-time spikes at each drain cap, which is the owner's stutter question. Both need the real renderer drawing every frame while a repeatable camera route streams new areas. No such route reaches the windowed engine (`research.md` §Near items, §The windowed camera). This is a capability for engine developers measuring streaming and residency. When this is done, a measurement build flies the camera along a pose file, one pose per rendered frame. Routes come from the walk harness: its exact frames, and a smoothed route through the same cells that moves like a player. The drain cap is set at launch. The run writes a per-frame record of frame time and drain work, marks each frame so a Metal System Trace reduces per frame, and exits at the route's end.

## Decisions
- **Poses are serialized, never regenerated in the engine.** The walk harness writes its far-point and random frames to a pose file; the engine only reads files. The generators stay in test code, so the spike's grep gate (walk machinery only in test code) holds unchanged. A route the engine regenerated would follow the walk's RNG streams and the bake's cell ids, and would silently change with either (`research.md` §Serialize).
- **Two routes from the walk, one format.** The harness writes its exact frames, and a smoothed route through the same cell sequence at run speed with a capped turn rate, cut where the walk teleports. Exact frames price a saturated drain. The smoothed route serves the vsync stutter rows and any visual read; the exact frames strobe on a visible window.
- **The pose file is world space.** Each pose is an eye position, not a capsule centre, with yaw and pitch in engine radians, written exact so a replay is bit-equal. A walk-written pose also carries its walk step and cell, so a file reproduces the walk's pose fingerprints. A header carries the bake identity the walk records, the projection the poses were generated for, and how they were made. The format extends the bake-gap record's radian frames, not the world-poses file's degrees, and adds no third pose dialect.
- **One pose per rendered world frame.** Pose k drives the k-th route frame that draws the world, at any frame rate. Rival: poses by elapsed time, which skips poses on slow frames, so each run would stream something different. The route is the walk's poses, but engine demand is not the walk's: doors seal portals, aspect follows the window, planar mirror cells join, and Settling streams from the pose. Playback renders with the file's horizontal FOV at the window's aspect, and the header records both projections. The engine's counts are never compared as equal to the walk's.
- **Placement: the binary owns playback and the renderer owns the frame marks.** Playback replaces the presented pose at the seam where the frontend menu already holds its camera, and bypasses view feel, so the render eye equals the file pose. Streaming, visibility, render and the audio listener follow that eye unchanged. Streaming reads the camera only through visibility (`research.md` §The windowed camera). Nothing streaming-specific is overridden, so the drains measured are production drains.
- **The pawn stays at spawn; the sim keeps ticking.** The pawn stays at the map's normal spawn and only the camera follows the route. The measured frame stays the production frame. The record logs any pawn life-state change.
- **Run lifecycle.**
  - Settling streams from the first pose through the camera.
  - Playback then holds the first pose for a launch-set number of frames, so a trace can attach and the settle tail drains. Hold frames are recorded, marked as hold.
  - A frame that draws no world does not advance the route. One that streams and drains without drawing writes a row marked no-draw carrying its drain work.
  - A vsync chord, a resize or window-mode change, a cap-slider move, a script hot reload, or a hidden-window gap marks the affected row with the change. That row is left out of per-run figures, and the run continues.
  - After the last pose, playback writes the record and exits 0.
  - A level change, unload or reload during playback ends the run: the record is written and marked aborted with its reason, and the process exits non-zero.
- **Playback refuses to start on bad input or a connected client.** An empty, malformed or unreadable pose file stops the launch, and so does one that does not match the loaded level. A file matches on the id-51 hash and walk epoch only, the walk's own same-bake check, so a matching file at another path or a copied PRL is accepted. When the level has no id-51 hash, the file is accepted with a warning and the header marks its identity unproven. The error names the file and the reason. This departs from `--start-pose`, which warns and starts at the spawn, because a run that silently draws the spawn view reads as a result. Playback also refuses to start when the session is a connected client.
- **The cap is set at launch through the spike's slider.** The cap is the launch seed of the spike's dev-tools drain-cap slider (`development_guide.md` §6.4: a seed ships with its surface). When unset, play uses the production cap, and Settling keeps its own cap either way. If the slider does not exist when this builds, this brief lands the slider with its seed, under the spike's lever decisions: with levers unset, admission is identical. Neither the cap nor playback is ever a player setting.
- **Present mode is set at launch.** Vsync defaults to on and can be turned off at launch for the run. The record names the mode. With vsync on, frame intervals show the stutter a player sees. With vsync off, they show cost (`research.md` §Frame pacing).
- **The record is a separate file, not telemetry events.** One event per frame would void the per-window rate warrant behind the telemetry sink's per-line flush (`in-progress/E20--streaming-telemetry`).
  - Its header records what §12 requires of a performance run: revision, build profile and features, adapter, surface and scene extents, render divisor, window mode, present mode, cap, the pose file, both bake identities, both projections, the settle outcome (settled or timed out), and CPU and GPU timing availability.
  - Each frame row holds the frame index, the pose index or hold, the frame interval, and the CPU split with the streaming stages.
  - Each frame row also holds admitted bytes and items per resource, whether a drain made a staging pool create a buffer (a cold drain), whether a drain grew a lightmap pool, and drawn-not-resident counts for both resources.
  - Stages are absent, never zero, while CPU timing is off.
  - The record is filled without per-frame allocation or I/O and written once at the end.
- **Gating: SDK bundles carry playback; player builds do not.** Agent-oriented development is part of the architecture, so playback and the launch cap seed ship in SDK bundles. Playback rides `dev-tools` only if inactive playback costs nothing per frame. If the build finds a per-frame cost it cannot remove, playback moves to its own dependency-free feature that SDK bundles also enable. Release (player) builds carry neither. `development_guide.md` §6.4's SDK sentence is corrected at promotion: SDK bundles ship a dev-tools engine (`xtask/src/dist.rs`, `tool/src/sdk_dist/assemble.rs`).
- **Frames carry marks only during playback.** Each playback frame's GPU work can be attributed to its frame index in an exported trace. Outside playback, every encoder label stays as it is today, so per-encoder reducers keep their grouping.
- **One reducer in `tools/`,** with a unittest that preflight runs. It joins the record with an MST export and reports per frame: interval, drain-encoder GPU time, cold flag and pool-growth flag. Per run it reports interval p50, p95 and max, and frames over budget, with cold and pool-growth drains counted apart. It follows `drafts/gpu-pass-paired-ab`'s per-encoder rules. If that reducer lands first, this one extends it rather than forking it.
- **Non-goals:**
  - Input record and replay: roadmap Epic 20's candidate replays input through the sim, and this brief drives no sim.
  - Offscreen or headless playback: capture owns offscreen rendering.
  - Recording a live play session into a pose file, and authored routes. The file format admits both later.
  - Choosing the drain cap: the spike owns that.

## Acceptance
### Automated
**Route fidelity:**
- [ ] An exact-frames file the walk harness writes for both walks at default steps reproduces every frame's pose fingerprint of the harness's in-memory walk, and two writes are byte-identical.
- [ ] A smoothed file visits the walk's cells in the walk's order. Except across a marked cut where the walk teleports, every pose's eye locates in the walk's current cell or the next one, and between consecutive poses the eye moves no faster than run speed and turns no more than the turn cap. Away from turns the turn cap slows, the eye moves at the run speed the executor reports. Two writes are byte-identical.
- [ ] Over an N-pose file, the first world frame after the hold draws the first pose as route frame 0, and route frame k renders with an eye, yaw and pitch bit-equal to pose k, including when the followed pawn carries view feel and when it stands on a mover. Each pose is drawn exactly once as a route frame. Hold frames draw the first pose and are not route frames (pin P1).
- [ ] Every route frame renders with the file's horizontal FOV at the window's aspect, and the header records the file's projection and the rendered one.
- [ ] With and without a local pawn, every Settling frame's held view and the reveal frame use the first pose's eye, yaw and pitch bit-equal (pin P5).
- [ ] A frame that draws no world and runs no streaming step neither advances the route nor writes a row. A frame that streams and drains from pose k and then gets no surface writes one row marked no-draw carrying its admitted bytes, items and drain flags, and the next world frame draws pose k (pin P9).
- [ ] A one-pose file settles, holds, draws that pose once as a route frame, writes the record and exits 0. A hold of zero frames starts the route on the first Running frame.

**Pawn and sim:**
- [ ] With a local pawn, the pawn starts at the map's normal spawn, and no playback frame sets its position or orientation; only the camera follows the route.
- [ ] A pawn death or respawn during the route is logged in the record on the frame it happens, and the route continues (pin P11).
- [ ] With CPU timing on, every route frame's CPU split carries the fixed-step stage and its tick count as a play frame does. Playback never pauses the sim.

**Lifecycle and refusal:**
- [ ] An empty file, a file with a header and no poses, and a malformed or unreadable file each stop the launch with an error naming the file and reason, before any world frame. So does a file whose id-51 hash or walk epoch differs from the loaded level's.
- [ ] A file whose id-51 hash and walk epoch match is accepted when the PRL is loaded from another path, or is a copy with another modification time, and the header records both full identities. On a level with no id-51 hash, the file is accepted with one warning and the header marks the identity unproven.
- [ ] Playback refuses to start in a session that is a connected client, with an error naming the reason.
- [ ] A level change, an unload or a level reload requested during Settling, the hold or the route writes the record marked aborted with its reason, keeps every row written before the request drained, and exits non-zero (pins P3, P4).
- [ ] A level change requested on the frame that draws the last pose leaves the run complete. The record holds the last route row with its own CPU split and drain counts, is not marked aborted, and the process exits 0 (pin P2).
- [ ] A vsync chord, a resize, a window-mode change, a cap-slider move, a script hot reload, and a hidden-window gap mid-route each mark the affected row with that change, per-run figures leave the row out, and the run reaches its end and exits 0 (pin P10).
- [ ] In a build without playback's feature, the flag warns once naming the missing feature, and the run behaves as if it were unset.

**Cap and pacing:**
- [ ] With the cap seed set, every play drain from the first Running frame admits against the seeded cap, and the dev-tools drain-cap slider shows the seeded cap at launch. With it unset, drains admit against the production cap. A settling drain uses the settling budget either way. A malformed seed warns once and keeps the production cap. A build without dev-tools ignores the seed.
- [ ] The present mode chosen at launch is the one the surface runs from its first frame, and the record names it.

**Record and reducer:**
- [ ] The record holds one row per hold, route and no-draw frame, in order, and its header holds every field the Decisions list, the settle outcome naming settled or timed out as Settling ended.
- [ ] Stage fields are absent with CPU timing off and present with it on. The frame interval, admitted bytes and items, drain flags and drawn-not-resident counts are present either way.
- [ ] A frame whose drain made either resource's staging pool create a buffer is flagged cold. A frame whose drains reused pooled buffers is not, nor is one whose only buffer creation served a per-frame upload.
- [ ] A frame whose drain grew a lightmap pool is flagged as pool growth, and the reducer counts it apart from warm drains, as it does cold drains.
- [ ] Filling a playback row allocates nothing (`alloc_probe`), and the record does no I/O until its one write.
- [ ] With playback inactive, a frame allocates nothing for playback, writes no frame mark, and every encoder label equals today's.
- [ ] The reducer's unittest summarizes a checked-in record and trace-export sample into the expected per-frame and per-run tables. A record with no matching frame marks is an error. The sample comes from one real playback run, and every route frame in it matches a frame mark.
- [ ] Preflight runs the reducer's unittest, and a failing reducer test fails preflight.
- [ ] In the reducer's unittest, a drain recorded on frame k and submitted before frame k's render work is attributed to frame k, and so is a frame-k drain the GPU runs after frame k + 1's work begins (pin P6).
- [ ] A trace export covering only part of the route gives per-run figures over the matched route frames and names how many route frames went unmatched. An unmatched frame never counts as zero drain GPU time, and hold frames before the trace attached are not an error (pin P7).
- [ ] The reducer's per-run interval figures and over-budget count cover route frames only. Hold, no-draw and change-marked rows are reported apart (pins P8, P10).

**Gating:**
- [ ] The SDK bundle's authoring engine builds with playback and the cap seed. The player dist engine builds with neither.
- [ ] Grep gate: playback code builds only behind `dev-tools` or its own feature, and the pose writer lives in test code. No player setting or options UI names playback, the pose file or the drain cap.

### Manual
- [ ] First consumer: Mac perf target, `--profile profiling`, campaign-test far-point routes, CPU timing on, `xctrace` attached during the hold. At the production cap and at least one other: drain GPU time per frame from the reducer on the exact frames with vsync off, and frame-interval stutter on the smoothed route with vsync on. Machine state follows §12, and cold and pool-growth drains are counted apart.
- [ ] The same route on the showcase copy, when the spike's frozen copy is available, recording its hash.

## Path
- **Seams:**
  - The pose writer goes beside `world_poses/bake_gap.rs` `materialise` and its radian frame record (`write_record`, `parse_record`), and `trip_record.rs` `pose_fingerprint`. The parser moves out of test code with the format. The walk's `bake_identity` and `bake_token` (`baked_set.rs`) move with it; the same-bake check to follow is the walk's `id51`/`walk_epoch` comparison in `walk_measurement/mod.rs`.
  - The override goes in `render_aim_pose` (`main.rs`, beside `apply_menu_camera_pose`) and the view-feel driver choice in `redraw` (`frame_loop/mod.rs`, before `assemble_frame_eye`); the connected-client `life_state_eye` adjustment follows it. The file's FOV replaces `HFOV` in `RenderCamera::new` (`camera.rs`). Settling's held view reads `App::presented_pose` (`startup/presented_pose.rs`), the chokepoint the menu pose takes; the pawn's install is untouched.
  - Cap: the `admit_within` call in `session/level_streaming/mod.rs`. Admitted bytes per resource come from `SharedDrain` after admission.
  - No-draw rows: the streaming step runs before render in `redraw`, and the SH drain before surface acquire (`renderer_render_frame.rs`).
  - CPU rows: expose `CpuFrameTimer`'s composed `FrameRecord`, test-only today, rather than re-timing.
  - Cold flag: buffer creation in `renderer/src/render/uploads/staging_pool.rs`. Lightmaps, SH residency and per-frame uploads each own a pool (`lighting/lightmap/stream.rs`, `render/sh_streaming/gpu/setup.rs`, `render/uploads/mod.rs`); confirm which pool each drain's uploads draw from. Pool growth: `PoolTextures::new` and `copy_layers_into` in `lighting/lightmap/stream/execute.rs`.
  - Record shape: the capture `MeasurementReport` (`capture/report.rs`). Present mode: `Renderer::toggle_vsync` and the init-time `AutoVsync`.
- **Shape and rival:** the chosen shape is a file-driven pose override in the windowed binary. The rival is regenerating the walk from seed and map in the engine, rejected in Decisions.
- **First slice:** a pose file of a few hundred far-point frames played on campaign-test with one frame-mark mechanism, then `xctrace export`. It falsifies the riskiest assumption: that an MST export can attribute the "Streamed Lightmap Drain" and "Streamed SH Upload Copies" intervals to a playback frame index (`research.md` §Frame marks).
- **Large files:** `main.rs` and `frame_loop/mod.rs` are past 800 lines. Put playback in its own module and add only calls there.

## Open questions
- The frame-mark mechanism: a frame-indexed label on one once-per-frame encoder during playback, debug groups, or time alignment. — **delegated**: the first slice proves one on a real export.
- Per-frame GPU timestamp columns on adapters that support them. — **delegated**: optional, and absent on the Mac.
- Pose-file flag and seed names, and the hold's default length. — **delegated**.
- The smoothed route's turn cap and its run speed source. — **delegated**: the executor picks values that read as play and reports them.
- Whether an over-budget frame interval is charged to its own row's drain only, or also to drains still in flight. — **delegated**: the reducer reports both, and the run's findings say which reading holds.
