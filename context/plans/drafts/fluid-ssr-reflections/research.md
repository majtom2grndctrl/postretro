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
- `reflection-probes/ip-check.md` (claim 15 row): a screen-space layer that fills where the distance test flags occlusion is closer to Tencent WO2026113651A1 claim 15. Compositing on SSR's own hits is a separate technique.
- `reflection-probes/research.md` §IP lists the EA/Uludag SSR-to-ray-trace viewport-exit family under Avoid. The brief's confidence-weighted miss handoff is under the owner's pre-ship claims check.

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
