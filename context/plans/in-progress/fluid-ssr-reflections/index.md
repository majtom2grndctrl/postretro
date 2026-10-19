# fluid-ssr-reflections

Brief · resumable · reads: `context/lib/index.md` §1.1, §3 · `rendering_pipeline.md` §7.3a, §7.5, §8, §10, §13 · `player_options.md` §4 · `resource_management.md` §5 · `plans/done/reflection-probes/` (`ip-check.md`, `research.md` §IP) · this plan's `ip-check.md` · read at 99f0601c6; Decisions re-verified at 3d03f724b · `fluid-surface-motion`, `fluid-depth-fog-waves`, `reflection-probes` and `fluid-base-pass-cost` have landed; independent of `fluid-surface-t-junction-closure` (compiler-only)

## Problem
The owner requested a capability. Fluid surfaces now reflect the static room their probe captured (`rendering_pipeline.md` §7.3a, Reflection), but movers, characters and dynamic lights never appear in them. Players will see the walls, lights and movers around a pool mirrored in its surface, and objects standing in the water reflected on every side the camera sees. Screen space cannot reflect a surface the camera never sees, such as the underside of a dock seen from above, so this brief does not cover it (Non-goals). A game author already chooses pixelated or smooth reflections (`render.reflections.style`). A player turns reflection quality down for cost, never past what the author chose, and can turn reflections off.

## Decisions
- **Screen-space reflections, always composited over the probe term and weighted by confidence.** Each layer covers what the other cannot see:
  - SSR owns movers, characters, dynamic lights, and the camera-facing sides of objects standing in the water.
  - The probe owns the static room off screen.
  - On every reflecting fragment the two blend by SSR confidence c = hit confidence × screen-edge fade, and nothing else.
  - **No direction term.** c reads no ray-direction quantity. The march never exits early, skips, re-weights or changes step by direction; a ray heading toward the camera marches like any other. Per-pixel technique choice by direction is the Microsoft US10586375B2 pattern (`ip-check.md` §3).
  - **SSR never reads the capture shadow.** s, the probe's capture-shadow weight, scales the probe's share only, so SSR's contribution F·c·SSR is identical at s = 0 and s = 1. The reflection pass reads no s, distance tile or probe colour; it may read the probe table's box and capture point, which the pixel cell point needs. Nothing is added when the probe's obstruction test fires, which keeps clear of Tencent claim 15 (`ip-check.md` §1).
  - **The probe lookup ignores the SSR hit.** It is keyed to its own search from the fragment or cell point, never to the SSR hit, its depth or its screen position. This distinguishes it from NetEase CN114748874B (`ip-check.md` §4).

  This is the layering §7.3a Limits anticipates ("composites over this term and falls back to it on a miss"). It is not the capture-shadow-triggered fill that §7.3a and `reflection-probes/ip-check.md` (claim 15 row) reserve for IP review. §7.3a records it as the screen-space layer, with s scaling the probe term only.
- **Owner claims check before shipping.** `ip-check.md` finds no live claim that reads on this design while the layering rules above hold. Open items are NetEase CN114748874B (granted, China only, claim text unverified) and Microsoft EP3756162 (granted text unread). The owner's pre-ship claims check extends to this layer with the items in `ip-check.md` §9. It gates shipping, not building.
- **`index.md` §1.1 gains a third admitted category of runtime ray work.** §1.1 stays a rule. Beside offline bake rays and cheap traces against baked structures, it admits *a bounded screen-space march against the current frame's depth buffer, on gated fragments, under a fixed step budget*. Each use still needs owner sign-off in its own brief. Fluid SSR is the first: the probe term is the baked equivalent for everything static, and movers and dynamic lights have none. The march adds no acceleration structure, runs only where the reflecting gate admits, and is measured on the §10 perf floor before landing. `ssr-decals` fits the category but needs its own sign-off.
- **No new material or author input.** `surfaceReflectivity` scales both layers through the same Fresnel. No world material gains a reflection input (§13). The only new name is the player option (Boundary inventory).
- **SSR reuses the probe term's gate.** A fragment gets SSR only where the landed reflecting gate admits it: per draw, the reflecting variant `FluidParamsRow::shader_variant` picks; per fragment, `fluid_reflection_gate`. A face bound to no probe, a down-facing face, reflectivity 0 and any lighting-mask isolation draw exactly as without reflection. The reflection-free variant never reads the SSR target, so it keeps its cost.
- **The reflection resolves into its own target, before the fluid pass.**
  - **Reflection pass.** It draws the frame's reflecting fluid draws into the target, reading opaque scene colour and scene depth. It shades water hidden behind opaque objects too, so a pixel-style cell partly hidden behind an object standing in the water still has its own value. Where fluid surfaces overlap, the nearest wins: the pass depth-tests its draws against each other, never against scene depth. A fragment the gate refuses writes nothing, and a texel no fragment writes this frame reads as a miss (c = 0).
  - **Fluid pass.** It samples the target, bound in group 5, only in the reflecting variant; fragment storage stays at 8/8 (§10). Smooth samples at its own screen position. Pixel samples at the projected centre of its water texel cell, so a reduced tier never lays a screen-locked grid over the water's blocks. The reflection pass never binds group 5, which holds its own target.
