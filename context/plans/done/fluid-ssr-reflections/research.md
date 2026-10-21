# fluid-ssr-reflections — research

Re-read at 99f0601c6, after `reflection-probes`, `fluid-depth-fog-waves`, `fluid-surface-motion` and `fluid-base-pass-cost` landed. Findings that inform the brief without deciding it.

## SSR plumbing
- **Depth.** `Depth32Float` with `TEXTURE_BINDING` (`create_depth_texture`, render/pipeline_layout.rs), at `prepass_attachment_extent`. Fog compute, SDF shadow compute, forward and the fluid pass already read it. At fluid time, depth holds world, movers and skinned meshes. The viewmodel pass clears depth after bloom.
- **Scene color.** `scene_color` is `Rgba16Float`, `RENDER_ATTACHMENT | TEXTURE_BINDING | TEST_COPY_USAGE` (`create_scene_color`, screen_effects.rs). No pass between "Skinned Mesh Pass" and "Fluid Surface Pass" attaches it (`Renderer::record_scene_passes`).
- **Reduced-resolution precedents.** SDF shadow (half), fog scatter (`fog_pixel_scale`, `fog.resize(…, &full.depth_view)`) and the bloom chain.
- **Fluid budgets** (crates/renderer/src/render/tests/pipeline_budget_tests.rs):
  - Fragment sampled textures per group `[0,4,0,3,2,1]` = 10 of `FORWARD_SAMPLED_TEXTURE_BUDGET` 16. Group 1 holds albedo, emissive, specular and normal; group 3 holds the SH textures; group 4 holds the probe colour and distance arrays; group 5 holds scene depth.
  - Storage `[0,0,5,3,0,0]` = 8/8.
  - Uniforms 7, samplers 2 (`fluid_pipeline_fragment_uniforms_and_samplers_stay_within_limits` pins the full per-group table).
  - `FLUID_BIND_GROUP_COUNT` = 6. Group 7 is the one budget slot left (§10).
- **Group 5 spares.** `FluidSceneUniform` (fluid_scene_inputs.rs) carries `spare: [f32; 2]`, reserved for screen-space reflection inputs. Group 5 is built in `FluidSceneInputs::new` and rebuilt only from `rebuild_scene_targets`, pinned by `fluid_scene_inputs_are_built_at_init_and_rebuilt_only_with_the_scene_targets` (Pin P4).
- **Dummy precedent.** `fluid_reflection_probes.rs` binds 1×1 dummy probe arrays on a level with surfaces but no usable id 54.
- No SSR, Hi-Z or runtime cubemap exists.

## Fluid draws and variants
- `FluidSurfacePass` owns `InstalledFluidSurfaces.slots` (`FluidSurfaceSlots`). `frame_draws(mask)` yields `(slot, variant)` for the frame. The collector sorts volumes far to near (render-cpu `fluid_surface/cull.rs`). Faces within a volume draw in install order, per material range.
- **Vertex streams.** Pos/normal/uv (locations 0–2), drift (3), probe index (4), plus an index buffer. `vs_main` reads group 3 for per-vertex baked light.
- **Variant choice.** `FluidParamsRow::shader_variant` (render-cpu `fluid_surface/slots.rs`) picks reflecting only for a probe-bound volume at reflectivity > 0 under the all-terms mask. Reflectivity is packed 0 for a volume with no probe-bound face. The per-fragment gate is `fluid_reflection_gate` (fluid_reflection.wgsl).
- **Two entry points.** `fs_main` and `fs_main_reflecting` are separate entry points (commit 67ef11133). The reflection block's presence slows the always-run lighting on some drivers (§7.3a Two pipelines).
- **Free row spares.** `surface.w` and `fog_albedo.w`.

## Probe term facts SSR builds on
- **Composite.** `F·R + (1−F)·(surface over fog over scene)` at coverage `F + (1−F)·a`, with F = Schlick(0.02 × reflectivity, cap 1) × capture-shadow weight × fog proximity band (`fluid_reflection_composite`).
- **Reflectivity.** `surfaceReflectivity` is 0..=50, default 1 (`FLUID_REFLECTIVITY_MAX`).
- **Style.** `render.reflections.style` is landed: `ModReflectionProfile { style: ModReflectionStyle }` (scripting-core runtime/types.rs), committed by `apply_mod_reflection_profile` / `renderer_reflection_style` (startup/render_profile.rs) at boot and staged reload, and delivered to `FluidSceneUniform.reflection_style`. Manifest parse tests live in `reflection_profile_tests.rs`.
- **Pixel cell point.** `fluid_reflection_point` / `fluid_reflection_cell_centre` work on the unwarped UV grid, tilted by `fluid_displayed_offset` (shimmer, rings). They use `dpdx`/`dpdy` texel axes. Their CPU reference is render-cpu `reflection_style.rs` (`FluidReflectionStyle`, `FLUID_REFLECTION_PIXEL_TILT`).

## IP
Full triage in `ip-check.md` (this draft). Summary:
- **Tencent WO2026113651A1 claim 15** (pending): supplements the probe's lighting *when* its obstruction test fires. SSR composited on its own hits, with a contribution independent of s, lacks that step.
- **EA/Uludag viewport-exit family** (US10621774B2, KR/CN counterparts): claims a roughness-driven ray count and ray tracing of failed rays. The brief has neither.
- **Microsoft US10586375B2 / US10360720B1**: every independent claim decides whether the ray reflects toward the camera. The brief's direction ban avoids it. EP3756162's text intended for grant is unread.
- **NetEase CN114748874B** (granted, China only): water reflection from screen colour and a probe, both "according to the reflection position", under Fresnel. The brief keys the probe to its own search, never the SSR hit. Claim text unverified.

