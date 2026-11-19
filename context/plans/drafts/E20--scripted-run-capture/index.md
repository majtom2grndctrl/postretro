# E20--scripted-run-capture

Brief · compact · Epic 20 · reads: `context/lib/rendering_pipeline.md` §7.5, §7.8 · `context/lib/development_guide.md` §6.4 · `context/lib/boot_sequence.md` §3 · research: `research.md` · read at ecd9489e

## Problem
Agents and the owner cannot get a picture of what play looks like after something happens. Frame capture draws one static pose of a level as installed, with no script VM and no tick. It draws no particles, no dynamic lights and no fog volumes, and skinned props stand at rest pose. The batch runner ticks the sim through scripted commands but never renders. Briefs that needed rendered evidence of animated or scripted content, such as bone-socket attachments, deferred it to this plan. When this is done, a runspec can ask for captures at chosen ticks. The run ticks the sim and presents every tick as windowed play does, and each capture tick writes a PNG from the player's camera or an authored pose. The PNG includes animated skinned meshes, particles, dynamic lights and fog volumes, and optionally the viewmodel. The runspec chooses, per run, whether each capture stands alone or carries the renderer's history from the run.

## Decisions
- **Capture is a section of the batch runspec.** One vocabulary, as `plans/done/E20--frame-capture` and `plans/done/agentic-observability` committed. A runspec without the section runs exactly as today: no adapter, byte-identical output. A runspec with it needs the `capture` feature and an adapter. A build without `capture` rejects it by name before loading the map, as batch runs reject live-only sections. The run still writes its dump, and the dump names that it came from a capture run.
- **Static capture's forcing knobs and measurement block stay on the scene spec.** Forcing a mover's phase or a light's radiance writes state the sim owns (Epic 20 non-goal: external writes that fight sim authority). Static capture keeps its own VM-free path; moving it onto the shared presentation step is not owed here.
- **One presentation step, shared with windowed play.** The render-input assembly inside the windowed redraw becomes a host-agnostic step that windowed play and scripted capture both call. Its state is one bundle both sessions hold, as `plans/done/agentic-observability` shared scripting-core construction so the two cannot drift. A second copy, as static capture's receiver collection already is, would drift from what players see.
- **Presentation advances every tick.** After each sim tick, the step advances by that tick's duration and draws inputs at the tick's end state, so particles, emitters, light and fog animation and mesh animation reach a capture tick as a 60 Hz windowed run would. Capture tick N shows the state after N ticks; tick 0 shows the installed level, as its revealed frame would.
- **A capture run is play without a window; there is no pause.** Presentation writes what it writes in play, including state the next tick reads: animation entry stamps, settled lights and fog, emitter bursts. A capture run's dump is therefore byte-identical across capture runs, and may differ from the same runspec's batch dump. It never depends on what the captures view, which mode runs, or where captures fall: `E10--ai-tick-scheduling` relies on a sim blind to the renderer. The engine has no real pause that would hold the sim still while presentation runs, and building one is not owed here. Moving those writes into the tick is not owed either.
- **Two renderer modes, chosen per run.**
  - *Isolated* (default): before each captured frame, after its residency preload, view-dependent render state returns to a fresh level's: light-slot incumbents and their fades and holds, fog hysteresis, planar selection, the skinned-pose cache. Promotions then start whole. The renderer owns one "drop view history" mechanism for its part and the app clears its planar choice; the capture run owns when to call them. Each capture equals a single-capture run at its pose and tick.
  - *Continuous*: the renderer draws a frame every tick from the player's held view, so fades, hysteresis and slot history accumulate as in play. A player-pose capture reads back that tick's own frame and adds no draw. An override-pose capture is an extra draw on its tick, and it counts as history. A continuous run with no local pawn fails, naming the mode.
