# Capture harness: dynamic receivers + authored animation state

## Goal

The static frame-capture harness (`crates/postretro/src/capture`, E20) renders a
world-only, script-free, single-instant frame: the scene vocabulary is
`{map, camera, resolution, output}` (`scene.rs`, `deny_unknown_fields`), the driver runs
no script VM (`now_seconds = 0`), and it installs **static world lights only**
(`capture_static_lights_and_shadow_selection` filters `!is_dynamic` and remaps the
entity-shadow selection into compact static-light index space; `light_influences: &[]`). It
builds `LevelGeometry` from `level_world_to_geometry` — which carries static world geometry
plus the mover `kinematic_geometry` buffers — but drives none of the per-frame entity-draw
collectors the windowed path runs, so no mover, `prop_mesh`, or billboard draws reach the
captured frame. Because of this, several visual acceptance surfaces cannot be automated as
capture goldens today — most recently the "animated direct SH on dynamic receivers"
feature, whose AC1/AC2/AC4/AC5/AC10 visual halves had to fall back to `[manual GPU]` eyeball
checks.

Extend the capture harness two ways — (1) render dynamic receivers by standing up the
map-authored entity state the runtime frame draws, and (2) accept an **authored animation
state** (a forced-active animated compose descriptor) so a static capture can reach an
animated light's "fired" appearance **without** running the script VM or an event loop.
This converts the deferred manual-GPU checks into deterministic, threshold-based capture
goldens, restoring CI visual-regression coverage for baked-animated-light features.

The dynamic-receiver entities spawn from the loaded level **without** the script VM: the
closet-door kinematic mover from `spawn_loaded_kinematic_movers` (over
`LevelWorld.kinematic_geometry`), and the `prop_mesh` skinned mesh from the VM-free built-in
classname handlers (`apply_classname_dispatch` / `register_builtins`), both before any
data-script or `levelLoad` work. Each draws at its authored rest pose with no tick and no
animation runtime. The `billboard_emitter` receiver is **not** purely load-only — a
map-authored emitter has emitted no particles at load, and `particle_render` draws
particles, not emitters, so materializing its sprite needs a bounded particle sim tick — so
**billboards are deferred to a follow-up spec**; this spec renders the mover and `prop_mesh`
receivers.

## Scope

### In scope

- **Capture-side entity state (mover + `prop_mesh`).** Stand up a VM-free entity registry
  from the loaded level and draw the map-authored kinematic mover and `prop_mesh` skinned
  mesh at rest pose, through the same renderer draw seams the windowed frame uses
  (`set_kinematic_mover_draws`, `set_mesh_draws`), so a captured frame shows the same
  receivers the windowed engine does.
- **Authored animation-state input.** Add an optional `force_active` field to `CaptureScene`
  naming light tags — each with an authored radiance — whose animated compose descriptors
  this capture forces active, so the capture renders the animated light at its red alarm peak
  deterministically. No script VM, no trigger firing — the state is authored directly in the
  scene JSON and seeded through `Renderer::write_animated_compose_descriptor` after
  `install_level_geometry`.
- **Golden coverage** for baked-animated-light dynamic-receiver features: convert the
  single-instant receiver-reddening `[manual GPU]` halves in
  `animated-direct-sh-dynamic-receivers` (the mover and `prop_mesh` reddening) into a capture
  golden against an authored red frame, and the no-authored-state pre-fire frame into a
  baseline golden. `alarm_light` bakes `start_active` (no `_start_inactive`; the `turnRed`
  `setLightAnimation` is a non-`levelLoad` reaction whose `startActive` never lowers the
  reservation default), so the pre-fire receivers are lit at its baked rest radiance, not dark
  — the baseline golden asserts that lit rest color (see AC3, P4).
- Keep the harness deterministic and GPU-adapter-gated (self-skip without an adapter),
  matching the existing E20 golden discipline.

### Out of scope

- **Billboard receivers.** `billboard_emitter` sprites require a bounded particle sim prime
  (`emitter_bridge::update` + `particle_sim::tick`) to materialize before they can be drawn;
  deferred to a follow-up spec. The dependency's billboard visual half stays `[manual GPU]`.
