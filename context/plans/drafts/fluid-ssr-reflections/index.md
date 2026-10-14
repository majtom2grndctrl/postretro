# fluid-ssr-reflections

Brief · resumable · reads: `context/lib/index.md` §1.1, §3 · `rendering_pipeline.md` §7.3a, §7.5, §8, §10, §13 · `player_options.md` §4 · `resource_management.md` §5 · read at e8f607b94 · builds after `fluid-surface-motion`, `fluid-depth-fog-waves` and `reflection-probes`

## Problem
The owner requested a capability: fluid surfaces reflect nothing, so water reads as a flat tinted sheet. Players will see the walls, lights and movers around a pool mirrored in its surface. A game author chooses whether reflections are pixelated or smooth as part of the game's art style. A player turns reflection quality down for cost, never past what the author chose, and can turn reflections off.

## Decisions
- **Screen-space reflections over the baked probe term.** `reflection-probes` gives water a box-projected reflection of its static room (`rendering_pipeline.md` §7.3a), but its probes do not hold movers, characters or dynamic lights. SSR reflects exactly what surrounds a pool on screen and composites over the probe term. At promotion, the docs record SSR as the dynamic fluid reflection layer.
- **`index.md` §1.1 is amended, and this brief is its first approved exception.** The rule becomes a strong preference for tracing baked structures at runtime. An exception needs owner approval and a use case with no reasonable workaround within pre-RTX constraints. Fluid SSR qualifies:
  - Mirroring nearby walls, lights and movers has no baked equivalent that moves with the scene.
  - The march reads scene depth, which fog and forward already sample, so it adds no acceleration structure.
  - It runs only for visible, up-facing fluid fragments, at the player's quality tier, under a fixed step budget.
  - Its cost is measured on the §10 perf floor before landing.

  The amendment lands at promotion. `ssr-decals` would need its own approval.
- **`surfaceReflectivity` widens fluid presentation, not the world material inventory.** `rendering_pipeline.md` §13 closes the authored material vocabulary and excludes image-based specular on world materials. This field is a fluid descriptor value beside `surfaceOpacity`, and no world material gains a reflection input.
- **The reflection resolves into its own target at the quality tier's resolution, before the fluid pass.** A reflection pass draws the frame's visible fluid faces into that target. It reads opaque scene color and scene depth directly: at that point neither is attached, and scene color holds exactly the opaque scene. No full-screen copy exists. The fluid pass samples the target at its own screen position under smooth style. Under pixel style it samples at the projected centre of its water texel cell, so a reduced tier never lays a screen-locked grid over the water's blocks. Fragment storage is at its 8/8 ceiling (§10), so the fluid pass takes the target through a sampled texture and a uniform only. Both join group 5, the fluid scene-inputs group `fluid-depth-fog-waves` opens for scene depth. The target lives with the scene targets and rebuilds with them on an extent or tier change. It exists only while a level with fluid surfaces is installed and reflections are not Off, which keeps §7.3a's rule that a level without surfaces allocates nothing.
- **What reflects.** Only a fluid face seen from its upward side reflects; side faces and any face seen from underwater do not. Reflections show what the opaque scene held when the reflection pass ran, so smoke, fog, billboards, projectile bodies, the viewmodel and other fluid surfaces never appear. These are accepted limits. A ray that misses fades (hit confidence × screen-edge fade) to the probe term rather than going dark; a face bound to no probe fades to no reflection.
- **The reflection sits on the surface, above the fog.** The fluid composite is surface over depth fog over scene (`fluid-depth-fog-waves`). The reflection takes the probe term's Fresnel weight, and `reflection-probes` settles the composite for both layers: `F·R + (1−F)·(surface over fog over scene)` at coverage `F + (1−F)·a`, so neither opacity nor fog dims it.
- **The author owns the style.** `render.reflections.style`, defined by `reflection-probes`, is `"pixel"` or `"smooth"`, default `"pixel"`, and governs both layers. Pixel snaps each ray's origin and perturbation to the water texel cells `fluid-surface-motion` introduces, so the reflection breaks into blocks that move with the water. Smooth marches per reflection-target pixel. The player never chooses the style. This matches bloom (`render.bloom.pixelated`) and fog (§7.5), where the author owns the look and the player owns cost.
- **The player owns cost: `reflection_quality` = off / low / medium / high, default high.** The owner chose this four-tier ladder; first-slice measurements tune what each tier costs, not whether it exists. It is a standalone graphics option on the `surface_depth_quality` path (`player_options.md` §4): persisted, slot-exposed and applied live. High resolves at scene extent; medium at half; low at quarter. Off records no reflection pass, and its picture is the water without reflection.
  - The step budget scales with resolution, so a ray reaches equally far at every tier.
  - This ladder diverges on purpose from the Surface Depth ruling against tiers. That ruling rejected a tier that changed the shape of the effect at grazing angles. A lower tier here changes only the resolution of the whole reflection.
  - No umbrella quality preset is added.