- **Capture list.** Each capture names a tick, an output path and a resolution, plus an optional pose and viewmodel flag. Several captures may share a tick. Ticks run in order, as commands do. A tick past the run's end, a list out of tick order, a repeated output path or an empty list fails parsing by field name. A capture that fails at its tick ends the run non-zero, naming it; PNGs already written stay.
- **Pose: the player's held view, or an override.** By default the camera sits at the local pawn's eye, at its facing yaw and the last applied aim's pitch (0 before any aim), at the windowed default 100° horizontal field of view, with no view feel: no bob, roll, sway or shake. An override uses static capture's pose fields and units. A player-pose capture with no local pawn fails, naming the capture.
- **Viewmodel is opt-in per capture, off by default.** On, capture records the first-person pass that offscreen capture skips today, with the equipped weapon at its authored placement for the held view. With no weapon equipped, on and off give the same bytes. UI, screen effects and overlays stay out, per §7.8.
- **Residency.**
  - Isolated mode: each capture preloads its own view, so its bytes do not depend on what earlier captures left in the pools.
  - Continuous mode, a per-run option: *settled* (default) settles each new view synchronously, lightmaps and SH, before drawing it, on any bake; the settle frames count as history. *Streamed* streams every tick as play does, for maps too large to settle view by view.
  - The runspec's choice governs the run. The streaming environment variables are ignored in a capture run, with one warning naming each that is set.
  - Streaming cost and frame timing are not measured here; `drafts/camera-path-playback` owns them.
- **Determinism: same-adapter byte identity per PNG**, as frame capture set it, in isolated mode and in settled continuous mode. Streamed continuous mode races background reads and promises no identity; its captures are for looking, not diffing.
- **Gating.** Rides the existing `capture` feature: no new feature, no dependency (`development_guide.md` §6.4). Not a player or modder surface.
- **No ordering against renderer target inspection.** The roadmap's sequencing line goes at landing.
- **Not this brief:**
  - A live-socket capture verb: the live channel runs no scripted tick loop (`plans/done/E20--live-socket-channel`).
  - Record/replay; multiple local or remote players; audio.
  - Cross-adapter goldens: frame capture's same-adapter boundary stands.

## Acceptance

### Automated
- [ ] A runspec with captures at two ticks, run twice in fresh processes on one adapter, writes byte-identical PNGs per capture.
- [ ] Two capture runs of one runspec write byte-identical, unfiltered dumps. The fixture carries map-authored lights, a light animation whose play count completes within the run, an emitter burst and spin tween, an animated fog volume and an animated entity that changes state (pin P8).
- [ ] On one machine, a capture-free runspec on a fixture with map-authored lights runs with no adapter, exits 0, and writes the same dump bytes from this branch's binary as from the base commit's.
- [ ] In a build without `capture`, a capture-bearing runspec fails before map load, naming the section and the feature (pin P4). The existing batch byte-identity test passes unchanged.
- [ ] A capture list out of tick order, a repeated output path, an empty capture list, a capture tick past `ticks`, and a capture entry carrying a scene-spec field (`force_active`, `force_promotion`, `force_movers`, `measurement`) each fail parsing, naming the field. A capture at tick `ticks`, the dumped state, succeeds, as does a tick-0 capture in a `ticks: 0` run (pin P12).
- [ ] One runspec run in isolated mode, in settled continuous mode, in streamed continuous mode, and with its capture list changed to other ticks and poses, writes the same dump bytes each time. The dump names that it came from a capture run (pin P20).
- [ ] A run whose second capture fails at its tick exits non-zero, naming that capture; the first capture's PNG stays on disk (pin P17).
- [ ] A capture at tick 0 draws the installed level as its revealed frame would: entities at spawn pose, no particles, and a light that a `levelLoad` reaction spawned during install (pin P1).
- [ ] An emitter that spawns from its first update shows billboard pixels in a capture after it has run, and none at tick 0. A burst emitter whose particles live L ticks shows billboard pixels in a capture before tick L and none in one after tick L+1 (pin P15).
- [ ] A dynamic light the fixture spawns on tick k lights a receiver in a capture at tick k+1, and not in one at tick k. A fog volume enabled on tick k draws at tick k+1, and not at tick k (pin P12).
- [ ] A capture at tick N draws a moving mover at the position a tick-N dump reports, not its tick N−1 position (pin P14).
- [ ] An animated skinned entity captured at two ticks of its clip shows two poses; the same tick in two runs gives the same bytes.
- [ ] After commands move and aim the pawn, a player-pose capture at tick `ticks` equals an override capture at the dumped position plus the pawn's eye height, the facing yaw and the aim pitch, at the override's default field of view. The fixture's aim converts exactly to and from degrees. With no aim command, the player pose's pitch is 0.
- [ ] A player-pose capture at tick N with an aim authored at tick N equals the same capture with that aim moved to tick N+1. At tick 0 the player pose is the spawn facing at pitch 0, even when tick 0 carries an aim (pin P13).
- [ ] A player-pose capture with no local pawn fails, naming the capture; an override capture at the same tick succeeds (pin P5).
- [ ] Two captures on one tick with the same pose give identical bytes (pin P2).
- [ ] With a weapon equipped, viewmodel on draws weapon pixels that viewmodel off does not. With no weapon equipped, the two give equal bytes (pin P6).

