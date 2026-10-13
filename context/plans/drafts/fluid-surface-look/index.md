# fluid-surface-look

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §7.3, §7.3a, §8, §10 · `player_options.md` §4 · `entity_model.md` §7 · `movement.md` §6 · read at e8f607b94

## Problem
The owner reports that water surfaces look off-brand. The fluid pass already samples albedo through Post Retro sampling, but its Quake-style warp computes a continuous offset per screen pixel. The result is texel edges that bend into smooth curves, the opposite of the blocky world textures. Still water and water with a current also render identically, so the player cannot see which pools will push them. And nothing reflects. When this is done, fluid motion moves in whole-texel steps on the water texture's own texel grid. Still water shows a quiet pixel shimmer and ambient ripple rings, while water over a push volume visibly drifts the way the player is pushed. Fluids reflect the on-screen scene through screen-space reflections. Players control reflections with an Off / Pixel / Smooth setting, and a game can lock its reflections to Pixel as part of its art style.

## Decisions
- **Color stays the face texture, and the grid is its own texel lattice.** No resolution parameter is added: the lattice `sample_post_retro` already snaps to is `textureDimensions` of the bound texture, so a game's texture resolution sets the water's grain automatically and cannot drift from its world textures. A palette-only procedural fluid would need its own texels-per-meter value and still need a face texture for TrenchBroom. Distinct animated looks per fluid type (lava, sludge, rushing water) are a later direction.
- **Motion is evaluated per texel cell.** The warp offset is computed from the cell a fragment's UV falls in, rounded to whole texels, and animated on a time base stepped at a fixed engine rate. The `sample_post_retro` body does not change; the fluid/mover parity test stays the guard (`rendering_pipeline.md` §7.3a, `plans/done/E24--water-and-environment-volumes`).
- **Still water: shimmer plus ambient ripple rings, all in the shader.** Ring placement is a pure function of cell and stepped time. No entity, CPU work, or per-frame upload. Gameplay-driven ripples (player, projectile, splash) are a non-goal: they need an event-fed buffer, and splash visuals are already an E24 follow-up.
- **Current comes from the push volume the player feels.** At install, each fluid volume resolves its push vector once, using the same winner order as `resolve_environment` (priority, then smaller volume, then map order), sampled at the volume's surface. The result rides in the reserved components of the volume's group-4 slot. Push volumes are static baked data, so this is load-time work with no per-frame cost and no replication (`entity_model.md` §7, `movement.md` §6). The shader projects the vector into face UV space through the UV/world Jacobian, the Surface Depth precedent (§7.3). Visual drift speed is an engine curve of push magnitude, not an authored field. One vector per volume: a pool whose current should vary is split into volumes, the same remedy as for differing opacity.
- **SSR is a shared march snippet called from the fluid fragment shader.** It is not a fullscreen deferred pass; deferred rendering stays a non-goal (`index.md` §4). Inputs:
  - a copy of opaque scene color at scene extent, recorded after skinned meshes and before the fluid pass, only on frames that have a visible fluid draw and reflections not Off;
  - scene depth, sampled read-only.

  Both bind in fluid group 5, which carries textures, samplers and uniforms only, because fragment storage is at its 8/8 ceiling (§10). A ray that misses fades out (Fresnel × hit confidence × screen-edge fade). No probe or cubemap fallback exists or is added. The march follows the FXC rules of §8. The copy target lives with the scene targets and rebuilds with them on extent change.
- **Reflection scope.** Only a fluid face seen from its upward side reflects; side faces and any face seen from underwater do not. The reflection sees what the opaque copy holds: smoke, fog, the viewmodel and other fluid surfaces never appear in it.
- **Player setting: `reflection_quality` = off / pixel / smooth, default pixel.** It is a standalone graphics option with the same persistence, slot, bridge and live-apply path as `surface_depth_quality` (`player_options.md` §4). The stops are a style and cost choice, not a Low/Medium/High ladder. That section records why a capped middle tier reads as an artifact rather than a cheaper picture. The stops behave as follows:
  - Pixel snaps each reflection ray's origin and perturbation to the water texel grid, so the reflection breaks into cells that move with the water.
  - Smooth marches per pixel.
  - Off records neither the copy nor the march, and its picture is the new water without reflection.

  No umbrella quality preset is added; none exists or is planned.
