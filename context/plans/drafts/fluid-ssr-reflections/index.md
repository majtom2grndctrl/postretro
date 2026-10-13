# fluid-ssr-reflections

Brief · resumable · reads: `context/lib/index.md` §1.1, §3 · `rendering_pipeline.md` §7.3a, §7.5, §8, §10, §13 · `player_options.md` §4 · `resource_management.md` §5 · read at e8f607b94 · builds after `fluid-surface-motion`

## Problem
The owner requested a capability: fluid surfaces reflect nothing, so water reads as a flat tinted sheet. Players will see the walls, lights and movers around a pool mirrored in its surface. A game author chooses whether reflections are pixelated or smooth as part of the game's art style. A player turns reflection quality down for cost, never past what the author chose, and can turn reflections off.

## Decisions
- **Screen-space reflections, not the documented baked cubemaps.** `index.md` §3 and `resource_management.md` §5 plan `env_cubemap` probes, but neither the bake tool nor the runtime exists. Cubemaps do not move with movers or dynamic lights, and they reflect a probe position rather than the pool's own view. SSR reflects exactly what surrounds a pool on screen. Cubemaps stay open as a later off-screen fallback. At promotion, the docs record SSR as the fluid reflection path.
- **A bounded march over the live depth buffer: a stated divergence from `index.md` §1.1**, which admits runtime ray work only against baked structures. The march reads scene depth, which is already resident and sampled by fog and forward, so it adds no acceleration structure. It runs only for visible, up-facing fluid fragments, at the player's quality tier, under a fixed step budget, and its cost is measured on the §10 perf floor before landing.
- **`surfaceReflectivity` widens fluid presentation, not the world material inventory.** `rendering_pipeline.md` §13 closes the authored material vocabulary and excludes image-based specular on world materials. This field is a fluid descriptor value beside `surfaceOpacity`, and no world material gains a reflection input.
- **The reflection resolves into its own target at the quality tier's resolution, before the fluid pass.** A reflection pass draws the frame's visible fluid faces into that target. It reads opaque scene color and scene depth directly: at that point neither is attached, and scene color holds exactly the opaque scene. No full-screen copy exists. The fluid pass then samples the reflection target at its own screen position. Fragment storage is at its 8/8 ceiling (§10), so the fluid pass takes the target through a sampled texture and a uniform only. The target lives with the scene targets and rebuilds with them on an extent or tier change.
- **What reflects.** Only a fluid face seen from its upward side reflects; side faces and any face seen from underwater do not. Reflections show what the opaque scene held when the reflection pass ran, so smoke, fog, the viewmodel and other fluid surfaces never appear. A ray that misses fades out (Fresnel × hit confidence × screen-edge fade). There is no off-screen fallback.
- **The author owns the style.** `render.reflections.style` is `"pixel"` or `"smooth"`, default `"pixel"`. Pixel snaps each ray's origin and perturbation to the water texel cells `fluid-surface-motion` introduces, so the reflection breaks into blocks that move with the water. Smooth marches per reflection-target pixel. The player never chooses the style. This matches bloom (`render.bloom.pixelated`) and fog (§7.5), where the author owns the look and the player owns cost.
- **The player owns cost: `reflection_quality` = off / low / medium / high, default high.** It is a standalone graphics option on the `surface_depth_quality` path (`player_options.md` §4): persisted, slot-exposed and applied live. High resolves at scene extent; medium at half; low at quarter. Off records no reflection pass, and its picture is the water without reflection.
  - The step budget scales with resolution, so a ray reaches equally far at every tier.
  - This ladder diverges on purpose from the Surface Depth ruling against tiers. That ruling rejected a tier that changed the shape of the effect at grazing angles. A lower tier here changes only the resolution of the whole reflection.
  - No umbrella quality preset is added.
- **Per-fluid reflectivity: `surfaceReflectivity`, 0..1, default 0.4.** Water reflects without authoring. The dev content's `sludge` sets it low. Existing mods' water gains reflections on upgrade, which is accepted before 1.0. It is local presentation like `surfaceOpacity`, so peers may differ and co-op compatibility is unaffected (`scripting.md` §2). A volume at 0 draws no reflection fragments.
- **Non-goals.**
  - Temporal reuse: a reflection updated below frame rate lags a turning camera without reprojection, and `motion-reprojection-layer` owns that facility.
  - SSR on non-fluid surfaces: `ssr-decals` stays separate and may reuse the march snippet.
  - Refraction and depth absorption, as E24 deferred them.

### Scripting surface
```ts
// Proposed design
export default defineMod({
  name: "neon-sump",
  id: "example.neon-sump",
  version: "1.0",
  render: {
    reflections: { style: "smooth" }, // optional; "pixel" (default) or "smooth"
  },
  fluids: [
    defineFluid({
      name: "water",
      viscosity: 2.0,
      viewTint: [0.1, 0.3, 0.4, 0.35],
      muffle: 0.8,
      surfaceOpacity: 0.6,
      surfaceReflectivity: 0.4, // optional; 0..=1, defaults to 0.4
    }),
  ],
});
```
The Luau mirror ships with the same names (`reflections = { style = "smooth" }`, `surfaceReflectivity = 0.4`).