- **Per-fluid reflectivity: `surfaceReflectivity`, defined by `reflection-probes`.** It scales Fresnel base reflectance, 0..=50, default 1 = water (`scripting.md` §2); this brief's former raw 0..1 weight with default 0.4 is superseded. The dev content's `sludge` sets it low. A volume at 0 draws no reflection fragments.
- **Non-goals.**
  - Temporal reuse: a reflection updated below frame rate lags a turning camera without reprojection, and `motion-reprojection-layer` owns that facility.
  - SSR on non-fluid surfaces: `ssr-decals` stays separate and may reuse the march snippet.
  - Refraction, as E24 deferred it. Depth fog seen from outside belongs to `fluid-depth-fog-waves`; underwater fog stays out of both.

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
      surfaceReflectivity: 1, // optional; 0..=50, default 1 (water); defined by reflection-probes
    }),
  ],
});
```
The Luau mirror ships with the same names (`reflections = { style = "smooth" }`, `surfaceReflectivity = 1`).

## Acceptance
### Automated
- [ ] Off, a level with no fluid volumes, and a frame with no visible fluid draw each record no reflection pass.
- [ ] Off and a level with no fluid surfaces allocate no reflection target. Switching from Off to a tier mid-level creates it, and switching back releases it.
- [ ] A fluid at reflectivity 0 draws exactly as under Off.
- [ ] Each tier resolves at its fraction of the scene extent. Changing tier or render resolution rebuilds the target, and the next frame binds the new one.
- [ ] The same ray reaches the same hit at high and low on a fixed synthetic depth buffer (GPU-free march reference): reach does not shrink with tier.
- [ ] Pixel style gives one ray per water texel cell, and every fragment of a cell reads the same reflection value at every tier. Smooth gives one ray per target pixel.
- [ ] `reflection_quality` round-trips through `settings.toml` and its slot for each of off, low, medium and high. An unrecognized stored value falls back for that field only.
- [ ] Manifest style:
  - omitted resolves to pixel;
  - `"smooth"` resolves to smooth;
  - an unknown string or wrong type warns and resolves to pixel without rejecting the manifest.
- [ ] The Scripting surface example parses in TS and its Luau mirror. `surfaceReflectivity` parsing is `reflection-probes`' acceptance.
- [ ] The player setting carries no style value, so the player cannot select smooth under a pixel game.
- [ ] Pipeline budget tests pass. Fluid fragment storage stays at 8. Sampled textures, 9 before the reflection target joins group 5 (`reflection-probes` adds the probe array), stay within the requested 16.
- [ ] The DX12 FXC shader test compiles the reflection and fluid shaders (Windows, ignored-gated run).

### Manual
- [ ] Walls, lights and a moving mover beside a pool appear in its surface. Under pixel style the reflection breaks into texel blocks that move with the water; under smooth it does not.
- [ ] Stepping the tier from high to low makes a smooth-style reflection coarser without shortening it. A pixel-style reflection keeps its texel blocks, with no screen-aligned stair-stepping. Off removes it. Each change applies live, mid-level.
- [ ] A ray leaving the screen edge fades with no hard line. The camera at the surface plane, the camera underwater, and a side face seen through glass show no reflection artifacts. Sludge reflects visibly less than water.
- [ ] GPU cost on the GTX 1660 Super with `POSTRETRO_GPU_TIMING=1`, and on the RDNA1 Mac via Metal System Trace where timestamps are unsupported. Measure the reflection and fluid passes at each tier, under both styles, looking down at a pool that fills the view; compare Off with the pre-change build. Report in the plan of record; the owner judges the numbers.

## Path
- **Insertion point.** `record_scene_passes` (renderer_render_frame.rs), after skinned meshes and before fluid. The reflection pass reuses the fluid draw list and vertex buffers from `FluidSurfacePass`.
  - It has no depth attachment at reduced resolution. Test occlusion by comparing against sampled scene depth.
  - Overlapping faces resolve by draw order: draws are already sorted far to near.
- **Pipeline pieces.**
  - `fluid-depth-fog-waves` opens group 5 for scene depth, opens the fluid pass's depth read-only (`depth_ops: None`), and rebuilds group 5 with the scene targets. The reflection target and uniform join it (device grants 8 groups).
  - The reflection pass reads depth as a sampled texture with no attachment.
  - Scene targets rebuild in `rebuild_scene_targets` (renderer_extent.rs).
- **Upsample.** Pixel style reads the target nearest-neighbour; smooth reads it bilinear.
- **March snippet.** Keep it in its own WGSL module so `ssr-decals` can include it (§8). FXC rules apply: no implicit-derivative sampling inside the march loop.
- **Option and style.**
  - Follow the `surface_depth_quality` touch list end to end: options enum and keys, `persist.rs`, `engine_state_catalog.rs` with regenerated SDK types and typedef fixtures, the bridge, `frontend-menu.ts` with the graphics-tab id/count test in `main.rs`, `render_profile.rs`, `app/options_menu.rs`, the renderer setter, and boot and level re-apply.
  - The style parses beside `ModBloomProfile` and maps next to `renderer_bloom_profile`.
- **Dev tuning.** Step budget and thickness can join the diagnostics panel's SDF / Fog Quality section. They are never player-facing.
- **Planar mirror, rejected.** It is the textbook water technique: re-render the scene from a camera mirrored about the water plane. It costs a scene pass per distinct water height, and the mirrored camera sits inside solid geometry, which drops portal visibility to its frustum fallback (§2).
- **Strongest rival:** Fresnel-weighted reads from the baked SH probes plus texel-snapped glints from lights the fluid pass already binds, after the `billboard-specular-shimmer` pattern. It costs no new pass or target, but it cannot mirror a wall or a mover, which is the requested look. It remains the natural fallback where a ray misses, if misses prove too dark.
- **First slice.** The reflection pass at high on one pool, timed on the 1660 Super. It falsifies the riskiest assumption, that the march fits the budget, before any option plumbing.

## Boundary inventory

| Name | Rust | settings.toml / slot | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reflection quality | `PlayerOptions::reflection_quality` | key `reflection_quality`; slot `options.reflectionQuality`; values `"off"` / `"low"` / `"medium"` / `"high"` | slot string | slot string | n/a |
| Reflection style | field on `ModRenderProfile` beside `bloom` | n/a | `render.reflections.style`: `"pixel"` / `"smooth"` | same | n/a |
| Fluid reflectivity | field on `FluidDescriptor` | n/a | `surfaceReflectivity` | `surfaceReflectivity` | n/a |

## Open questions
- Whether pixel-style reflection blocks follow the wave-displaced cells (`fluid-depth-fog-waves`) or the unwarped texel grid — owner: this brief's drafting session — **blocks build**
- How the reflection target binds in the shared fluid group 5, which `fluid-depth-fog-waves` builds at init and rebuilds only with the scene targets, while the target exists only on a fluid level with reflections not Off: a placeholder binding, or a group-5 rebuild on install and tier change — **blocks build**
- Whether low may also update at half frame rate — **delegated**: only if a capture of a fast camera turn shows no visible lag. Otherwise tiers vary resolution alone, and the plan of record says which.
- Whether the menu shows the tiers as a stepped slider or as the tab's existing enum buttons — **delegated**: match the graphics tab's current enum controls unless a stepped slider widget already exists.
- Whether a hot-reloaded manifest applies a changed style live or at the next level install — **delegated**: follow `render.bloom`, and report it.
