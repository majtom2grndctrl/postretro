# E20--scripted-run-capture — research

Read at ecd9489e.

## What capture draws today
- `capture/scene.rs` `CaptureScene`: one `CameraPose`, resolution, output, plus forcing knobs (`force_active`, `force_promotion`, `force_movers`, `force_missing_lightmap_blocks`, pool overrides), `light_term_mask`, reflection and ghost settings, and an optional `measurement` block (warmup and sample frames, report). No pose list, no ticks, no commands.
- `capture/prepared.rs` `PreparedCapture::prepare` builds a VM-free `EntityRegistry`: kinematic movers plus built-in classname dispatch, so `prop_mesh` exists. No `HeadlessSession`, scripting core, archetype sweep, pawn or tick. It skips `install_world_cpu`.
- Skinned props draw at rest pose: anim time 0, empty clip tables (`capture/receivers.rs` `collect_capture_receiver_draws_with_mirror`).
- Particles: none. The driver passes `&[]` to `Renderer::capture_frame_indirect`.
- Dynamic lights: stripped (`capture/setup.rs` `capture_static_lights_and_shadow_selection`; pinned by `forced_promotion_bridge_input_remains_static_only_with_mixed_authored_lights`).
- Fog volumes: the pass records, but nothing calls `upload_fog_volumes`, so no volume draws.
- Residency: `capture/lightmap.rs` `preload_capture_lightmap` reads and drains the view's blocks once; SH preloads through `preload_visible_sh` under `POSTRETRO_SH_STREAMING=sync-proof`. No further drain runs after the preload. `preload_visible_sh` draws frames through `capture_measurement_frame_indirect` until the settle set is sampleable (`capture/prepared.rs`); how many depends on what is already resident.
- Entry: `--capture <scene.json>` (`startup/session.rs` `capture_arg`), feature `capture` (enables `postretro-renderer/capture`), `cargo run -p xtask -- capture` (`crates/xtask/src/main.rs` `capture_frame`, no scripts sidecar). No live-socket capture verb (`observe_live/ingress.rs` `ObserveRequest` has `Dump` only).

## Batch runner seams
- `session/mod.rs` `HeadlessSession::build` (feature `observability`); `observability/driver.rs` `run_headless_inner`, tick loop at its step 6, `anim_time += TICK_DT` per tick.
- World install: `startup/lifecycle_world_handles.rs` `install_world_gravity_and_nav`; `startup/lifecycle_world_cpu.rs` `install_world_cpu` wraps `install_world_cpu_with_models(.., impl ModelSweep, ..)`. Headless passes a no-op model closure; windowed passes `WindowedModelSweep`. The test `LevelModels::Parse` closure (`clear_mesh_pass_for_level_load`, `upload_parsed_skinned_model`, clip-table insert) is the renderer-backed template.
- `observability/runspec.rs` `RunSpec { map, ticks, commands, dump }`, `deny_unknown_fields` throughout. `RunSpecError::LiveOnlySection` rejects a live-only section in a batch run by name: the precedent for rejecting a capture section in a build without `capture`.
- `observability/document.rs` `OutputDocument`: `player { entity, position, facing_yaw, health }`. Headless tracks no pitch (`facing_pitch: 0.0`).
- `observability = []` adds no dependency; the renderer links regardless, the batch path just never creates a device. Byte identity: `tests/headless_byte_identity.rs`.