- **Running the script VM / firing triggers in capture.** The animation state is authored in
  the scene JSON, not produced by executing `spawner-test.ts`. The reddening comes from
  seeding the baked animated light's compose descriptor, not from the VM-driven `turnRed`
  trigger. Full end-to-end script execution in a headless frame loop is a separate, larger
  effort, not this spec.
- **`anim_time_s` / curve-time authoring.** Threading a frozen time into the frame `time`
  uniform to evaluate a baked curve is deferred: the fixture's `alarm_light` is a finite
  `play_count` animation whose descriptor bytes are a function of elapsed-since-fire (not one
  instant), and no fixture light offers a steady or phase-addressable descriptor for a golden
  to exercise. This spec authors the fired radiance directly (see Task 1) and leaves curve-time
  authoring to a spec that has a sound consumer for it.
- **Event loop / multi-frame animation.** A single authored instant per capture, not a time
  series.
- **New engine lighting behavior.** This is a test-harness capability; it changes no runtime
  rendering path, only what the capture driver spawns, feeds, and freezes.

## Dependencies

Depends on `animated-direct-sh-dynamic-receivers` shipping first (it provides the compose
animation descriptors and the dynamic-receiver direct term this harness freezes and
photographs, and recompiles `spawner-test` with the alarm light as a baked animated
`light_spot`). This spec is the deferred follow-up that plan's Task 5 names.

## Acceptance criteria

- [ ] **AC1** `[unit]` `CaptureScene` accepts the new `force_active` field — light tags each
      with an authored radiance — and rejects malformed input (unknown fields on both
      `CaptureScene` and `ForcedAnimLight` via `deny_unknown_fields`, an empty tag, a
      non-finite radiance) through the existing `validate_scene` path, mirroring the
      `scene.rs` parser tests.
- [ ] **AC2** `[golden]` A capture scene with `force_active` naming `alarm_light` at an
      authored red radiance on `spawner-test.prl` renders the closet-door mover and the
      `prop_mesh` reddened with the wall inside the cone — the automated replacement for the
      mover and `prop_mesh` manual-GPU checks deferred from
      `animated-direct-sh-dynamic-receivers`.
- [ ] **AC3** `[golden]` A capture scene with no authored animation state renders the
      receivers under the light's **baked rest descriptor** — the state
      `install_level_geometry` seeds before any authored write — captured deterministically on
      one adapter and distinct from the AC2 red frame. The fixture's rest activity is stated
      explicitly: a `start_active` baked descriptor contributes its rest radiance (the
      receivers are lit by
      the animated term at rest, not unlit), a `start_active: false` descriptor contributes
      zero (start-dark). "No authored state" and "all descriptors force-inactive" name the
      **same** frame only when the baked rest descriptor is itself inactive; the golden
      asserts whichever the fixture bakes. Rest-appearance coverage only — the no-pop transition
      stays `[manual GPU]` per AC6.
- [ ] **AC4** `[unit]` Seeding `force_active` onto a finite `play_count`-bounded baked
      descriptor authors the chosen radiance directly into the descriptor's `base_color` with
      `color_count` 0 (the `active_without_animation` path) and claims no byte-equivalence to
      the runtime `light_bridge`, whose `pack_compose_animation_descriptor` bytes are a
      function of elapsed-since-fire, not a single authored instant.
- [ ] **AC5** `[review]` The mover and `prop_mesh` draws populate the captured frame from the
      same renderer draw seams the runtime frame uses (`set_kinematic_mover_draws`,
      `set_mesh_draws`) over a VM-free spawned registry, drawn at rest pose — no capture-only
      divergent draw path, and no script VM or event loop.
- [ ] **AC6** `[review]` For each `animated-direct-sh-dynamic-receivers` AC whose deferred
      `[manual GPU]` visual half is now reproduced by a capture golden here — the mover and
      `prop_mesh` reddening, and the pre-fire rest baseline — that half is re-tagged `[golden]`
      in the dependency spec with a reference to the covering scene (AC2 or AC3). Halves
      outside this spec's scope — the deferred billboard receiver, and multi-frame behavior
      (despawn/reload drop, brightness-pop transition, animation over time) — stay
      `[manual GPU]`, so the conversion is per-half, not a whole-AC flip.