- **The author lock lives in the manifest `render` block.** `render.reflections.pixelated: true` locks a game to pixel. The effective stop is resolved at the app render-profile chokepoint, where mod render preferences and player options already meet: off stays off, pixel stays pixel, and smooth becomes pixel. The player's saved value is never rewritten, so it returns if the lock is removed. The menu does not offer Smooth while locked. Off is always offered. This mirrors `render.bloom.pixelated`, the existing author-owned pixel switch. Omitted, `false`, or malformed means unlocked.
- **Per-fluid reflectivity is a descriptor field, `surfaceReflectivity`.** It sits beside `surfaceOpacity`, ranges 0..1 and defaults to 0.4, so water reflects out of the box. The dev content's `sludge` sets it low. It is local presentation like `surfaceOpacity`, so peers may differ and co-op compatibility is unaffected (`scripting.md` §2). Zero skips the march for that volume.
- **Non-goal: reduce motion does not still the water.** Reduce motion governs view feel, shake and UI tweens (`player_options.md` §5), not world animation; animated lights don't follow it either.

### Scripting surface
```ts
// Proposed design
export default defineMod({
  name: "neon-sump",
  id: "example.neon-sump",
  version: "1.0",
  render: {
    bloom: { resolution: "quarter", pixelated: true },
    reflections: { pixelated: true }, // optional; locks reflections to the Pixel style
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
The Luau mirror ships with the same names (`reflections = { pixelated = true }`, `surfaceReflectivity = 0.4`).

## Acceptance
### Automated
- [ ] Cell warp reference, GPU-free: every offset is a whole number of texels; all UVs in one texel cell get one offset; the offset changes only at time-step boundaries; zero current gives no net drift over a full cycle; a nonzero current drifts along its projected direction, and reversing the push reverses the drift.
- [ ] Install current resolve:
  - a fluid volume with a coincident push volume gets that push;
  - a fluid volume alone gets zero;
  - a push volume beside but not over the surface is ignored;
  - two push volumes over one surface yield the winner `resolve_environment` picks for a body at that point.
- [ ] A level with no fluid volumes creates no copy target and records no copy in any setting.
- [ ] A frame with fluid volumes but no visible fluid draw records no copy.
- [ ] Off records neither the copy nor the march.
- [ ] A fluid whose reflectivity is 0 draws exactly as under Off.
- [ ] Render-resolution change rebuilds the copy target at the new scene extent; the next frame binds the new one.
- [ ] The setting round-trips through `settings.toml` and its slot for each of off, pixel and smooth. An unrecognized stored value falls back for that field only.
- [ ] Effective stop resolution covers all six cases:
  - locked with saved smooth → pixel, and the saved value stays smooth;
  - locked with off → off;
  - locked with pixel → pixel;
  - unlocked with smooth → smooth;
  - lock removed → the saved smooth applies again;
  - `pixelated: false` and a malformed lock → unlocked.
- [ ] The Scripting surface example parses in TS and its Luau mirror. `surfaceReflectivity` omitted → 0.4; 0 and 1 accepted; −0.1, 1.1 and NaN rejected with a warning naming the field, the declaration otherwise kept.
- [ ] The fluid/mover Post Retro parity test passes with its comparison unchanged.
- [ ] Pipeline budget tests pass: fluid fragment storage stays at 8, and sampled textures stay within the requested limit.
- [ ] The DX12 FXC shader test compiles the fluid shader (Windows, ignored-gated run).

### Manual
- [ ] Still pool beside a current pool: the still pool shimmers and rings in place; the current visibly drifts. Standing in the current pushes the player the way the surface drifts.
- [ ] Motion reads as whole-texel steps on a 64 px and a 256 px water texture; the 64 px pool is visibly chunkier. Texel seams at cell borders show no smear or shimmer band.
- [ ] Each stop on a reflective pool: Off shows no reflection. Pixel shows a reflection broken into texel cells that move with the water. Smooth shows a per-pixel reflection.
- [ ] With a locking manifest, the menu offers only Off and Pixel. A saved Smooth renders as Pixel.
- [ ] A ray leaving the screen edge fades with no hard line. The camera at the surface plane and the camera underwater show no reflection artifacts, and a pool's side face seen through glass shows none. The sludge reflects visibly less than water.
- [ ] Changing the setting and render resolution live, mid-level, applies without a reload.
- [ ] GPU cost with `POSTRETRO_GPU_TIMING=1` on the GTX 1660 Super, and on the RDNA1 Mac via Metal System Trace where timestamps are unsupported. Measure the fluid pass and the copy at each stop, looking down at a pool that fills the view, and compare Off with the pre-change build. Report the numbers in the plan of record; the owner judges them.

## Path
- **Warp.** It lives in `fluid_warp_uv` (fluid_surface.wgsl). Derivatives stay hoisted from the unwarped UV, as now. A CPU mirror of the cell warp in `postretro-render-cpu` lets the reference test pin it, the same shape as `surface_depth`.
- **Current.**
  - Seams: `pack_fluid_surfaces` and `FluidSurfaceSlots` (render-cpu `fluid_surface`), `Renderer::install_fluid_surfaces`.
  - Today `set_draws` writes and dirty-checks opacity only. The current is static, so it can be written at install and survive later opacity writes.
  - Reuse the winner order without duplicating it: `collision::resolve` owns it, so check whether its comparator can be called from install or lifted to a shared crate.
- **Copy and march.**
  - `record_scene_passes` (renderer_render_frame.rs) holds the insertion point.
  - `create_scene_color` (screen_effects.rs) needs `COPY_SRC` outside tests. The alternative is a blit pass into the copy target; choose whichever is cheaper on Metal.
  - The fluid pass attaches depth Load/Store today. Move it to `depth_ops: None` as forward does.
  - Scene targets rebuild in `rebuild_scene_targets` (renderer_extent.rs).
- **Strongest rival:** a fullscreen SSR pass gated by a reflectivity mask, which `ssr-decals` imagines. It is rejected here because fluid is translucent and drawn after opaque: a post pass cannot place a reflection under the fluid's own alpha. The snippet and the copy target are the parts `ssr-decals` can reuse.
- **Option and lock.**
  - Follow the `surface_depth_quality` touch list end to end: options enum and keys, `persist.rs`, `engine_state_catalog.rs` with regenerated SDK types and typedef fixtures, the bridge, `frontend-menu.ts` with the graphics-tab id/count test in `main.rs`, `render_profile.rs`, `app/options_menu.rs`, the renderer setter, and boot and level re-apply.
  - The lock parses beside `ModBloomProfile` and resolves in `render_profile.rs` next to `renderer_bloom_profile`.
- Dev-only march tuning (step budget, thickness) can join the diagnostics panel's SDF / Fog Quality section (`debug_ui`); never player-facing.
- **First slice.** The cell warp plus the opaque copy and a bare march at Smooth on one pool, timed on the 1660. That falsifies the two riskiest assumptions together: that whole-texel motion reads well through the seam AA, and that the copy and march fit the budget.
- **File sizes.** `fluid_surface.rs` and `record_scene_passes`' file may pass ~800 lines with this work. Check before extending, and split first in their own commit.

## Boundary inventory

| Name | Rust | settings.toml / slot | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reflection setting | `PlayerOptions::reflection_quality` | key `reflection_quality`; slot `options.reflectionQuality`; values `"off"` / `"pixel"` / `"smooth"` | slot string | slot string | n/a |
| Author lock | field on `ModRenderProfile` beside `bloom` | n/a | `render.reflections.pixelated` | `render.reflections.pixelated` | n/a |
| Fluid reflectivity | field on `FluidDescriptor` | n/a | `surfaceReflectivity` | `surfaceReflectivity` | n/a |

## Open questions
- Stepped animation rate, ring density and lifetime, shimmer amplitude, and the push-to-drift curve — **delegated**: tune by eye at the first manual capture, record the values in the plan of record.
- Whether the menu renders the three stops as a stepped slider or as the tab's existing enum buttons — **delegated**: match the graphics tab's current enum controls unless a stepped slider widget already exists.
- Whether a hot-reloaded manifest applies a changed lock live or at next level install — **delegated**: follow whatever `render.bloom` does today, and report it.