## Windowed presentation
- `frame_loop/mod.rs` `redraw(app: &mut App, ..)` assembles every render input inline: `presentation_pool`, particle time and impact requests, `emitter_bridge.update(.., frame_dt, ..)`, `particle_sim::tick_batched(frame_dt)`, `light_bridge.update` and `absorb_dynamic_lights`, `fog_volume_bridge.tick` → `upload_fog_volumes` / `set_fog_aabbs` / `upload_fog_points`, particle and billboard-ghost collection, `resolve_pending_animation_stamps`, `mesh_render.collect_with_mirror`, `collect_viewmodel`, ghost/mover/fluid collectors, then `render_frame_indirect`. The file is past 800 lines.
- Collector state lives on the windowed `Session`; `HeadlessSession` has only a `FogVolumeBridge`.
- `renderer_capture.rs` `capture_frame_indirect` records the same `record_scene_passes` as `render_frame_indirect` and already takes `particle_collections`; meshes, lights and fog are renderer state set beforehand, so those need no new renderer input. The viewmodel does: offscreen capture returns after fog and bloom, before the first-person pass (`renderer_render_frame.rs`).
- Particles and emitters advance per presentation frame, never per sim tick. Emitter seeding is deterministic (`emitter_bridge.rs` `emitter_seed`, a per-entity step counter).
- Fog has no temporal history: ray-start jitter in `fog_volume.wgsl` is per pixel, stationary. `rendering_pipeline.md` §7.5 still names a `frame_index` field that `FogParams` no longer has.
- Mesh pose sampling is a pure function of `anim_time` and the entity's stamps (`mesh_render.rs` `resolve_sample`); distant instances time-slice on the collector's `frame_index`. Windowed collection draws at the frame's sub-tick alpha (`EntityRegistry::interpolated_transform`); the pawn eye reads the current transform.
- Dynamic lights need the bridge populated at install (`startup/lifecycle_install.rs` `install_level_light_bridge`, `start_installed_presentation_animations`) and `absorb_dynamic_lights` after each tick. Install spawns one registry entity per map light, before fog, shifting later ids; headless skips it. `LightBridge::update` absorbs lights spawned since the last enrollment, frames without a tick included, so the reveal frame draws a light a `levelLoad` reaction spawned during install.
- Presentation writes registry state the dump serializes and the next tick reads: `LightBridge::update` commits settled light components, `EmitterBridge::update` consumes `burst` and settles spin, `FogVolumeBridge::tick` writes sampled fog animation, and `resolve_pending_animation_stamps` fills `entered_at`, which `switch_animation_state` and locomotion read. A capture run therefore dumps what play would, not what the batch run dumps.
- Camera: `main.rs` `follow_camera_to_local_pawn` → `pawn_eye_position` (transform + capsule eye height), private to `main.rs`. `startup/presented_pose.rs` `PresentedPose::held_view` builds a view with no view-feel offset.
- Viewmodel: `mesh_render.rs` `collect_viewmodel` appends a world-space instance; `main.rs` `viewmodel_world_transform` composes the view matrix with view-feel offsets and the weapon's placement. With a held view the offsets are zero.

## Commitments touched
- `plans/done/E20--frame-capture`: same-adapter byte identity; `capture` its own dependency-free feature; the follow-up "reuses the shipped batch runspec"; entities and fog deferred to it.
- `plans/done/agentic-observability`: the batch path needs no adapter and is byte-identical; the runspec is the shared vocabulary.
- `plans/done/cpu-frame-profiling` and `sh-probe-streaming--measurement-harness`: per-tick capture and camera paths belong to this plan, not to static capture.
- Roadmap Epic 20 non-goal: external state writes that fight sim authority.
- `in-progress/E10--ai-tick-scheduling/research.md`: AI LOD must not read the renderer's visible set: a batch run has no renderer, and a capture run's sim must not depend on what its captures view.

## Consumers and doors
- `plans/done/E21--bone-sockets-attachments` deferred its captured evidence here.
- `plans/done/emissive-surfaces-bloom` deferred a rendered-frame golden harness, not evidence: capture already draws bloom. This plan ships same-adapter byte identity only.
- `drafts/E20--memory-visibility` blocks on whether an offscreen session host exists.
- `drafts/camera-path-playback` owns per-frame streaming and frame timing along a route; this plan preloads per capture (isolated) or streams synchronously every tick (continuous), and times nothing.
- Renderer target inspection (roadmap, undrafted): no technical dependency either way.

