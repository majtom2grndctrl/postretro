# E20--scripted-run-capture

Brief · compact · Epic 20 · reads: `context/lib/rendering_pipeline.md` §7.5, §7.8 · `context/lib/development_guide.md` §6.4 · `context/lib/boot_sequence.md` §3 · research: `research.md` · read at ecd9489e

## Problem
Agents and the owner cannot get a picture of what play looks like after something happens. Frame capture draws one static pose of a level as installed, with no script VM and no tick. It draws no particles, no dynamic lights and no fog volumes, and skinned props stand at rest pose. The batch runner ticks the sim through scripted commands but never renders. Briefs that needed rendered evidence of animated or scripted content, such as bone-socket attachments, deferred it to this plan. When this is done, a runspec can ask for captures at chosen ticks. The run ticks the sim and presents every tick as windowed play does, and each capture tick writes a PNG from the player's camera or an authored pose. The PNG includes animated skinned meshes, particles, dynamic lights and fog volumes, and optionally the viewmodel. The runspec chooses whether each capture stands alone or carries the renderer's history from the run.

## Decisions
- **Capture is a section of the batch runspec.** One vocabulary, as `plans/done/E20--frame-capture` and `plans/done/agentic-observability` committed. A runspec without the section runs exactly as today: no adapter, byte-identical output. A runspec with it needs the `capture` feature and an adapter. A build without `capture` rejects it by name before loading the map, as batch runs reject live-only sections. The run still writes its dump.
- **Static capture's forcing knobs and measurement block stay on the scene spec.** Forcing a mover's phase or a light's radiance writes state the sim owns (Epic 20 non-goal: external writes that fight sim authority). Static capture keeps its own VM-free path; moving it onto the shared presentation step is not owed here.
- **One presentation step, shared with windowed play.** The render-input assembly inside the windowed redraw becomes a host-agnostic step that windowed play and scripted capture both call. A second copy, as static capture's receiver collection already is, would drift from what players see.
- **Presentation advances every tick.** After each sim tick, the step advances by that tick's duration and draws inputs at the tick's end state, so particles, emitters, light and fog animation and mesh animation reach a capture tick as a 60 Hz windowed run would. Capture tick N shows the state after N ticks; tick 0 shows the installed level, as its revealed frame would.
- **A capture run is play without a window; there is no pause.** Presentation writes what it writes in play, including state the next tick reads: animation entry stamps, settled lights and fog, emitter bursts. A capture run's dump is therefore byte-identical across capture runs, and may differ from the same runspec's batch dump. The engine has no real pause that would hold the sim still while presentation runs, and building one is not owed here. Moving those writes into the tick is not owed either.
- **Two renderer modes, chosen per run.**
  - *Isolated* (default): before each captured frame, after its residency preload, view-dependent renderer state returns to a fresh level's: light-slot incumbents and their fades and holds, fog hysteresis, planar selection, the skinned-pose cache. Promotions then start whole. Each capture equals a single-capture run at its pose and tick.
  - *Continuous*: the renderer draws an uncaptured frame every tick from the player's held view, and streams as play does. Fades, hysteresis and residency accumulate as in play, and each capture reads back on its tick, carrying the run's history.
- **Capture list.** Each capture names a tick, an output path and a resolution, plus an optional pose and viewmodel flag. Several captures may share a tick. Ticks run in order, as commands do. A tick past the run's end, a list out of tick order, a repeated output path or an empty list fails parsing by field name. A capture that fails at its tick ends the run non-zero, naming it; PNGs already written stay.
- **Pose: the player's held view, or an override.** By default the camera sits at the local pawn's eye, at its facing yaw and the last applied aim's pitch (0 before any aim), with no view feel: no bob, roll, sway or shake. An override uses static capture's pose fields and units. A player-pose capture with no local pawn fails, naming the capture.
- **Viewmodel is opt-in per capture, off by default.** On, capture records the first-person pass that offscreen capture skips today, with the equipped weapon at its authored placement for the held view. With no weapon equipped, on and off give the same bytes. UI, screen effects and overlays stay out, per §7.8.
- **Residency.** In isolated mode each capture preloads its own view as static capture does, so its bytes do not depend on what earlier captures left in the pools. In continuous mode streaming runs every tick and completes synchronously, so runs repeat. Streaming cost and frame timing are not measured here; `drafts/camera-path-playback` owns them.
- **Determinism: same-adapter byte identity per PNG**, in both modes, as frame capture set it.
- **Gating.** Rides the existing `capture` feature: no new feature, no dependency (`development_guide.md` §6.4). Not a player or modder surface.
- **No ordering against renderer target inspection.** The roadmap's sequencing line goes at landing.
- **Not this brief:**
  - A live-socket capture verb: the live channel runs no scripted tick loop (`plans/done/E20--live-socket-channel`).
  - Record/replay; multiple local or remote players; audio.
  - Cross-adapter goldens: frame capture's same-adapter boundary stands.

