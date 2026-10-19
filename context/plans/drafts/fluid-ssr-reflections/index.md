# fluid-ssr-reflections

Brief · resumable · reads: `context/lib/index.md` §1.1, §3 · `rendering_pipeline.md` §7.3a, §7.5, §8, §10, §13 · `player_options.md` §4 · `resource_management.md` §5 · `plans/done/reflection-probes/` (`ip-check.md`, `research.md` §IP) · this draft's `ip-check.md` · read at 99f0601c6 · `fluid-surface-motion`, `fluid-depth-fog-waves`, `reflection-probes` and `fluid-base-pass-cost` have landed; independent of `fluid-surface-t-junction-closure` (compiler-only)

## Problem
The owner requested a capability. Fluid surfaces now reflect the static room their probe captured (`rendering_pipeline.md` §7.3a, Reflection), but movers, characters and dynamic lights never appear in them. Players will see the walls, lights and movers around a pool mirrored in its surface, and objects standing in the water reflected on every side the camera sees. SSR cannot reflect a surface the camera never sees, such as the underside of a dock seen from above, so this brief does not cover it (Non-goals). A game author already chooses pixelated or smooth reflections (`render.reflections.style`). A player turns reflection quality down for cost, never past what the author chose, and can turn reflections off.

## Decisions
- **Screen-space reflections, always composited over the probe term and weighted by confidence.** SSR reflects what surrounds a pool on screen. Each layer covers what the other cannot see:
  - SSR owns movers, characters, dynamic lights, and the camera-facing sides of objects standing in the water.
  - The probe owns the static room off screen.
  - On every reflecting fragment the two blend by SSR confidence c = hit confidence × screen-edge fade, and nothing else.
  - **No direction term.** c reads no ray-direction quantity. The march never exits early, skips, re-weights or changes step by direction; a ray heading toward the camera marches like any other. Per-pixel technique choice by direction is the Microsoft US10586375B2 pattern (`ip-check.md` §3).
  - **SSR never reads the capture shadow.** s, the probe's capture-shadow weight, scales the probe's share only, so SSR's contribution F·c·SSR is identical at s = 0 and s = 1. The reflection pass reads no s, distance tile or probe resource. Nothing is added when the probe's obstruction test fires, which keeps clear of Tencent claim 15 (`ip-check.md` §1).
  - **The probe lookup ignores the SSR hit.** It is keyed to its own search from the fragment or cell point, never to the SSR hit, its depth or its screen position. This distinguishes it from NetEase CN114748874B (`ip-check.md` §4).

  This is the layering §7.3a Limits anticipates ("composites over this term and falls back to it on a miss"). It is not the capture-shadow-triggered fill that §7.3a and `reflection-probes/ip-check.md` (claim 15 row) reserve for IP review. At promotion, the docs record SSR as the dynamic fluid reflection layer and state that s scales the probe term only.
- **Owner claims check before shipping.** `ip-check.md` finds no live claim that reads on this design while the layering rules above hold. The EA/Uludag viewport-exit family claims a roughness-driven ray count and ray tracing of failed rays; this design has neither. Open items are NetEase CN114748874B (granted, China only, claim text unverified) and Microsoft EP3756162 (granted text unread). The owner's pre-ship claims check, which already covers the probe term, extends to this layer with the items in `ip-check.md` §9. It gates shipping, not building.
- **`index.md` §1.1 gains a third admitted category of runtime ray work.** §1.1 stays a rule. Beside offline bake rays and cheap traces against baked structures, it admits *a bounded screen-space march against the current frame's depth buffer, on gated fragments, under a fixed step budget*. Each use still needs owner sign-off in its own brief. Fluid SSR is the first:
  - The probe term (`reflection-probes`) is the baked equivalent for everything static. What remains, movers and dynamic lights, has none.
  - The march reads scene depth, which fog, forward and the fluid pass already sample, so it adds no acceleration structure.
  - It runs only for fragments the reflecting gate admits, at the player's quality tier, under a fixed step budget.
  - Its cost is measured on the §10 perf floor before landing.

  The amendment lands at promotion. `ssr-decals` fits the category but needs its own sign-off.