## Pins
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Capture at tick 0; a `levelLoad` reaction spawned a light during install | Install, no tick, no presentation time advance; the light bridge absorbs install-time lights | Spawn pose; no particles; the `levelLoad` light draws, as on the revealed frame |
| P2 | Two captures on one tick, same pose | Both after that tick's presentation step | Identical bytes; the dump is unchanged |
| P3 | Capture at tick k, then a later capture at a pose in another area of a streamed level | Each preloads its own view | Each equals a single-capture run at its pose |
| P4 | Capture-bearing runspec in a build without `capture` | Parse, before map load | Fails, naming the section and the feature |
| P5 | Player-pose capture with no local pawn | At the capture tick | Fails, naming the capture; an override capture at the same tick succeeds |
| P6 | Viewmodel on, no weapon equipped | At the capture tick | Equal bytes to viewmodel off |
| P7 | Promotable light near a receiver; first capture, then a later capture at another pose | No uncaptured frame between captures (SH preload frames aside, P16); the renderer's crossfade steps from the last drawn frame (`renderer_light_slots.rs`, elapsed clamped to 0.25 s, first frame 1/60 s) | Both captures draw full weight; neither depends on the other |
| P8 | Two capture runs of one runspec; the fixture has map lights, a completing light animation, an emitter burst and spin tween, an animated fog volume and an entity that changes animation state | Presentation step after every tick writes back before the next tick reads the registry | Dump bytes and entity ids equal across runs |
| P9 | Isolated: promotable light reaches the first capture's view, not a later capture's view under 0.5 s of run time later | Promote-whole steps the slot lifecycle by 0.3 s, less than the 0.5 s sticky hold; last frame's seats rank as incumbents | Later capture shows no trace of the light and seats no slot for it; equals a single-capture run at its pose |
| P10 | Isolated: two captures on one tick at different poses; the first sees a fog volume and a planar reflector outside the second's cells | Same run time for both, so no fog hysteresis window elapses; the held planar plane persists by score margin | Second capture draws no fog or reflection selected only by the first view; equals a single-capture run at its pose |
| P11 | Isolated: far-bucket skinned entity captured at tick k, then at tick k+j where its stride phase skips the resample | No uncaptured frame between captures; the palette cache holds the pose the first capture uploaded; capture-time collection may advance the stride phase | Later capture shows the tick-(k+j) pose; equals a single-capture run at that tick and pose |
| P12 | Command or script effect on tick k; captures at ticks k and k+1; a capture at tick `ticks`; a `ticks: 0` run | Tick k is loop iteration k; capture tick N follows N iterations | Tick k shows the state before the effect, tick k+1 after; tick `ticks` shows the dumped state; `ticks: 0` with a tick-0 capture succeeds; tick `ticks + 1` fails parsing |
| P13 | Player-pose capture at tick N with an aim authored at tick N; at tick 0 with an aim at tick 0; on the tick a teleport turns the pawn | An aim on tick N applies in iteration N, after capture N | Yaw is the pawn's transform yaw after N ticks, teleport turn included; pitch is the last aim among commands before tick N; at tick 0, spawn yaw and pitch 0, whatever the dump's `facing_yaw` says |
| P14 | Mover or NPC moving through tick N; capture at tick N | Windowed play draws between tick N−1 and N transforms at a sub-tick alpha | Every entity draws at its tick-N transform (alpha 1), the instant the pawn eye and the dump report |
| P15 | Burst emitter fires on its first update; particles live L ticks; captures before tick L and after tick L+1 | Step advances after every tick from tick 1; no catch-up advance at a capture | Billboards before tick L; none after tick L+1 |
| P16 | Isolated: capture on a streamed-SH level after an earlier capture at another pose | The SH preload draws its frames, then the reset applies to the captured frame, never to a preload frame | Captured bytes equal a single-capture run at that pose, whatever number of preload frames ran |
| P17 | Capture A writes at tick j; capture B fails at tick k > j (e.g. player pose, no local pawn) | B fails at its tick; no later capture runs | Run exits non-zero naming B; A's PNG stays on disk |
| P18 | Continuous: a promotable map light comes into view by an aim on tick k; captures at k, 0.1 s after and 1 s after | A frame drawn every tick steps the 0.3 s crossfade by 1/60 s; script-spawned lights are dynamic and never fade, so the light is map-authored | About a third of full weight at 0.1 s; full at 1 s; two preloaded runs give equal bytes |
| P19 | Continuous tick-0 capture | Install, then the revealed frame: settled residency, promotions whole | Equals the isolated tick-0 capture at that pose |
| P20 | Same runspec across modes, residency options and capture lists | Presentation writes are the same every tick whatever the renderer draws | Equal dumps; the sim never reads what captures view |

## Renderer timed state
- `renderer_light_slots.rs`: `PROMOTE_SECONDS` 0.3 s crossfade, `STICKY_SECONDS` 0.5 s, demote 0.3 s, stepped by elapsed time since the last drawn frame. `start_next_promotions_whole` only sets a flag that enlarges one frame's step to 0.3 s; incumbents, sticky holds and demote fades carry across it. Only Settling calls it (`startup/settling.rs`); static capture never does. A reveal frame is history-free because install leaves no incumbents.
- `fog_pass.rs` `FOG_HYSTERESIS_SECONDS` 0.3 s against run time, so captures on one tick share `now`. `FogPass::clear_for_level_load` resets it, reached today only through level load.
- Planar selection is app-side: a held plane on `App.planar_render` (`frame_loop/planar.rs`), kept by a score margin against the previous selection (`render-cpu` `planar_reflection/selection.rs`), not a timer. Static capture builds a fresh selection per capture.
- Skinned palettes: mid and far instances resample on the collector's `frame_index` stride (`mesh_render.rs` `should_resample`, which takes a `force` input); a skip reuses the renderer's palette cached from the last drawn frame (`mesh_pass/pose.rs`).
- Without a reset, a first capture draws a promoted light at about 0.055 weight; a later capture after a 0.25 s step at about 0.83. In continuous mode the crossfade steps 1/60 s per tick and reaches full weight after 0.3 s.