- **The target exists only on levels that can reflect.** A 1×1 placeholder is bound at init. The real target is created when a level with a probe-bound fluid face installs at a tier other than Off, or when the tier leaves Off on such a level, and released on Off or on unload. Probe binding is fixed at install; reflectivity is not (a staged reload or tuning payload retunes it live), so it only switches draw variants and never creates or releases the target. Group 5 is rebuilt at those points and with the scene targets, never per frame. Pin P4 widens deliberately to name exactly these points.
- **What reflects.** Reflections show what the opaque scene held when the reflection pass ran. Smoke, fog, billboards, projectile bodies, the viewmodel and other fluid surfaces never appear. These are accepted limits.
- **Composite.** With F the Fresnel × fog-proximity weight (unshadowed), s the probe's capture-shadow weight, and c the SSR confidence:
  - reflected colour = `F·[c·SSR + (1−c)·s·Probe]`;
  - its weight W = `F·[c + (1−c)·s]`;
  - the fragment = reflected colour + `(1−W)·(surface over fog over scene)` at coverage `W + (1−W)·a`.

  At c = 0 this is exactly the landed composite (`F·s` is today's F), so a miss, Off and a fragment SSR never reached all draw today's picture. Opacity and fog never dim either layer. The composite joins the GPU-free CPU reference in render-cpu, mirrored statement for statement (§7.3a References).
- **The author owns the style.** SSR reads the landed `render.reflections.style`. Pixel takes each ray's origin and direction from the probe term's cell point (`fluid_reflection_point`), so both layers break into the same blocks. Smooth marches per target pixel about the flat normal. The player never chooses the style.
- **The player owns cost: `reflection_quality` = off / low / medium / high, default high.**
  - **Off turns off both layers.** Every draw takes the reflection-free variant, so the player recovers the probe term's cost too. No reflection pass is recorded.
  - **Low, medium and high** keep the probe term unchanged. SSR resolves at a per-axis fraction of the scene extent, starting at quarter, half and full; first-slice measurements may move the fractions, not the ladder. Every tier marches the same full-resolution depth stride and step budget, so a lower tier traces fewer rays and reach and thin occluders stay identical.
  - **Precedent.** It reuses `fog_quality`'s plumbing, not its policy: in fog the map owns resolution and the player owns step size; here the player owns resolution and the march is fixed.
  - **Divergence from the Surface Depth ruling.** That ruling (`player_options.md` §4) gave Surface Depth two states because its middle tier was a pure cost lever that only looked worse at grazing angles. This ladder is also mostly a cost lever: under pixel style, the default, low through high look alike until water cells shrink below a target pixel with distance. Four tiers stand anyway: a lower tier changes only how many rays are traced, and the reflecting fluid pass already costs ≈10 ms on the 5300M floor.
- **Sequencing.** SSR builds before the roadmap's undrafted "reflection-probe fluid-pass performance" item, which then optimizes the combined composite once.
- **Non-goals.**
  - Temporal reuse, including a half-rate tier: a reflection updated below frame rate lags a turning camera without reprojection. `motion-reprojection-layer` owns that facility, and reflection-probes §IP lists reflection history under Avoid.
  - SSR on non-fluid surfaces: `ssr-decals` stays separate and may reuse the march snippet.
  - SSR on faces the probe gate refuses, such as a face bound to no probe: such a face has no term to fall back to on a miss, so SSR alone would pop at every screen edge. Revisit only if a level shows up-facing water with no derived probe.
  - Surfaces the camera never sees, such as a dock's underside seen from above. An authored `reflection_volume` whose `_capture_offset` shifts C horizontally to beneath the deck (C stays 2 mm above the water) may cover them where the deck clears the water (`resource_management.md` §5).
  - Glints from dynamic lights, after `billboard-specular-shimmer`: a possible follow-up for lights off screen or drawn only as billboards.
  - Refraction, as E24 deferred it.

## Acceptance
Pins (P-numbers) are in `research.md`.

### Automated
- [ ] Off, a level with no fluid volumes, and a frame with no reflecting draw each record no reflection pass. A frame with a reflecting draw at low, medium or high records exactly one.
- [ ] Off makes every draw take the reflection-free variant: `shader_variant` returns it for a probe-bound volume at reflectivity 1 under the all-terms mask when the option is Off, and the reflecting variant at every other tier.
- [ ] Source-order test: the reflection pass records after the skinned mesh pass and before the fluid pass, and both passes take their variants from the frame's one mask snapshot and one tier value (P9, P13).
- [ ] Composite CPU reference:
  - at c = 0 it equals the landed composite for every s, F and a;
  - at c = 1 the probe contributes nothing;
  - on the whole composite's premultiplied output, the change when SSR goes from 0 to a colour X is F·c·X, equal within float tolerance at s = 0 and s = 1 over a sweep of F, c and a;
  - the reflected colour is the same at every opacity a and fog extinction for fixed F, c, s, SSR and Probe.
- [ ] GPU-free march reference, on a fixed synthetic depth buffer:
  - the same ray reaches the same hit at high and low, including a thin occluder one depth texel wide;
  - a ray leaving the screen or exhausting its step budget gets c = 0, and one ending in the edge fade band gets 0 < c < 1;
  - a ray heading toward the camera takes the same screen-space step length as one heading away until it hits, leaves the screen or leaves the depth range, then ends as a miss by the same rule (P11);
  - a ray whose origin lies behind an occluder does not hit that occluder at its start (P15).
- [ ] The WGSL composite and march match their CPU references body for body, on the existing transcription-pin pattern.
- [ ] Pixel style gives one ray per water texel cell, from the same cell point and direction as `fluid_reflection_point`. Smooth gives one ray per target pixel.
- [ ] The reflection pass's pipeline layout binds no probe colour array, distance array or group 5, and its entry reaches no capture-shadow function. It may bind the probe table, whose box and capture point the pixel cell point needs, and read the face's probe index and the volume's reflectivity for the gate (P12). The reflection target joins the variant test's reflection globals: `fs_main` reaches it nowhere and its `global_uses` omit it; `fs_main_reflecting` reaches it.
- [ ] IP gates (structural, not proofs):
  - source scan: the march's confidence takes hit confidence and screen position alone, and its step length and exit tests read no ray- or view-direction value;
  - grep gate and naga walk: the target holds colour and confidence alone, and in the fluid shader no probe search or lookup function reaches the target's global.
- [ ] `reflection_quality` round-trips through `settings.toml` and its slot for each of off, low, medium and high. An absent key loads as high, and an unrecognized stored value falls back for that field only. The graphics-tab id/count test in `main.rs` counts the new control.
- [ ] Grep gates: no `PlayerOptions` field, settings key or options slot names a reflection style; no world material, FGD key or texture-prefix rule gains a reflection input.
- [ ] Pipeline budget tests pass with group 5's per-group row updated: fluid fragment storage stays at 8, sampled textures go from 10 to 11 of 16, and uniforms and samplers stay at their pinned counts.
- [ ] The reflection pipeline is built at full init at every tier, so the DX12 FXC shader test compiles it with both fluid variants (Windows, ignored-gated run, log shows the DX12 case ran); raising the tier compiles nothing (P14).

### Automated, adapter-backed
Each row's run log shows the GPU case ran, not skipped.
- [ ] A horizontal pool bound to a probe whose capture is a uniform colour, with an opaque quad of another colour standing in it in view, reflects the quad. At high in smooth style, fragments whose CPU march reference hits the quad read the CPU composite within 2/255 and differ from the same frame at Off; at low the same fragments still read the quad. A face seen from below, a face bound to no probe and an isolating mask each read the reflection-free frame byte for byte. Recording both passes raises no validation error (P12).
- [ ] Pixel style, with SSR hits in view: every fragment of one cell reads one reflection value at low, medium and high, and those cells differ from the same frame at Off. A cell partly hidden behind the quad reads its own cell's SSR, not a hit on the quad's back, at every visible fragment; a cell whose centre is off screen or past the face's edge reads c = 0 (P15).
- [ ] Two reflecting surfaces whose draws share target texels, the visible one drawn first: every visible fragment reads its own surface's SSR (P16).
- [ ] A staged reload raising a probe-bound volume's reflectivity from 0 shows SSR on the next frame without rebuilding the target or group 5; lowering it to 0 draws the reflection-free variant (P17).
- [ ] A texel no reflecting draw wrote this frame reads c = 0, never an earlier frame's hit: a frame whose hits reach the water's edge, then a frame with the camera moved so those texels lie off the water, equals that second frame drawn by a fresh renderer (P7). A smooth fragment whose bilinear footprint straddles a written and an unwritten texel reads between the probe-only and the SSR composite, never darker than both.
- [ ] Off, a level with no fluid volumes and a fluid level with no probe-bound face allocate only the 1×1 placeholder at every tier. On a level with a probe-bound fluid face, Off → tier creates the target and rebinds group 5, and tier → Off rebinds the placeholder before the next frame and releases the target (P6). Raising the tier with no fluid level allocates nothing until a fluid level installs (P3); unload rebinds the placeholder (P4); setting the tier it already holds builds nothing (P5); full init or resume with Off stored allocates only the placeholder at the first fluid install.
- [ ] Both Pin P4 tests, the source pin and the adapter case, name exactly the widened set of group 5 rebuild points: full init, the scene-target rebuild, a fluid install or tier change that creates or drops the target, and unload.
- [ ] Each tier resolves at its per-axis fraction of the scene extent, rounded up and at least 1×1. Tier and render resolution changed on one tick: the next frame binds one target at the new tier's fraction of the new extent, and no group 5 build references a superseded target or scene colour (P1, P2).
- [ ] Switching style between two frames, with SSR hitting: the first frame after the switch equals a steady-state frame of the new style byte for byte (P8).
- [ ] With SSR hits in view, the camera 1 mm below the plane draws the same frame as at Off byte for byte (P10).

### Manual
- [ ] Walls, lights, a moving mover, and a pillar standing in a pool, including the pillar's camera-facing side, appear in its surface. Under pixel style the reflection breaks into texel blocks that move with the water and line up with the probe term's blocks, with no gap between a mover's base and its reflection; under smooth it does not break into blocks.
- [ ] Panning until a reflected mover leaves the screen fades its reflection with no hard line: to the probe term where the probe sees that water, to no reflection where it does not.
- [ ] Stepping the tier from high to low makes a smooth-style reflection coarser without shortening it or losing thin objects. A pixel-style reflection keeps its texel blocks, with no screen-aligned stair-stepping. Off removes both the SSR and the probe reflection. Each change applies live, mid-level.
- [ ] The camera at the surface plane, the camera underwater, and a side face seen through glass show no reflection artifacts. Sludge reflects visibly less than water.
- [ ] GPU cost:
  - Measure on the GTX 1660 Super with `POSTRETRO_GPU_TIMING=1`, and on the RX 5300M Mac via Metal System Trace where timestamps are unsupported.
  - Time the reflection and fluid passes as labelled pairs at each tier, under both styles, looking down at a pool that fills the view at 4K near.
  - Compare against the pre-change build. Its reflecting-minus-free fluid cost is ≈10 ms on the 5300M and 2.5 ms on the 1660 (`plans/done/fluid-base-pass-cost/`).
  - Off must match the pre-change reflection-free cost.
  - Report in the plan of record; the owner judges the numbers.

## Path
- **Shape:** a separate reduced-resolution reflection pass feeding the reflecting fluid variant. **Strongest rival:** texel-snapped glints from lights the fluid pass already binds; no new pass or target, but it cannot mirror a mover or a character (a follow-up, Non-goals). Planar mirror is rejected: a scene pass per water height from a camera inside solid geometry, and `resource_management.md` §9 rules out runtime render-to-texture for mirrors. An inline march in `fs_main_reflecting` is rejected: the reflecting block's size already slows the always-run lighting on AMD Metal (§7.3a Two pipelines), and it would lose the reduced-resolution tiers.
- **Insertion point.** `Renderer::record_scene_passes` (renderer_render_frame.rs), between "Skinned Mesh Pass" and "Fluid Surface Pass". It iterates `FluidSurfaceSlots::frame_draws`, keeps draws whose variant `reflections()`, and shares the install vertex and index buffers. Volumes arrive far to near (render-cpu `fluid_surface/cull.rs`).
- **Reflection pass inputs.** Its own bind group carries scene depth, scene colour, the style, the target scale and dev tuning; group 5 holds its target, and both rebuild with the scene targets. A target-resolution depth attachment of its own, cleared each frame, orders fluid against fluid; scene depth never occludes. A hidden origin starts behind its occluder; the march must not count that starting overlap as a hit. Clear the target to c = 0 each frame, and build the pipeline at full init.
- **Vertex entry.** `vs_main` reads group 3 for per-vertex baked light, which the reflection pass does not need. Give it a minimal vertex entry over locations 0–4.
- **Pixel ray origin.** `fluid_reflection_cell_centre` builds texel axes from `dpdx`/`dpdy`; compute them before the march loop (FXC forbids implicit-derivative sampling inside it). Shimmer and ring tilt depend on `texels_per_pixel`, which the reflection pass measures at its own resolution; scale it to scene resolution so both layers tilt the same cells.
- **Reduced-resolution depth.** Depth is at `prepass_attachment_extent`. The march `textureLoad`s full-resolution depth at full-resolution stride. `fog.resize(…, &full.depth_view)` is the precedent.
- **Group 5.** Add binding 2 to `fluid_scene_inputs_bind_group_layout_entries`; `FluidSceneUniform.spare` carries the target scale (0 under Off or with no target). Only `fs_main_reflecting` references binding 2. Naga's Metal writer declares only the globals an entry uses, so the reflection-free variant gains no argument.
- **Upsample.** Pixel reads the target nearest by `textureLoad`. Smooth reads it bilinear through the existing linear-clamp sampler.
- **March snippet.** Its own WGSL module, so `ssr-decals` can include it (§8), with a render-cpu reference beside the probe references.
- **IP rules in code.** Comment the no-direction rule beside the march, the s-independence rule beside the composite, and the keying rule beside the probe lookup, so a later change does not cross them unawares.
- **Option.** Follow the `fog_quality` / `surface_depth_quality` touch list end to end: options, bridge, state catalog and slot table with regenerated SDK typedefs, the frontend and pause menus with the graphics-tab test, render profile, lifecycle and splash. Unlike `set_fog_step_size`, the renderer setter creates, resizes or releases the target, rebuilds group 5, and feeds `shader_variant`.
- **Dev tuning.** Step budget and thickness can join the diagnostics panel's "SDF / Fog Quality" section (`debug_ui/mod.rs`). They are never player-facing.
- **First slice.** The reflection pass at high on one pool with a mover, timed on the 5300M and the 1660 Super, before any option plumbing. It tests the riskiest assumption: that the march fits beside a reflecting variant already costing ≈10 ms on the 5300M.

## Boundary inventory

| Name | Rust | settings.toml / slot | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reflection quality | `PlayerOptions::reflection_quality` | key `reflection_quality`; slot `options.reflectionQuality`; values `"off"` / `"low"` / `"medium"` / `"high"` | slot string | slot string | n/a |
| Reflection style (landed) | `ModRenderProfile.reflections: ModReflectionProfile` | n/a | `render.reflections.style` | same | n/a |
| Fluid reflectivity (landed) | `FluidDescriptor` field; packed in `FluidParamsRow` | n/a | `surfaceReflectivity` | `surfaceReflectivity` | n/a |

## Open questions
- Whether the menu shows the tiers as a stepped slider or as the tab's existing enum buttons — **delegated**: match `fog_quality`'s control.
- Whether the reserved "reflection-probe streaming and seams" drafts (branch `claude/probe-streaming-brief-364a5e`) change group 4 or the probe gate before this builds — **delegated**: check that branch's status at build start. If it has landed, reread the gate and the budget row.