## Perf baseline (`plans/done/fluid-base-pass-cost/`)
- **5300M at 4K.** The reflection-free fluid pass is 2.2 ms. Reflecting minus free is ≈10 ms: near 11.80 → 10.16, probes 14.29 → 12.69, mid 6.65 → 5.27.
- **1660 Super, 4K near.** Reflecting minus free is 2.48 ms.

## Options precedent
- Every graphics option is its own `PlayerOptions` field. No preset exists.
- `fog_quality` (low/medium/high, live via `Renderer::set_fog_step_size`, default medium) is the closest tiered precedent. `surface_depth_quality` is off/on.
- No `reflection_quality` exists yet.

## Dev content
- **Fluids.** Declared in `content/dev/start-script.ts` and `scripts/fluids-swim.ts` (with its Luau mirror): `water` and `sludge`.
- **Maps.** `maps/stress-env-volumes.map` and `maps/e24-environment-test.map` name them. The manual pillar / mover capture needs a pool with a pillar and a mover in view, in one of these or a new fixture.

## T-junction closure (in progress)
Compiler-only. It changes no draw-list shape and no vertex layout, and adds, drops and reorders no faces. A face may carry many more vertices, which matters only for pass cost. A weld may tilt a sub-6 mm sliver off up-facing, so it loses its probe binding and, by the shared gate, SSR too.

## Pin table
Orderings and edges the Acceptance rows cite.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Tier and render resolution change on one tick | Options apply sets tier T and records resolution R on tick N; frame N+1's start commits extents and rebuilds scene targets | Frame N+1 binds one target at T's fraction of R's scene extent, with the matching scale; no frame samples a target whose extent disagrees with the scale |
| P2 | Scene-target rebuild with a live target | Depth and scene colour, then the target, then group 5 and the reflection pass's inputs, in one rebuild | No group 5 or reflection-input build references a superseded depth, scene colour or target |
| P3 | Tier leaves Off with no fluid level installed | No level, or one with no fluid volumes; tier Off → high; then a fluid level installs | No target before the install; the install creates it at the current tier and rebuilds group 5 once |
| P4 | Unload at a tier other than Off, then level changes | Fluid level A at medium draws; unload; level B with no fluid; level C with fluid | Unload rebuilds group 5 over the placeholder and no bind group holds A's target; B allocates only the placeholder; C creates its own target at medium |
| P5 | Tier set to the value it already holds | An options re-apply, resume replay or menu write repeats the current tier | No target creation and no group 5 build |
| P6 | Tier goes Off with a target live | Setter runs between frames N and N+1 | Group 5 holds the placeholder before the target is dropped; frame N+1 records no reflection pass and every draw takes the reflection-free variant |
| P7 | Target texel no reflecting draw wrote this frame | Frame N writes a hit at texel t; at frame N+1 t lies outside every reflecting draw (mover left, gate shut, camera moved) | t reads c = 0 at frame N+1, never frame N's hit |
| P8 | Style change by staged reload mid-level | Reload commits between frames N−1 and N | Frame N's reflection pass and fluid pass both use the new style; no frame mixes pixel rays with smooth sampling or the reverse |
| P9 | Isolating lighting mask at a tier other than Off | The debug mask isolates one term for frame N | Both passes take variants from frame N's one mask snapshot; no reflection pass is recorded, and the frame equals the reflection-free frame |
| P10 | Camera at or below the water plane | Eye on the plane, then 1 mm below it | Below: the gate shuts in both passes and the frame equals the same frame at Off. On the plane: a fragment the fluid pass admits but the reflection pass did not shade reads c = 0 (P7) |
| P11 | Ray heading toward the camera reaches the near plane or leaves the depth range | The march starts as for any ray | Same step sequence as a ray heading away up to that point; it ends as a miss with c = 0, by the same rule as a ray leaving the screen sideways |
| P12 | Reflection pass reads its inputs while writing its target | The target is the pass's colour attachment and group 5's binding 2 | The reflection pass binds no group holding its own target; a frame recording both passes raises no validation error |
| P13 | Pass order within the frame | Skinned Mesh Pass → reflection pass → Fluid Surface Pass → billboards, smoke, viewmodel | The reflection pass reads scene colour holding only the opaque scene, with neither scene colour nor depth attached |
| P14 | Reflection pipeline creation | Full init at Off; tier raised mid-level | The pipeline is built at full init at every tier; raising the tier compiles nothing |
| P15 | Pixel cell whose centre the camera cannot see | Centre behind an opaque object standing in the water; or off screen, or past the face's edge | Behind an object: the reflection pass shaded the centre, its march skips the starting overlap with that object, and every visible fragment reads the cell's own SSR. Off screen or past the edge: c = 0, the probe term alone |
| P16 | Reflecting draws overlap in the target | Volumes sort far to near by bounding-box centre, so a hidden or farther surface may draw after the visible one | The nearest surface's fragment owns the texel by the pass's own depth test; draw order never decides it |
| P17 | Reflectivity changes mid-level | Staged reload or tuning payload retunes a probe-bound volume 0 → 1 → 0 between frames | The target exists throughout (keyed to probe binding); the next frame switches variant; no target or group 5 rebuild |
