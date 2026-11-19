# E20--scripted-run-capture — research

Read at ecd9489e. Replaces the pre-draft note written while `E20--frame-capture` was unbuilt; that note's line references and its "world geometry only" description of capture are stale.

## What capture draws today
- `capture/scene.rs` `CaptureScene`: one `CameraPose`, resolution, output, plus forcing knobs (`force_active`, `force_promotion`, `force_movers`, `force_missing_lightmap_blocks`, pool overrides), `light_term_mask`, reflection and ghost settings, and an optional `measurement` block (warmup and sample frames, report). No pose list, no ticks, no commands.
- `capture/prepared.rs` `PreparedCapture::prepare` builds a VM-free `EntityRegistry`: kinematic movers plus built-in classname dispatch, so `prop_mesh` exists. No `HeadlessSession`, scripting core, archetype sweep, pawn or tick. It skips `install_world_cpu`.
- Skinned props draw at rest pose: anim time 0, empty clip tables (`capture/receivers.rs` `collect_capture_receiver_draws_with_mirror`).
- Particles: none. The driver passes `&[]` to `Renderer::capture_frame_indirect`.
- Dynamic lights: stripped (`capture/setup.rs` `capture_static_lights_and_shadow_selection`; pinned by `forced_promotion_bridge_input_remains_static_only_with_mixed_authored_lights`).
- Fog volumes: the pass records, but nothing calls `upload_fog_volumes`, so no volume draws.
- Residency: `capture/lightmap.rs` `preload_capture_lightmap` reads and drains the view's blocks once; SH preloads through `preload_visible_sh` under `POSTRETRO_SH_STREAMING=sync-proof`. No further drain runs after the preload.
- Entry: `--capture <scene.json>` (`startup/session.rs` `capture_arg`), feature `capture` (enables `postretro-renderer/capture`), `cargo run -p xtask -- capture` (`crates/xtask/src/main.rs` `capture_frame`, no scripts sidecar). No live-socket capture verb (`observe_live/ingress.rs` `ObserveRequest` has `Dump` only).

## Batch runner seams
- `session/mod.rs` `HeadlessSession::build` (feature `observability`); `observability/driver.rs` `run_headless_inner`, tick loop at its step 6, `anim_time += TICK_DT` per tick.
- World install: `startup/lifecycle_world_handles.rs` `install_world_gravity_and_nav`; `startup/lifecycle_world_cpu.rs` `install_world_cpu` wraps `install_world_cpu_with_models(.., impl ModelSweep, ..)`. Headless passes a no-op model closure; windowed passes `WindowedModelSweep`. The test `LevelModels::Parse` closure (`clear_mesh_pass_for_level_load`, `upload_parsed_skinned_model`, clip-table insert) is the renderer-backed template.
- `observability/runspec.rs` `RunSpec { map, ticks, commands, dump }`, `deny_unknown_fields` throughout. `RunSpecError::LiveOnlySection` rejects a live-only section in a batch run by name: the precedent for rejecting a capture section in a build without `capture`.
- `observability/document.rs` `OutputDocument`: `player { entity, position, facing_yaw, health }`. Headless tracks no pitch (`facing_pitch: 0.0`).
- `observability = []` adds no dependency; the renderer links regardless, the batch path just never creates a device. Byte identity: `tests/headless_byte_identity.rs`.

