# camera-path-playback

Brief · compact · reads: `context/lib/rendering_pipeline.md` §11, §12 (GPU Pass Timing, CPU Stage Timing, Machine-state confounders) · `context/lib/development_guide.md` §6.4, §6.5 · `context/lib/boot_sequence.md` §Settling · `context/plans/in-progress/streaming-drain-budget-spike/` · read at ecd9489e0 · evidence: `research.md`

## Problem
The agent running `streaming-drain-budget-spike` reported two gaps, and the owner relayed them. The spike cannot measure what a drain costs the GPU while the GPU is busy rendering. Its row 18 plans an offline drain test on an idle GPU. It also cannot measure frame-time spikes at each drain cap, which is the owner's stutter question. Both need the real renderer drawing every frame while a repeatable camera route streams new areas. No such route reaches the windowed engine. The walk harness that generates the spike's routes is test code with no renderer. Capture renders one preloaded pose. Every windowed camera follows input or the frontend menu. This is a capability for engine developers measuring streaming and residency. When this is done, a measurement build flies the camera along a pose file, one pose per rendered frame. Routes come from the walk harness: its exact frames, and a smoothed route through the same cells that moves like a player. The drain cap is set at launch. The run writes a per-frame record of frame time and drain work, marks each frame so a Metal System Trace reduces per frame, and exits at the route's end.

## Decisions
- **Poses are serialized, never regenerated in the engine.** The walk harness writes its far-point and random frames to a pose file. It reuses the frame materialisation `world_poses/bake_gap.rs` already does. The engine only reads files. The generators stay in test code, so the spike's row 14 holds unchanged. A route the engine regenerated would follow the walk's RNG streams and the bake's cell ids, and would silently change with either (`research.md` §Serialize).
- **Two routes from the walk, one format.** The harness writes its exact frames, and a smoothed route through the same cell sequence at run speed with a capped turn rate, cut where the walk teleports. Exact frames price a saturated drain and compare the engine with the walk. The smoothed route serves the vsync stutter rows and any visual read; the exact frames strobe on a visible window. Both generators stay in test code.
- **The pose file is world space.** Each pose is an eye position, not a capsule centre, with yaw and pitch in engine radians, written exact so a replay is bit-equal. A header carries the bake identity the walk records, the projection the poses were generated for, and how they were made. The first pose is the start. The format extends the `world_poses` vocabulary. It does not add a third pose dialect.
- **One pose per rendered world frame.** Pose k drives the k-th frame that draws the world, at any frame rate. The demand sequence is therefore the walk's, frame for frame. Rival: poses by elapsed time, which skips poses on slow frames, so each run would stream something different. Matching the walk's route does not reproduce its late-block counts: realtime reads race disk and frame pacing differs. The record reports the engine's counts beside the walk's and never asserts that they are equal.
- **Placement: the binary owns playback and the renderer owns the frame marks.** Playback replaces the presented pose at the seam where the frontend menu already holds its camera (`render_aim_pose`, `apply_menu_camera_pose`). It also bypasses view feel, so the render eye equals the file pose. Streaming, visibility, render and the audio listener follow that eye unchanged. Streaming reads the camera only through visibility (`research.md` §The windowed camera). Nothing streaming-specific is overridden, so the drains measured are production drains.
- **Run lifecycle.**
  - The first pose is the presented start pose, and Settling streams from it as for `--start-pose`.
  - Playback then holds the first pose for a launch-set number of frames, so a trace can attach and the settle tail drains. Hold frames are recorded, marked as hold.
  - A frame that draws no world does not advance the route. That covers a hidden window, no surface, and an early return.
  - After the last pose, playback writes the record and exits 0.
  - A level change, unload or reload during playback ends the run: the record is written and marked aborted with its reason, and the process exits non-zero.
