# Scripted-run capture (entities + fog) — research note

**Status:** pre-draft research. NOT a spec — no AC, no task paragraphs. Captured
while context was hot after grounding the E20 frame-capture (static-pose) spec.
The E20-first seams below are **planned, not built** — re-ground them on landed
source before promoting this to a full draft. The existing-seam refs are grounded
against current source and are stable.

## What this adds beyond the static-pose floor

The shipped `E20--frame-capture` slice renders **world geometry + baked lighting
only**, at a **static pose**, one frame, no session. This follow-up adds:

- **Entities** — skinned meshes, particles/billboards, dynamic (non-baked) lights.
  Needs the scripting core, the archetype sweep (entity + player-pawn spawn), mesh
  upload, and per-frame mesh/pose/particle collection.
- **Fog volumes** — full volumetric fog including dynamic light shafts (which need
  the dynamic lights entities bring), plus the temporal-accumulation warmup the
  static slice deferred.
- **Scripted ticks then capture** — run N fixed ticks like the batch runner, then
  capture from the resulting player-camera pose (or an explicit override). The true
  "visual sibling of the state dump": one runspec, capture as a directive on it.

## Reused existing seams (grounded, stable)

The batch runner already owns the CPU/session half; fog and entities already have
windowed render paths. This follow-up merges the batch driver with a renderer.

**Batch runner / session** (`plans/done/agentic-observability/`):
- `HeadlessSession::build` (`session/mod.rs:663`), scripting-core extractor
  `build_scripting_core` (`:490`), manifest drain `drain_manifest_registrations`
  (`:593`), `run_headless` driver + tick loop (`observability/driver.rs:54`), the
  world install segment A `install_world_gravity_and_nav` (`lifecycle.rs:921`) and
  segment B `install_world_cpu` (`:1241`, runs the archetype sweep + pawn spawn),
  `simulate_tick`, the `RunSpec` vocabulary (`observability/runspec.rs:32`).
- The scripted-run driver = this batch driver, but segment B's mesh-upload hook is
  **real** (not the headless no-op) and a renderer exists.

**Entity render path** (windowed, `App::redraw` in `main.rs`):
- Mesh upload: `renderer.load_skinned_model` / `clear_mesh_pass_for_level_load` /
  `skinned_model_clip_metadata` / `skinned_model_local_bounds`
  (`lifecycle.rs:713-738`, the segment-B mesh hook).
- Light bridge: `light_bridge.populate_from_level` (`lifecycle.rs:686`),
  `absorb_dynamic_lights` (`:803`).
- Per-frame collection: `mesh_render.collect_with_hit_zones(...)` →
  `renderer.set_mesh_draws(mesh_render.instances())` (`main.rs:2640/:2653`);
  particle pack → `particle_collections` (`:2610`), passed to
  `render_frame_indirect` (`:2919`).

**Fog render path** (windowed, `main.rs:2559-2579`):
- `fog_volume_bridge.update_volumes(&registry)` → `renderer.upload_fog_volumes`,
  `renderer.set_fog_aabbs(fog_volume_bridge.active_aabbs())`, fog point lights from
  `light_bridge.collect_all_as_map_lights(&registry, time)`. Fog volumes populate
  headless from PRL + registry (observability plan). Light shafts need the dynamic
  lights → available once entities are in scope.
- Temporal accumulation (`rendering_pipeline.md` §7.5) → warmup frames at the
  capture pose before the captured frame.

## Depends on E20-first seams (re-ground before promotion)

Built by `plans/done/E20--frame-capture`; grounded here only as spec text, will
shift in implementation:
- Surfaceless (offscreen) `Renderer` constructor (full-ready, no surface).
- The capture render entry (records scene into `scene_color`, reads back RGBA8) —
  this follow-up extends it with entity/particle inputs and the warmup loop.
- The renderer-owned readback helper, the `capture` module + cargo feature, the
  `--capture` CLI branch, `xtask capture`.

## Open questions

- **Static-entities vs scripted-ticks.** A rest-pose entity capture (spawn, one
  frame, no ticks) is a cheaper intermediate than full scripted-run; the per-frame
  collection is the bulk either way, so the tick loop adds little — argues for
  going straight to scripted-run rather than a third intermediate slice.
- **Capture directive vs separate scene spec.** Reuse the batch `RunSpec` with a
  capture directive (which tick(s), pose = player camera or override), per the
  roadmap north star of one shared vocabulary — not a parallel scene format.
- **GPU adapter.** Like the static slice, this REQUIRES an adapter (unlike the pure
  batch runner's "no GPU" guarantee). It is batch driver + renderer, so it cannot
  ride the renderer-free `observability` build; likely its own feature.
- **Determinism.** Sim is already byte-identical (batch runner); render is
  same-adapter byte-identity; fog temporal warmup must be deterministic (fixed
  warmup count + fixed frame sequence).
- **Fog without entities?** If ever wanted standalone, fog is ambient-only without
  dynamic lights; full fog wants the entity-tier lights, so fog naturally lands
  with entities, not before.

## Trigger

Draft in full once `E20--frame-capture` **Task 1 (the renderer floor)** lands — the
offscreen renderer, capture entry, and readback are the seams this stands on.