- [ ] **AC7** `[manual GPU]` + `[review]` The harness stays adapter-gated and deterministic
      (self-skips without an adapter; identical bytes across runs on one adapter). Each scene
      captures in its own `--capture` process, so process isolation precludes a prior scene's
      authored red seed leaking into a later rest scene.
- [ ] **AC8** `[review]` `context/lib/rendering_pipeline.md` §7.8 (frame capture) records the
      dependency: the capture harness reaches an animated light's fired appearance by seeding
      an authored animation state (a forced-active compose descriptor), with no script VM or
      event loop. Verified by grep/read.

## Tasks

### Task 1: Authored animation-state scene input

Extend `crates/postretro/src/capture/scene.rs`: add an optional
`force_active: Option<Vec<ForcedAnimLight>>` to `CaptureScene`, where each `ForcedAnimLight`
carries a light `tag: String` and an authored `radiance: [f32; 3]` and derives
`deny_unknown_fields` + `snake_case` like the other scene structs, so an unknown subfield is
rejected, not just an unknown top-level field. Extend `validate_scene` consistently with the
existing finite/range checks — reject an empty tag and a non-finite radiance. In `driver.rs`, after `install_level_geometry` rebuilds
`sh_volume_resources` (which reinstalls the baked descriptors from the SH sections and
overwrites any earlier seed) and before `capture_frame_indirect`, resolve each named tag
against the **unfiltered** `world.lights` (`MapLight.tags` → `MapLight.animated_slot`, the SH
animation-descriptor index — not the `static_lights` clone
`capture_static_lights_and_shadow_selection` produces) and seed that slot active with the
authored radiance via `Renderer::write_animated_compose_descriptor`. Build the descriptor
bytes through the shared `active_without_animation` packer
(`light_bridge::pack_animation_descriptor`, exposed `pub(crate)` and reused, not
re-implemented): the radiance goes into `base_color` with `color_count` 0, so no `anim_samples`
curve region is seeded and no runtime `light_bridge` byte-equivalence is claimed (for a finite
`play_count`-bounded descriptor that equivalence is unreproducible from a single instant). The
write is flushed to
GPU on the following `update_per_frame_uniforms`, so it must precede that flush. An unknown tag
is a validation error, not a silent no-op; multiple matched slots are each seeded once,
order-independent. Parser + validation unit tests mirror the existing `scene.rs` suite. Pins
P1, P3, P5, P6.

### Task 2: Capture-side entity state — mover + `prop_mesh` draws

Stand up a VM-free entity registry in `driver.rs`: construct a bare `EntityRegistry`, spawn
the map-authored receivers from the loaded `LevelWorld` — the kinematic mover via
`spawn_loaded_kinematic_movers`, the `prop_mesh` via the built-in classname handlers
(`apply_classname_dispatch` over `world.map_entities` converted through the
`MapEntity::from(MapEntityRecord)` adapter, with a `ClassnameDispatch` from
`register_builtins`) — and do **not** call `install_world_cpu` wholesale (it also runs the
data script and fires `levelLoad`). Upload each spawned `prop_mesh`'s model (its
`MeshComponent.model` handle) via `renderer.load_skinned_model` with the driver's
`content_root` + `prm_cache_root`, before `capture_frame_indirect` — the mesh plan gates on the
model being loaded, so an uncached model silently drops the draw. Run the
`KinematicMoverRenderCollector` (`collect(registry, world, visible, alpha = 1.0)`) and
`MeshRenderCollector` (`collect_with_hit_zones` with `alpha = 1.0`, `anim_time = 0.0`, a fresh
`MeshClipTables`, the capture eye as `camera_pos`, an empty `HitZoneStore`) over the registry
at rest and submit their instances through `set_kinematic_mover_draws` and `set_mesh_draws`
before `capture_frame_indirect`. The mover geometry is already threaded into
capture by `level_world_to_geometry`; the mover and `prop_mesh` draw at authored rest pose
with no tick. Confirm both consumers sample the composed direct atlas (binding 15) in the
captured frame exactly as in the windowed engine. Pin P8.