- **Bad input stops the launch.** An empty, malformed or unreadable pose file stops the launch, and so does one whose bake identity differs from the loaded level. The error names the file and the reason. This departs from `--start-pose`, which warns and starts at the spawn, because a run that silently draws the spawn view reads as a result.
- **The cap is set at launch through the spike's slider.** The cap is the launch seed of the spike's dev-tools drain-cap slider (`development_guide.md` §6.4: a seed ships with its surface). When unset, play uses the production cap, and Settling keeps its own cap either way. The value is logged once and written to the record. If this brief builds before the spike lands the slider, it lands the runtime play-cap seam under the spike's lever decisions: with levers unset, admission is identical. Neither the cap nor playback is ever a player setting.
- **Present mode is set at launch.** Vsync defaults to on and can be turned off at launch for the run. The record names the mode. With vsync on, frame intervals show the stutter a player sees. With vsync off, they show cost (`research.md` §Frame pacing).
- **The record is a separate file, not telemetry events.** One event per frame would void the per-window rate warrant behind the telemetry sink's per-line flush (`in-progress/E20--streaming-telemetry`). The record follows the capture measurement report's shape.
  - Its header records what §12 requires of a performance run: revision, build profile and features, adapter, surface and scene extents, render divisor, window mode, present mode, cap, pose-file and bake identity, and CPU and GPU timing availability.
  - Each frame row holds the frame index, the pose index or hold, the frame interval, and the CPU split with the streaming stages.
  - Each frame row also holds admitted bytes and items per resource, whether the staging pool created a buffer (a cold drain), and drawn-not-resident counts for both resources.
  - Stages are absent, never zero, while CPU timing is off.
  - The record is sized at launch from the pose count, filled without per-frame allocation or I/O, and written once at the end.
- **Gating: `dev-tools`.** Agents are part of the developer tool set, so playback rides the feature they already build with. This departs from `capture`'s own dependency-free feature. Release and dist builds still enable no `dev-tools` (`development_guide.md` §6.4).
- **The sim keeps ticking.** The measured frame stays the production frame. The idle pawn stays at the first pose's start; measurement maps keep anything hostile away from it.
- **Frames carry marks only during playback.** Each playback frame's GPU work can be attributed to its frame index in an exported trace. Outside playback, every encoder label stays as it is today, so per-encoder reducers keep their grouping.
- **One reducer in `tools/`.** It is stdlib Python with a unittest that preflight runs. It joins the record with an MST export and reports per frame: interval, drain-encoder GPU time and cold flag. Per run it reports interval p50, p95 and max, and frames over budget, with cold drains counted apart. It follows `drafts/gpu-pass-paired-ab`'s per-encoder rules. If that reducer lands first, this one extends it rather than forking it.
- **First consumer ships with the tool:** the spike's missing rows, on the Mac perf target, measured at two or more caps.
- **Non-goals:**
  - Input record and replay: roadmap Epic 20's candidate replays input through the sim, and this brief drives no sim.
  - Offscreen or headless playback: capture owns offscreen rendering.
  - Recording a live play session into a pose file, and authored routes. The file format admits both later.
  - A telemetry kind for playback.
  - Choosing the drain cap: the spike owns that.

## Acceptance
### Automated
**Route fidelity:**
- [ ] An exact-frames file the walk harness writes for both walks at default steps reproduces every frame's pose fingerprint of the harness's in-memory walk, and two writes are byte-identical.
- [ ] A smoothed file visits the walk's cells in the walk's order. Between consecutive poses the eye moves no faster than run speed and turns no more than the turn cap, except across a marked cut where the walk teleports. Two writes are byte-identical.
- [ ] Over an N-pose file, the k-th world frame after the hold renders with an eye, yaw and pitch bit-equal to pose k, including when the followed pawn carries view feel and when it stands on a mover. Each pose is drawn exactly once.
- [ ] A frame that draws no world neither advances the route nor writes a playback row. The next world frame draws the pose that frame would have drawn.
- [ ] A one-pose file settles, holds, draws that pose once, writes the record and exits 0. A hold of zero frames starts the route on the first Running frame.