- **No new material input.** `surfaceReflectivity` (landed with `reflection-probes`) scales both layers through the same Fresnel. No world material gains a reflection input (§13).
- **SSR reuses the probe term's gate.** A fragment gets SSR only where the landed reflecting gate admits it:
  - per draw, the `FluidShaderVariant::reflections()` choice made by `FluidParamsRow::shader_variant`: a volume with a probe-bound face, reflectivity above 0, and every `LightTermMask` bit on;
  - per fragment, `fluid_reflection_gate`.

  So a face bound to no probe, a down-facing face, reflectivity 0, and any lighting-mask isolation draw exactly as without reflection. The reflection-free variant never reads the SSR target, so it keeps its cost, and the two variants still match pixel for pixel where the gate is shut.
- **The reflection resolves into its own target at the tier's resolution, before the fluid pass.**
  - **Reflection pass.** It draws the frame's reflecting fluid draws into that target. It reads opaque scene color and scene depth directly: at that point neither is attached, and scene color holds exactly the opaque scene, so no full-screen copy is needed.
  - **Fluid pass.** It samples the target only in `fs_main_reflecting`. Under smooth style it samples at its own screen position. Under pixel style it samples at the projected centre of its water texel cell, so a reduced tier never lays a screen-locked grid over the water's blocks.
  - **Group 5.** The target joins group 5 as binding 2, and SSR's per-frame inputs use the two reserved spares in `FluidSceneUniform` (`fluid_scene_inputs.rs`). Fragment storage stays at 8/8 (§10).
- **Group 5 binds a placeholder until a real target exists.** A 1×1 placeholder, following the probe-array dummies in `fluid_reflection_probes.rs`, is bound at init.
  - The real target is created when a fluid level installs with a tier other than Off, or when the tier leaves Off. It is released on Off or on unload.
  - Group 5 is rebuilt by `FluidSceneInputs::rebuild` from those points and from `rebuild_scene_targets`.
  - Pin P4 (`fluid_scene_inputs_are_built_at_init_and_rebuilt_only_with_the_scene_targets`) widens deliberately to name exactly these callers. Group 5 is still never rebuilt per frame, and `install` and `clear` still do not touch it directly.