## Acceptance

### Automated
- [ ] A runspec with captures at two ticks, run twice in fresh processes on one adapter, writes byte-identical PNGs per capture.
- [ ] The same runspec with its capture section removed writes a byte-identical dump to the capture run's. With no adapter, it still runs and exits 0.
- [ ] In a build without `capture`, a capture-bearing runspec fails before map load, naming the section and the feature (pin P4). The existing batch byte-identity test passes unchanged.
- [ ] A capture tick past the run's end, a repeated output path and an empty capture list each fail parsing, naming the field. A capture at the run's last tick succeeds.
- [ ] A capture at tick 0 draws the installed level: entities at spawn pose, no particles, no runtime-spawned light (pin P1).
- [ ] An emitter that spawns from its first update shows billboard pixels in a capture after it has run, and none at tick 0.
- [ ] A dynamic light the fixture spawns at a known tick lights a receiver in a capture after that tick, and not in one before it.
- [ ] A fog volume the fixture enables at a known tick draws in a capture after that tick, and not in one before it.
- [ ] An animated skinned entity captured at two ticks of its clip shows two poses; the same tick in two runs gives the same bytes.
- [ ] After commands move and aim the pawn, a player-pose capture equals an override capture at the pawn's eye, facing yaw and aim pitch. With no aim command, the player pose's pitch is 0.
- [ ] A player-pose capture with no local pawn fails, naming the capture; an override capture at the same tick succeeds (pin P5).
- [ ] Two captures on one tick with the same pose give identical bytes (pin P2).
- [ ] On a streamed level, a run with captures at poses in two distant areas writes, for each, the bytes a single-capture run at that pose writes (pin P3).
- [ ] A promotable light reaching a skinned receiver draws at full promotion weight in the run's first capture and in a later capture at a different pose. Each equals a single-capture run at its pose (pin P7).
- [ ] With a weapon equipped, viewmodel on draws weapon pixels that viewmodel off does not. With no weapon equipped, the two give equal bytes (pin P6).
- [ ] Existing static capture tests pass with byte-identical output, and windowed frame-loop tests pass, after the presentation step is extracted.

### Manual
- [ ] Owner visual read on the fixture run: each capture beside a windowed screenshot at about the same tick and pose. Particles, dynamic lights, fog and animation look as they do in play.
- [ ] The bone-socket attachment capture that `E21--bone-sockets-attachments` deferred is taken and attached to the PR.

## Path
- **Seams:** `frame_loop/mod.rs` `redraw` (split first, behavior-preserving, own commit; 2957 lines); collectors in `scripting/frame_systems/` (`MeshRenderCollector`, `EmitterBridge`, `LightBridge`, `FogVolumeBridge`, particle render); `observability/driver.rs` `run_headless_inner`; `session/mod.rs` `HeadlessSession`, which gains the windowed `Session`'s presentation state only in a capture run; `install_world_cpu_with_models` with a renderer-backed model sweep; `Renderer::new_offscreen` and `capture_frame_indirect`; capture's `preload_capture_lightmap` and `preload_visible_sh`, which must run more than once per process.
- **Pose:** lift `pawn_eye_position` and the pawn lookup out of `main.rs`; `PresentedPose::held_view` for the matrix. Pitch comes from the last aim command's direction.
- **Viewmodel:** `collect_viewmodel` with `viewmodel_world_transform` at zero view-feel offsets.
- **Chosen shape:** the batch driver plus an offscreen renderer and the shared step. **Strongest rival:** extend static capture with a tick loop. Rejected: static capture is VM-free by contract, and its forcing knobs fight the sim.
- **Settling:** `Renderer::start_next_promotions_whole`, called today only from `startup/settling.rs`; fog and planar hysteresis need an equivalent reset.
- **First slice:** the shared step extracted, then a capture-bearing runspec on a fixture with one emitter, one dynamic light, one fog volume and one promotable light near a skinned mesh, captured at tick 0 and a later tick. It tests the riskiest assumption: that one presentation advance per tick reproduces play's particles and lights.
- **Fixture:** a `content/dev` map and runspec. A map script can enable the light and the fog at known ticks; runspec commands carry only movement, aim, fire and reload.
- **xtask:** the headless verb already builds the scripts sidecar; the capture run needs it too.
- **Docs at landing:** `rendering_pipeline.md` §7.8 gains the scripted run; §7.5 drops the stale `frame_index` field; the roadmap's Epic 20 entry drops its sequencing line.

## Open questions
- Fixture map: a new `content/dev` map, or an existing one such as `anim-demo` or `combat-demo` extended — **delegated**.
- Whether the run takes the existing `--headless` flag or its own — **delegated**: the executor keeps one runspec parser and reports the choice.