**Lifecycle and refusal:**
- [ ] An empty, malformed or unreadable file stops the launch with an error naming the file and reason, before any world frame, and so does a bake-identity mismatch. A matching file is accepted.
- [ ] A level change requested mid-route writes the record marked aborted, names the reason, and exits non-zero.
- [ ] In a build without `dev-tools`, the flag warns once naming the missing feature, and the run behaves as if it were unset.

**Cap and pacing:**
- [ ] With the cap seed set, every play drain from the first Running frame admits against the seeded cap. With it unset, drains admit against the production cap. A settling drain uses the settling budget either way. A malformed seed warns once and keeps the production cap. A build without dev-tools ignores the seed.
- [ ] The present mode chosen at launch is the one the surface runs from its first frame, and the record names it.

**Record:**
- [ ] The record holds one row per hold and route frame, in order, and its header holds every field the Decisions list.
- [ ] Stage fields are absent with CPU timing off and present with it on.
- [ ] A frame whose drain made the staging pool create a buffer is flagged cold.
- [ ] Playback frames allocate nothing (`alloc_probe`). The record is written once.
- [ ] Outside playback, every encoder label equals today's.
- [ ] The reducer's unittest summarizes a checked-in record and trace-export sample into the expected per-frame and per-run tables. A record with no matching frame marks is an error.
- [ ] Grep gate: playback code builds only behind `dev-tools`, and the pose writer lives in test code. No player setting or options UI names playback, the pose file or the drain cap.

### Manual
- [ ] Mac perf target, `--profile profiling`, campaign-test far-point routes, CPU timing on, `xctrace` attached during the hold. At the production cap and at least one other: drain GPU time per frame from the reducer on the exact frames with vsync off, and frame-interval stutter on the smoothed route with vsync on. Machine state follows §12, and cold drains are counted apart.
- [ ] The same route on the showcase copy, when the spike's frozen copy is available, recording its hash.

## Path
- **Seams:**
  - The pose writer goes beside `world_poses/bake_gap.rs` `materialise`, `poses.rs` `format_poses`/`parse_poses`, and `trip_record.rs` `pose_fingerprint`. The parser moves out of test code with the format. The walk's `bake_identity` (`baked_set.rs`) moves with it.
  - The override goes in `render_aim_pose` (`main.rs`) and the view-feel driver choice in `redraw` (`frame_loop/mod.rs`, before `assemble_frame_eye`). Start comes through `install_level_camera_pose` and `StartPose`. The capsule-to-eye offset is named in `poses.rs`.
  - Cap: the `admit_within` call in `session/level_streaming/mod.rs`. Admitted bytes per resource come from `SharedDrain` after admission.
  - CPU rows: expose `CpuFrameTimer`'s composed `FrameRecord`, test-only today, rather than re-timing.
  - Cold flag: buffer creation in `renderer/src/render/uploads/staging_pool.rs`.
  - Present mode: `Renderer::toggle_vsync` and the init-time `AutoVsync`.
- **Shape and rival:** the chosen shape is a file-driven pose override in the windowed binary. The rival is regenerating the walk from seed and map in the engine, rejected in Decisions.
- **First slice:** a pose file of a few hundred far-point frames played on campaign-test with one frame-mark mechanism, then `xctrace export`. It falsifies the riskiest assumption: that an MST export can attribute the "Streamed Lightmap Drain" and "Streamed SH Upload Copies" intervals to a playback frame index (`research.md` §Frame marks).
- **Large files:** `main.rs` and `frame_loop/mod.rs` are past 800 lines. Put playback in its own module and add only calls there.

## Open questions
- The frame-mark mechanism: a frame-indexed label on one once-per-frame encoder during playback, debug groups, or time alignment. — **delegated**: the first slice proves one on a real export.
- Per-frame GPU timestamp columns on adapters that support them. — **delegated**: optional, and absent on the Mac.
- Pose-file flag and seed names, and the hold's default length. — **delegated**.
- The smoothed route's turn cap and its run speed source. — **delegated**: the executor picks values that read as play and reports them.