## Windowed presentation
- `frame_loop/mod.rs` `redraw(app: &mut App, ..)` assembles every render input inline: `presentation_pool`, particle time and impact requests, `emitter_bridge.update(.., frame_dt, ..)`, `particle_sim::tick_batched(frame_dt)`, `light_bridge.update` and `absorb_dynamic_lights`, `fog_volume_bridge.tick` → `upload_fog_volumes` / `set_fog_aabbs` / `upload_fog_points`, particle and billboard-ghost collection, `resolve_pending_animation_stamps`, `mesh_render.collect_with_mirror`, `collect_viewmodel`, ghost/mover/fluid collectors, then `render_frame_indirect`. The file is 2957 lines.
- Collector state lives on the windowed `Session`; `HeadlessSession` has only a `FogVolumeBridge`.
- `renderer_capture.rs` `capture_frame_indirect` records the same `record_scene_passes` as `render_frame_indirect` and already takes `particle_collections`; meshes, lights and fog are renderer state set beforehand. The renderer API needs no new input.
- Particles and emitters advance per presentation frame, never per sim tick. Emitter seeding is deterministic (`emitter_bridge.rs` `emitter_seed`, a per-entity step counter).
- Fog has no temporal history: ray-start jitter in `fog_volume.wgsl` is per pixel, stationary. `rendering_pipeline.md` §7.5 still names a `frame_index` field that `FogParams` no longer has.
- Mesh pose sampling is a pure function of `anim_time` and the entity's stamps (`mesh_render.rs` `resolve_sample`); distant instances time-slice on the collector's `frame_index`.
- Dynamic lights need the bridge populated at install (`startup/lifecycle_install.rs` `install_level_light_bridge`, `start_installed_presentation_animations`) and `absorb_dynamic_lights` after each tick.
- Camera: `main.rs` `follow_camera_to_local_pawn` → `pawn_eye_position` (transform + capsule eye height), private to `main.rs`. `startup/presented_pose.rs` `PresentedPose::held_view` builds a view with no view-feel offset.
- Viewmodel: `mesh_render.rs` `collect_viewmodel` appends a world-space instance; `main.rs` `viewmodel_world_transform` composes the view matrix with view-feel offsets and the weapon's placement. With a held view the offsets are zero.

## Commitments touched
- `plans/done/E20--frame-capture`: same-adapter byte identity; `capture` its own dependency-free feature; the follow-up "reuses the shipped batch runspec"; entities and fog deferred to it.
- `plans/done/agentic-observability`: the batch path needs no adapter and is byte-identical; the runspec is the shared vocabulary.
- `plans/done/cpu-frame-profiling` and `sh-probe-streaming--measurement-harness`: per-tick capture and camera paths belong to this plan, not to static capture.
- Roadmap Epic 20 non-goal: external state writes that fight sim authority.
- `in-progress/E10--ai-tick-scheduling/research.md`: AI LOD must not read the renderer's visible set, because this plan assumes a byte-identical sim.

## Consumers and doors
- `plans/done/E21--bone-sockets-attachments` and `plans/done/emissive-surfaces-bloom` deferred their captured evidence here.
- `drafts/E20--memory-visibility` blocks on whether an offscreen session host exists.
- `drafts/camera-path-playback` owns per-frame streaming and frame timing along a route; this plan preloads per capture and times nothing.
- Renderer target inspection (roadmap, undrafted): no technical dependency either way.

## Pins
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Capture at tick 0 | Install, no tick, no presentation advance | Spawn pose; no particles; no runtime-spawned light |
| P2 | Two captures on one tick, same pose | Both after that tick's presentation step | Identical bytes; the dump is unchanged |
| P3 | Capture at tick k, then a later capture at a pose in another area of a streamed level | Each preloads its own view | Each equals a single-capture run at its pose |
| P4 | Capture-bearing runspec in a build without `capture` | Parse, before map load | Fails, naming the section and the feature |
| P5 | Player-pose capture with no local pawn | At the capture tick | Fails, naming the capture; an override capture at the same tick succeeds |
| P6 | Viewmodel on, no weapon equipped | At the capture tick | Equal bytes to viewmodel off |
| P7 | Promotable light near a receiver; first capture, then a later capture at another pose | No GPU frame between captures; the renderer's crossfade steps from the last drawn frame (`renderer_light_slots.rs`, elapsed clamped to 0.25 s, first frame 1/60 s) | Both captures draw full weight; neither depends on the other |

## Renderer timed state
- `renderer_light_slots.rs`: `PROMOTE_SECONDS` 0.3 s crossfade, sticky and demote timers, stepped by elapsed time since the last drawn frame. Only Settling calls `start_next_promotions_whole` (`startup/settling.rs`); static capture never does.
- `fog_pass.rs` `FOG_HYSTERESIS_SECONDS` 0.3 s; planar-reflection selection keeps its own hysteresis.
- Without settling, a first capture draws a promoted light at about 0.055 weight; a later capture after a 0.25 s step at about 0.83.