### Task 3: Convert deferred manual-GPU checks to goldens

Compile `spawner-test.map` → `.prl` at test time (as `specular_shadowmask_capture` does via
`prl-build`), then add adapter-gated capture regressions (self-skip without an adapter,
same-adapter comparison — the harness commits no golden PNGs, since adapter rounding makes
rendered bytes unsuitable for cross-adapter CI): the reddened mover and `prop_mesh` under an
authored red `force_active` scene (AC2), and the rest frame under no authored state (AC3,
asserting whichever rest state the fixture bakes — pins P2, P4). Each scene captures in its own
`--capture` process, so process isolation already precludes a red seed leaking into a later
rest scene (pin P7). Once these land, re-tag the covered deferred `[manual GPU]`
halves in `animated-direct-sh-dynamic-receivers` to `[golden]` per AC6, each referencing the
covering scene, and leave the out-of-scope halves manual. Note the harness dependency in
`context/lib/rendering_pipeline.md` §7.8 (frame capture) (AC8).

## Pinned behaviors

| id | Scenario | Expected outcome | Kind |
|---|---|---|---|
| P1 | Authored red seed written after `install_level_geometry` and before `capture_frame_indirect` | Seed survives the SH-section reseed; the red frame is captured | unit |
| P2 | `force_active` absent (no authored state) | Descriptors retain their baked rest state, not forced inactive | unit |
| P3 | `force_active` targets a finite `play_count`-bounded descriptor | Radiance authored into `base_color` directly; no runtime byte-equivalence claimed | unit |
| P4 | Fixture `alarm_light` baked `start_active` | The no-authored-state golden is lit at the baked rest color per the fixture, stated explicitly | unit |
| P5 | `force_active` names a tag matched by two or more lights | Every matched descriptor is seeded once; final bytes are order-independent | unit |
| P6 | `force_active` names an unknown tag | Hard error when the driver resolves tags to slots against the loaded world (post-`install_level_geometry`), not a silent no-op | unit |
| P7 | Each capture scene runs in its own `--capture` process | Process isolation precludes a prior scene's red seed leaking into a later rest scene; a future in-process multi-scene API must re-install geometry per scene | unit |
| P8 | Mover + `prop_mesh` draws in capture | Populated from the per-frame collectors over a spawned registry — not `level_world_to_geometry` — at rest pose, no tick, no VM | unit |

## Notes

Grounding for the harness extension:
- `crates/postretro/src/capture/scene.rs` — scene vocabulary + `validate_scene`.
- `crates/postretro/src/capture/driver.rs` — `capture_static_lights_and_shadow_selection`,
  `LevelGeometry { light_influences: &[] }` (the entity-shadow selection is remapped, not
  empty), `install_level_geometry`, `capture_frame_indirect`.
- `crates/renderer/src/render/renderer_lighting.rs::write_animated_compose_descriptor` — the
  compose-descriptor seed seam; `crates/postretro/src/scripting/systems/light_bridge.rs::pack_animation_descriptor`
  — the `active_without_animation` `base_color` path Task 1 mirrors.
- `crates/postretro/src/runtime_movers.rs::spawn_loaded_kinematic_movers` and
  `crates/postretro/src/scripting/builtins/mod.rs::apply_classname_dispatch` — the VM-free
  spawn seams Task 2 uses; `crates/renderer/src/render/renderer_models.rs::{set_kinematic_mover_draws, set_mesh_draws}`
  — the renderer draw seams the runtime frame uses.
- `crates/renderer/src/render/renderer_geometry.rs::level_world_to_geometry` — the
  static-world geometry-build seam (world geometry plus the mover `kinematic_geometry`
  buffers). It does **not** build the per-frame mover / `prop_mesh` / billboard draws — those
  come from the entity-registry collectors (`set_mesh_draws`, `set_kinematic_mover_draws`)
  Task 2 drives.
- See also `context/plans/*/E20--frame-capture` for the original capture design intent.