## Acceptance
### Automated
- [ ] Off, a level with no fluid volumes, and a frame with no visible fluid draw each record no reflection pass.
- [ ] A fluid at reflectivity 0 draws exactly as under Off.
- [ ] Each tier resolves at its fraction of the scene extent. Changing tier or render resolution rebuilds the target, and the next frame binds the new one.
- [ ] The same ray reaches the same hit at high and low on a fixed synthetic depth buffer (GPU-free march reference): reach does not shrink with tier.
- [ ] Pixel style gives one ray per water texel cell. Smooth gives one per target pixel.
- [ ] `reflection_quality` round-trips through `settings.toml` and its slot for each of off, low, medium and high. An unrecognized stored value falls back for that field only.
- [ ] Manifest style:
  - omitted resolves to pixel;
  - `"smooth"` resolves to smooth;
  - an unknown string or wrong type warns and resolves to pixel without rejecting the manifest.
- [ ] The Scripting surface example parses in TS and its Luau mirror. `surfaceReflectivity`:
  - omitted resolves to 0.4;
  - 0 and 1 are accepted;
  - −0.1, 1.1 and NaN are rejected with a warning naming the field, and the declaration is otherwise kept.
- [ ] The player setting carries no style value, so the player cannot select smooth under a pixel game.
- [ ] Pipeline budget tests pass. Fluid fragment storage stays at 8, and sampled textures stay within the requested limit.
- [ ] The DX12 FXC shader test compiles the reflection and fluid shaders (Windows, ignored-gated run).

### Manual
- [ ] Walls, lights and a moving mover beside a pool appear in its surface. Under pixel style the reflection breaks into texel blocks that move with the water; under smooth it does not.
- [ ] Stepping the tier from high to low makes the reflection coarser without shortening it. Off removes it. Each change applies live, mid-level.
- [ ] A ray leaving the screen edge fades with no hard line. The camera at the surface plane, the camera underwater, and a side face seen through glass show no reflection artifacts. Sludge reflects visibly less than water.
- [ ] GPU cost on the GTX 1660 Super with `POSTRETRO_GPU_TIMING=1`, and on the RDNA1 Mac via Metal System Trace where timestamps are unsupported. Measure the reflection and fluid passes at each tier, under both styles, looking down at a pool that fills the view; compare Off with the pre-change build. Report in the plan of record; the owner judges the numbers.

## Path
- **Insertion point.** `record_scene_passes` (renderer_render_frame.rs), after skinned meshes and before fluid. The reflection pass reuses the fluid draw list and vertex buffers from `FluidSurfacePass`.
  - It has no depth attachment at reduced resolution. Test occlusion by comparing against sampled scene depth.
  - Overlapping faces resolve by draw order: draws are already sorted far to near.
- **Pipeline pieces.**
  - The fluid pipeline's group 5 is free (groups 0–4 used; device grants 8).
  - The fluid pass attaches depth Load/Store today. It does not need to change for this design, because only the reflection pass reads depth.
  - Scene targets rebuild in `rebuild_scene_targets` (renderer_extent.rs).
- **Upsample.** Pixel style reads the target nearest-neighbour; smooth reads it bilinear.
- **March snippet.** Keep it in its own WGSL module so `ssr-decals` can include it (§8). FXC rules apply: no implicit-derivative sampling inside the march loop.
- **Option and style.**
  - Follow the `surface_depth_quality` touch list end to end: options enum and keys, `persist.rs`, `engine_state_catalog.rs` with regenerated SDK types and typedef fixtures, the bridge, `frontend-menu.ts` with the graphics-tab id/count test in `main.rs`, `render_profile.rs`, `app/options_menu.rs`, the renderer setter, and boot and level re-apply.
  - The style parses beside `ModBloomProfile` and maps next to `renderer_bloom_profile`.
- **Dev tuning.** Step budget and thickness can join the diagnostics panel's SDF / Fog Quality section. They are never player-facing.
- **Strongest rival:** Fresnel-weighted reads from the baked SH probes plus texel-snapped glints from lights the fluid pass already binds, after the `billboard-specular-shimmer` pattern. It costs no new pass or target, but it cannot mirror a wall or a mover, which is the requested look. It remains the natural fallback where a ray misses, if misses prove too dark.
- **First slice.** The reflection pass at high on one pool, timed on the 1660 Super. It falsifies the riskiest assumption, that the march fits the budget, before any option plumbing.

## Boundary inventory

| Name | Rust | settings.toml / slot | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reflection quality | `PlayerOptions::reflection_quality` | key `reflection_quality`; slot `options.reflectionQuality`; values `"off"` / `"low"` / `"medium"` / `"high"` | slot string | slot string | n/a |
| Reflection style | field on `ModRenderProfile` beside `bloom` | n/a | `render.reflections.style`: `"pixel"` / `"smooth"` | same | n/a |
| Fluid reflectivity | field on `FluidDescriptor` | n/a | `surfaceReflectivity` | `surfaceReflectivity` | n/a |

## Open questions
- Whether low may also update at half frame rate — **delegated**: only if a capture of a fast camera turn shows no visible lag. Otherwise tiers vary resolution alone, and the plan of record says which.
- Whether the menu shows the tiers as a stepped slider or as the tab's existing enum buttons — **delegated**: match the graphics tab's current enum controls unless a stepped slider widget already exists.
- Whether a hot-reloaded manifest applies a changed style live or at the next level install — **delegated**: follow `render.bloom`, and report it.