**Isolated mode, the default: these runspecs name no mode, and each capture equals a single-capture run at its pose and tick.**
- [ ] On a streamed level, a run with captures at poses in two distant areas writes, for each, the bytes a single-capture run at that pose writes, however many frames each capture's SH preload drew (pins P3, P16).
- [ ] A promotable light reaching a skinned receiver draws at full promotion weight in the run's first capture and in a later capture at a different pose. Each equals a single-capture run at its pose (pin P7).
- [ ] A promotable light that the run's first capture promotes, and that a capture under 0.5 s of run time later at another pose does not reach, draws in neither that later capture nor a single-capture run at its pose; the two give equal bytes (pin P9).
- [ ] Two captures on one tick at different poses, the first seeing a fog volume and a planar reflector the second's view excludes, each give the bytes a single-capture run at that pose gives (pin P10).
- [ ] A skinned entity animating beyond the far resample distance, captured at two ticks in one run, gives at the second tick the bytes a single-capture run at that tick gives (pin P11).

**Continuous mode.**
- [ ] A promotable map light with a skinned receiver in its reach, which an aim on tick k turns into the held view, shows in player-pose captures a receiver shadow absent at k, partial 0.1 s after k, and full 1 s after k: the 0.1 s capture differs from the tick-k capture in the shadow region by less than the 1 s capture does. Two settled continuous runs give identical bytes per capture (pin P18).
- [ ] In a continuous run, a player-pose capture on tick k gives the same bytes with an override capture on tick k listed before it, after it, or absent (pin P21).
- [ ] A continuous tick-0 capture equals an isolated tick-0 capture at the same pose (pin P19).
- [ ] In a continuous run, a player-pose capture at tick 60 gives the same bytes whether or not the run also takes player-pose captures at tick 0 and tick 30.
- [ ] A continuous run with no local pawn fails, naming the mode.
- [ ] In a settled continuous run on a streamed level, the first frame whose held view enters a new area, and an override capture in an area the held view never reached, each draw with no lightmap block or SH cluster missing. The same runspec with streamed residency runs to the end and writes every PNG.
- [ ] With `POSTRETRO_SH_STREAMING` and `POSTRETRO_LIGHTMAP_STREAMING` set to values that disagree with the runspec, a capture run follows the runspec, warns once per variable, and writes the bytes it writes with neither set.

**Structure and proof.**
- [ ] Windowed redraw and the scripted capture run reach emitter, particle, light, fog and mesh collection through the one presentation step. A grep gate finds no per-frame caller of those collectors outside the step and static capture.
- [ ] Existing static capture tests pass with byte-identical output, and windowed frame-loop tests pass, after the presentation step is extracted.
- [ ] Every row above that reads a capture runs in a pass with `POSTRETRO_REQUIRE_GPU` set, and fails rather than skips with no adapter.