- **What reflects.** Reflections show what the opaque scene held when the reflection pass ran. Smoke, fog, billboards, projectile bodies, the viewmodel and other fluid surfaces never appear. These are accepted limits.
- **Composite.** With F the Fresnel × fog-proximity weight (unshadowed), s the probe's capture-shadow weight, and c the SSR confidence:
  - reflected colour = `F·[c·SSR + (1−c)·s·Probe]`;
  - its weight W = `F·[c + (1−c)·s]`;
  - the fragment = reflected colour + `(1−W)·(surface over fog over scene)` at coverage `W + (1−W)·a`.

  At c = 0 this is exactly the landed composite (`F·s` is today's F), so a miss, Off and a fragment SSR never reached all draw today's picture. Opacity and fog never dim either layer. The composite joins the GPU-free CPU reference in render-cpu, mirrored statement for statement (§7.3a References).
- **The author owns the style.** `render.reflections.style` is landed (`ModReflectionStyle`, `renderer_reflection_style`) and already reaches the fluid uniform. SSR reads it, with no new parse.
  - **Pixel** takes each ray's origin and direction from `fluid_reflection_point` / `fluid_reflection_cell_centre`, the probe term's cell point on the unwarped texel grid. Shimmer and rings tilt it; drift and wave tilt nothing. So both layers break into the same blocks.
  - **Smooth** marches per reflection-target pixel about the flat normal.

  The player never chooses the style.
- **The player owns cost: `reflection_quality` = off / low / medium / high, default high.** The owner chose this four-tier ladder; first-slice measurements tune what each tier costs, not whether it exists.
  - **Off turns off both layers.** Every draw takes the reflection-free variant, so the player recovers the probe term's cost too. `shader_variant` gains the option as an input. No reflection pass is recorded.
  - **Low, medium and high** keep the probe term unchanged and resolve SSR at quarter, half and full scene extent. The step budget scales with resolution, so a ray reaches equally far at every tier.
  - **Precedent.** It reuses `fog_quality`'s plumbing (a tiered ladder, persisted, slot-exposed, applied live), not its policy. In fog the map owns resolution (`fog_pixel_scale`) and the player owns step size. Here the player owns resolution and reach is fixed. Unlike `set_fog_step_size`, the setter also creates, resizes or releases the target and rebuilds group 5.
  - **Divergence from the Surface Depth ruling.** This ladder departs on purpose from the ruling against tiers (`player_options.md` §4). That ruling rejected a tier that changed the shape of the effect at grazing angles. A lower tier here changes only the resolution of the dynamic layer, plus Off. Under pixel style, the default, low through high look alike wherever a water texel cell spans several target pixels, and differ only where cells shrink below a target pixel with distance. That makes the ladder mostly a cost lever with little visible loss, which is what a player turning quality down wants. The owner kept four tiers knowing this.
  - No umbrella quality preset is added.
- **Sequencing.** SSR builds before the roadmap's undrafted "reflection-probe fluid-pass performance" item. That item is drafted afterwards and optimizes the combined composite in `fs_main_reflecting` once.
- **Non-goals.**
  - Temporal reuse: a reflection updated below frame rate lags a turning camera without reprojection. `motion-reprojection-layer` owns that facility, and §IP lists reflection history under Avoid.
  - SSR on non-fluid surfaces: `ssr-decals` stays separate and may reuse the march snippet.
  - SSR on faces the probe gate refuses, such as a face bound to no probe. Revisit only if a level shows up-facing water with no derived probe.
  - Surfaces the camera never sees, such as a dock's underside seen from above. Screen space cannot return them. An authored `reflection_volume` whose `_capture_offset` shifts C horizontally to beneath the deck (C stays 2 mm above the water) may cover them where the deck clears the water (`resource_management.md` §5). At promotion, `roadmap.md`'s fluid-ssr-reflections line and §7.3a Limits ("Water C cannot see or resolve, such as the underside of a dock over open water, waits for that layer") are corrected to say so, without describing the screen-space layer as filling the capture shadow.
  - Glints from dynamic lights, after `billboard-specular-shimmer`, as a complement for lights off screen or drawn only as billboards. This is a possible follow-up, not this brief.
  - Refraction, as E24 deferred it. Underwater fog stays out.

### Scripting surface
No new author surface. `render.reflections.style` and `surfaceReflectivity` landed with `reflection-probes` (`scripting.md` §2). The only new name is the player option `reflection_quality` (Boundary inventory).

## Acceptance
### Automated
- [ ] Off, a level with no fluid volumes, and a frame with no reflecting draw each record no reflection pass.
- [ ] Off makes every draw take the reflection-free variant: `shader_variant` returns it for a probe-bound volume at reflectivity 1 under the all-terms mask when the option is Off, and the reflecting variant at every other tier.
- [ ] Off and a level with no fluid surfaces allocate only the 1×1 placeholder. Switching from Off to a tier mid-level creates the target and rebinds group 5, and switching back releases it. Pin P4 names exactly the widened set of `FluidSceneInputs::rebuild` callers.
- [ ] Each tier resolves at its fraction of the scene extent. Changing tier or render resolution rebuilds the target, and the next frame binds the new one.
- [ ] Composite CPU reference:
  - at c = 0 it equals the landed composite for every s, F and a;
  - at c = 1 the probe contributes nothing;
  - SSR's contribution F·c·SSR is bit-identical at s = 0 and s = 1 for fixed F and c.
- [ ] The reflection pass's pipeline layout binds no probe resource, and its shader module reads neither s nor the distance tile.
- [ ] The same ray reaches the same hit at high and low on a fixed synthetic depth buffer (GPU-free march reference), so reach does not shrink with tier. A ray leaving the screen gets c = 0, and one ending at the edge fade band gets 0 < c < 1. A ray heading toward the camera marches the same budget as one heading away and gets c from hit and edge alone.
- [ ] Pixel style gives one ray per water texel cell, from the same cell point and direction as `fluid_reflection_point`. Every fragment of a cell reads the same reflection value at every tier. Smooth gives one ray per target pixel.
- [ ] `reflection_quality` round-trips through `settings.toml` and its slot for each of off, low, medium and high. An unrecognized stored value falls back for that field only. The graphics-tab id/count test in `main.rs` counts the new control.
- [ ] The player setting carries no style value, so the player cannot select smooth under a pixel game.
- [ ] Pipeline budget tests pass with group 5's per-group row updated:
  - fluid fragment storage stays at 8;
  - sampled textures go from 10 to 11 of 16;
  - uniforms and samplers stay at their pinned counts.
- [ ] The DX12 FXC shader test compiles the reflection shader and both fluid variants (Windows, ignored-gated run).

### Manual
- [ ] Walls, lights, a moving mover, and a pillar standing in a pool, including the pillar's camera-facing side, appear in its surface. Under pixel style the reflection breaks into texel blocks that move with the water and line up with the probe term's blocks; under smooth it does not.
- [ ] Panning until a reflected mover leaves the screen fades its reflection with no hard line: to the probe term where the probe sees that water, to no reflection where it does not.
- [ ] Stepping the tier from high to low makes a smooth-style reflection coarser without shortening it. A pixel-style reflection keeps its texel blocks, with no screen-aligned stair-stepping. Off removes both the SSR and the probe reflection. Each change applies live, mid-level.
- [ ] The camera at the surface plane, the camera underwater, and a side face seen through glass show no reflection artifacts. Sludge reflects visibly less than water.
- [ ] GPU cost:
  - Measure on the GTX 1660 Super with `POSTRETRO_GPU_TIMING=1`, and on the RX 5300M Mac via Metal System Trace where timestamps are unsupported.
  - Time the reflection and fluid passes at each tier, under both styles, looking down at a pool that fills the view at 4K near.
  - Compare against the pre-change build. Its reflecting-minus-free fluid cost is ≈10 ms on the 5300M and 2.5 ms on the 1660 (`plans/done/fluid-base-pass-cost/`).
  - Off must match the pre-change reflection-free cost.
  - Report in the plan of record; the owner judges the numbers.

## Path
- **Insertion point.** `Renderer::record_scene_passes` (renderer_render_frame.rs), after "Skinned Mesh Pass" and before "Fluid Surface Pass".
  - The reflection pass iterates `FluidSurfaceSlots::frame_draws` and keeps only draws whose variant `reflections()`. It shares the install vertex and index buffers.
  - The volume order is far to near, done by the collector (render-cpu `fluid_surface/cull.rs`). Faces within a volume draw in install order.
  - It has no depth attachment at reduced resolution. Test occlusion by comparing against sampled scene depth.
- **Vertex entry.** `vs_main` reads group 3 for per-vertex baked light, which the reflection pass does not need. Give it a minimal vertex entry over locations 0–4, so its layout carries no SH group.
- **Pixel ray origin.** `fluid_reflection_cell_centre` builds texel axes from `dpdx`/`dpdy` of world position and UV. Compute them in the reflection fragment before the march loop. FXC rules forbid implicit-derivative sampling inside the loop.
- **Reduced-resolution depth.** Depth is at `prepass_attachment_extent`. The march `textureLoad`s at scaled coordinates. `fog.resize(…, &full.depth_view)` is the precedent.
- **Group 5.**
  - Add binding 2 to `fluid_scene_inputs_bind_group_layout_entries`.
  - Use `FluidSceneUniform.spare` for the target scale (0 under Off or with no target) and whatever the march needs per frame.
  - Only `fs_main_reflecting` references binding 2. Naga's Metal writer declares only the globals an entry uses (§7.3a Two pipelines), so the reflection-free variant gains no argument.
- **Upsample.** Pixel style reads the target nearest-neighbour by `textureLoad`. Smooth reads it bilinear through the existing linear-clamp sampler, which adds no sampler.
- **March snippet.** Keep it in its own WGSL module so `ssr-decals` can include it (§8). Give it a render-cpu reference beside the probe references.
- **IP rules in code.** Comment the no-direction rule beside the march, the s-independence rule beside the composite, and the keying rule beside the probe lookup, so a later change does not cross them unawares.
- **Option.** Follow the `fog_quality` / `surface_depth_quality` touch list end to end:
  - options `graphics.rs`, `mod.rs`, `persist.rs`, bridge `top_level.rs` / `mod.rs`;
  - `engine_state_catalog.rs`, `slot_table.rs` and the regenerated SDK `.d.ts` / `.d.luau` with typedef fixtures;
  - `frontend-menu.ts` / `.js` and `pause-menu.js`, plus the graphics-tab test in `main.rs`;
  - `startup/render_profile.rs`, `app/options_menu.rs`, `lifecycle.rs`, `splash_lifecycle.rs`;
  - the renderer setter, which feeds both the target and `shader_variant`.
- **Dev tuning.** Step budget and thickness can join the diagnostics panel's "SDF / Fog Quality" section (`debug_ui/mod.rs`). They are never player-facing.
- **Planar mirror, rejected.** It is the textbook water technique: re-render the scene from a camera mirrored about the water plane. It costs a scene pass per distinct water height. The mirrored camera sits inside solid geometry, which drops portal visibility to its frustum fallback (§2), and `resource_management.md` §9 rules out runtime render-to-texture for mirrors.
- **Strongest rival:** texel-snapped glints from lights the fluid pass already binds, after the `billboard-specular-shimmer` pattern, layered on the probe term. It costs no new pass or target, but it cannot mirror a mover or a character, which is the requested look. As a complement it stays a follow-up (Non-goals).
- **Inline march, rejected.** Marching inside `fs_main_reflecting` saves the pass and target. But the reflecting block's size already slows the always-run lighting on AMD Metal (§7.3a Two pipelines), and inline marching would lose the reduced-resolution tiers.
- **First slice.** The reflection pass at high on one pool with a mover, timed on the 5300M and the 1660 Super. It tests the riskiest assumption, that the march fits beside a reflecting variant already costing ≈10 ms on the 5300M, before any option plumbing.

## Boundary inventory

| Name | Rust | settings.toml / slot | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reflection quality | `PlayerOptions::reflection_quality` | key `reflection_quality`; slot `options.reflectionQuality`; values `"off"` / `"low"` / `"medium"` / `"high"` | slot string | slot string | n/a |
| Reflection style (landed) | `ModRenderProfile.reflections: ModReflectionProfile` | n/a | `render.reflections.style` | same | n/a |
| Fluid reflectivity (landed) | `FluidDescriptor` field; packed in `FluidParamsRow` | n/a | `surfaceReflectivity` | `surfaceReflectivity` | n/a |

## Open questions
- Whether low may also update at half frame rate — **delegated**: only if a capture of a fast camera turn shows no visible lag. Otherwise tiers vary resolution alone, and the plan of record says which.
- Whether the menu shows the tiers as a stepped slider or as the tab's existing enum buttons — **delegated**: match `fog_quality`'s control.
- Whether the reserved "reflection-probe streaming and seams" drafts (branch `claude/probe-streaming-brief-364a5e`) change group 4 or the probe gate before this builds — **delegated**: check that branch's status at build start. If it has landed, reread the gate and the budget row.