### Manual
- [ ] Owner visual read on the fixture run: each capture beside a windowed screenshot at about the same tick and pose. Particles, dynamic lights, fog and animation look as they do in play.
- [ ] The bone-socket attachment capture that `E21--bone-sockets-attachments` deferred is taken and attached to the PR.

## Path
- **Seams:** `frame_loop/mod.rs` `redraw` (past ~800 lines: split first, behavior-preserving, own commit); collectors in `scripting/frame_systems/` (`MeshRenderCollector`, `EmitterBridge`, `LightBridge`, `FogVolumeBridge`, particle render); `observability/driver.rs` `run_headless_inner`; `session/mod.rs` `HeadlessSession`, which gains the windowed `Session`'s presentation state only in a capture run; `install_world_cpu_with_models` with a renderer-backed model sweep; `Renderer::new_offscreen` and `capture_frame_indirect`; capture's `preload_capture_lightmap` and `preload_visible_sh`, which must run more than once per process.
- **Pose:** lift `pawn_eye_position` and the pawn lookup out of `main.rs`; `PresentedPose::held_view` for the matrix. Pitch comes from the last aim applied before the capture tick; `effective_aim_at` also returns the tick's own entries, which have not turned the pawn yet.
- **Viewmodel:** `collect_viewmodel` with `viewmodel_world_transform` at zero view-feel offsets. Offscreen capture returns before the first-person pass (`renderer_render_frame.rs`, the early return on no `swapchain_view`), so capture needs a way to record that pass when asked, without the debug, UI or resolve tail.
- **Chosen shape:** the batch driver plus an offscreen renderer and the shared step. **Strongest rival:** a windowless `App`: drive the real windowed frame loop with no surface and a tick-sized frame, captures as a frame hook, so play parity holds by construction. Rejected: the runspec's commands and tick loop live in the batch driver, and `App` is bound to winit, input, audio and net. Its lesson is taken: one presentation-state bundle both sessions hold.
- **Isolated reset:** `Renderer::start_next_promotions_whole` (today only `startup/settling.rs`) only enlarges one frame's crossfade step; incumbents, sticky holds and demote fades survive it, so isolation needs a full light-slot reset. Fog has `FogPass::clear_for_level_load`, reached today only through level load. Planar selection is app-side (`App.planar_render`, `frame_loop/planar.rs`) and holds by score margin, not a timer: clear the held plane. A time-slice skip reuses the renderer's cached skinned palette (`mesh_pass/pose.rs`); the collector's `force` input bypasses the stride. `preload_visible_sh` draws frames, so the reset follows the last preload frame.
- **Residency:** `HeadlessSession` gains the SH and lightmap residency sessions in a capture run. The lightmap mode is chosen in-process today only through a test path; lift it to a loader entry the runspec drives. SH `sync-proof` settles a view and promotes a cluster the frame after compose (`sh_streaming/`); a settled view needs those frames drawn before its own. `preload_capture_lightmap` refuses once streaming has started, so a mid-run lightmap settle is a new seam.
- **First slice:** the shared step extracted, then a capture-bearing runspec on a fixture with one emitter, one dynamic light, one fog volume and one promotable light near a skinned mesh, captured at tick 0 and a later tick. It tests the riskiest assumption: that one presentation advance per tick reproduces play's particles and lights.
- **Fixture:** a `content/dev` map and runspec. A map script can enable the light and the fog at known ticks; runspec commands carry only movement, aim, fire and reload.
- **xtask:** the headless verb already builds the scripts sidecar; the capture run needs it too.
- **Docs at landing:** `rendering_pipeline.md` §7.8 gains the scripted run; §7.5 drops the stale `frame_index` field; the roadmap's Epic 20 entry drops its sequencing line.

## Open questions
- Fixture map: a new `content/dev` map, or an existing one such as `anim-demo` or `combat-demo` extended — **delegated**.
- Whether the run takes the existing `--headless` flag or its own — **delegated**: the executor keeps one runspec parser and reports the choice.
