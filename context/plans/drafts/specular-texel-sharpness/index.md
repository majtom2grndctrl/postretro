# specular-texel-sharpness

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Static specular ownership), §7.3, §7.3a (Style), §13 · `context/lib/resource_management.md` §3, §4.1, §4.6 · `context/lib/build_pipeline.md` §Baked texture mips · `context/lib/scripting.md` §2 (Mod render profile) · read at e919c0e0

## Problem
The owner requested two things.

**Per-texel sharpness.** One texture should be able to mix matte and glossy texels. Today the exponent is one value per material, taken from the name prefix.

**Texel-snapped highlights.** These come first because they are what makes sharpness safe. Normal and intensity are already constant across a texel, but the highlight's position and view vector vary per pixel. As a result, today's glossy prefixes (metal 64, glass 96) already draw a smooth highlight inside a texel that crawls as the camera moves, against the texel grid every other surface read snaps to. Sharpness maps would put the tightest highlights exactly where an author asked for them, so they make that crawl the headline look rather than an edge case.

When done:
- By default, every world and mover highlight takes its shape from one point per texel. The highlight is softened only across the seam band texture reads already use.
- A mod can keep today's per-pixel highlights.
- Authors paint sharpness in a grayscale sibling PNG.

## Decisions
- **Texel-centre highlight.** Under the pixel style, L and V are evaluated at the world position of the reconstructed texel centre on the albedo lattice. That position is the reconstructed UV `sample_post_retro` samples at, mapped to world through the fragment's UV→world Jacobian. So the highlight snaps where texture reads snap, and ramps across the same seam band. Each site keeps its current height convention: forward static specular uses the true plane, mover specular the Surface Depth hit. A degenerate Jacobian (edge-on fragment, collapsed UV chart) falls back to the per-pixel position. Only L and V snap. Shadowmask visibility, SDF visibility slices and SDF light selection stay per pixel, so specular shadows keep matching diffuse ones, and a shadow edge may still split a texel.
- **Default pixel applies to existing content.** Metal and glass highlights on every shipped material change look without any new map. The owner chose this.
- **Look is the mod author's, as for fluid reflections.** `render.specular.style` accepts `"pixel" | "smooth"`, default pixel, and follows the mod render profile's existing rules (`scripting.md` §2):
  - a malformed value warns and degrades on its own;
  - a successful staged init replaces the style, and a failed init keeps it;
  - it applies live.
  Smooth is today's per-pixel evaluation. Smooth with sharpness maps is the author's accepted trade: sharp texels crawl. This is a mod setting, not a player option, exactly like reflection style (`player_options.md`).
- **Sites in scope.** Forward world static specular (owners and the SDF selection) and the mover pass's selected-static specular. Nothing else evaluates world specular: the forward dynamic loop, fluids and skinned models carry none, and billboards evaluate at the sprite centre with their own exponent. The style does not reach billboards.
- **Authoring: `{name}_p.png`, specular power.** A grayscale, linear PNG, resolved like `_s`/`_n`/`_e`/`_h`, with the same colour-space rules as `_h`. Its dimensions must match the diffuse, and `_s`/`_h` when present. A mismatch is a hard bail, as for `_h` (`resource_management.md` §4.6). `_g` is ruled out: the dev texture packs already name diffuse maps `*_G.png`, and suffix lookup ignores case. `_s.png` is unchanged.
- **Sharpness is absolute.** Exponent = 2^(8·p/255), spanning 1 to 256, the range `Material::shininess` already documents. The prefix exponent remains the fallback for any material with no `_p`. Because of the log encoding, linear mip filtering averages exponents geometrically.
- **Storage: a widened specular slot.** A material with `_p` bakes slot 1 as `Rgba8Unorm`:
  - R: intensity, all zero without `_s`, the same zero response as the black placeholder;
  - G: inverted height, or the mid-gray plane without `_h`, which gives an empty relief band and so no march cost (§4.6);
  - B: sharpness;
  - A: unused.
  The SPECULAR slot bit is set when any of `_s`/`_h`/`_p` is present. Materials without `_p` keep `R8Unorm`/`Rg8Unorm` and their sidecar keys. No binding is added. Diffuse alpha stays free for future cutouts; emissive alpha keeps its reservation (§4.5). Cost: 4 bytes per texel, only for materials with `_p`.
- **Has-depth and has-sharpness come from the loaded slot.** Has-depth widens to "two- or four-channel". Has-sharpness means "four-channel", and rides a material-uniform word the Surface Depth player switch never rewrites.
- **Bake stage version bumps.** Content addressing must fold `_p` in only when it is present, as it does for `_h`. The transform change still invalidates stale sidecars (`build_pipeline.md` §Baked texture mips).
- **§13 vocabulary widens by one entry: specular sharpness.** It rides the specular slot's third channel. It is a per-texel gloss input, which §13's "no roughness inputs" clause used to exclude, so §13 is amended to admit a per-texel Blinn-Phong exponent. §13 keeps excluding a PBR BRDF, energy conservation, Fresnel on world surfaces, and metallic inputs (`rendering_pipeline.md` §13, `index.md` §2).
- **First consumer.** A dev test material with an authored `_p` gradient, placed in a dev map on both a static face and a mover. `content/dev`'s mod keeps the default pixel style.
- **Non-goals.**
  - Specular antialiasing (Toksvig, mip-aware exponents). It is a separate follow-up, informed by the minification observation below.
  - Moving shininess to `materials.toml`, which `emissive-volumes` names as its eventual home.
  - Generating `_p` in texture-tool or `gen_specular.py`.
  - Per-texel specular tint, which stays open for a colour `_s.png` later.
  - Specular for the forward dynamic loop.
  - A player-facing specular option.

### Scripting surface
```ts
export default defineMod({
  id: "my-game",
  version: "1.0.0",
  render: {
    // "pixel" (default): one highlight per texel. "smooth": per-pixel highlights.
    specular: { style: "smooth" },
  },
});
```
The Luau mirror ships too: `render = { specular = { style = "smooth" } }`, typed in both SDK type files beside `reflections`.

## Acceptance
### Automated
**Texel-constant highlight (pixel style)**
- [ ] Run on the real shader under pixel style:
  - fragments at different positions inside one magnified texel produce identical specular;
  - adjacent texels with different highlight geometry differ;
  - a fragment inside the seam band falls between its two neighbours.
  Smooth style fails the first clause.
- [ ] An edge-on fragment and a collapsed-UV fragment produce finite specular equal to the per-pixel evaluation.
- [ ] A directional owner's highlight is texel-constant under pixel style, because V snaps even though L does not depend on position.
**Style**
- [ ] Smooth style on a material with no `_p` renders specular byte-identical to the build before this change.
- [ ] Each authored style reaches the renderer as that style. Omitted and malformed values both give pixel, and a malformed value warns. A failed staged init keeps the previous style.
- [ ] Changing the style live changes the next frame's specular without rebuilding any pipeline.
- [ ] The scripting-surface example, run as a dev mod manifest in both TS and Luau, selects smooth.
**Sharpness read**
- [ ] On a four-channel slot, sharpness byte 0 shades as exponent 1, 255 as 256, and 128 as 2^(8·128/255). A one- or two-channel slot on the same face shades at the prefix exponent.
- [ ] Switching Surface Depth Off then On keeps per-texel sharpness throughout and restores the march.
**Bake**
- [ ] A material with no `_p` bakes the same sidecar bytes and key as before this change.
- [ ] `_p` alone, `_p`+`_s`, `_p`+`_h` and all three each bake four-channel with R, G and B as decided.
- [ ] Without `_h`, a four-channel material's uniform relief row is byte-equal to that of a material with no height map.
- [ ] With `_h`, the four-channel relief band equals the two-channel band measured from the same height.
- [ ] A `_p` whose dimensions disagree with the diffuse, `_s` or `_h` fails the build and names both files. Matching dimensions pass.
- [ ] A `_p` carrying sRGB or iCCP, or a non-unit gAMA, fails colour-space validation.
- [ ] One mip down, a 2×2 block of sharpness bytes 0 and 255 in equal parts filters to about 128.
- [ ] Texture byte accounting reports four bytes per texel for a four-channel specular slot at every mip.
- [ ] A sidecar baked before this change is rebuilt, not reused.
### Manual
- [ ] GPU captures under a static light, pixel style against the previous build: metal and glass highlights become texel blocks with antialiased seams, and concrete is visually unchanged. Smooth style matches the previous build.
- [ ] A slow camera move across a glossy face under pixel style shows no movement inside any texel.
- [ ] The `_p` gradient material gives tight highlights on sharp texels and broad ones elsewhere, identically on the static face and the mover.
- [ ] On a carved Surface Depth material with `_p`, top and side faces each hold one highlight value per texel face.
- [ ] Under pixel style, a static shadow edge crossing a glossy texel cuts that texel's highlight on the same line it cuts the diffuse.
- [ ] Minification observation, measure and report (not a gate): whether high-sharpness texels sparkle at a distance.
- [ ] On the Mac target, the forward and mover passes at Surface Depth `Off` cost no more than the previous build, in either style. The mover shader's register count stays below the documented cliff (`rendering_pipeline.md` §7.3).

## Path
- **Reconstruction precedent:** `fluid_reflection_cell_centre` and `fluid_reflection_texel_axes` (`fluid_reflection_ray.wgsl`, fluid pipelines only). `surface_depth_resolve` solves the same Jacobian inline, but only once the march is active. Factor the UV reconstruction out of `sample_post_retro` so sample and position cannot disagree. The mover pass carries its own copy of that function.
- **Mover pass:** its loop takes diffuse L from the Surface Depth hit. Pixel-style specular needs a second direction for specular lights.
- **Style chain precedent:** `ModReflectionStyle` → `renderer_reflection_style` (`render_profile.rs`) → `set_fluid_reflection_style`, a live uniform write. Parsing lives in scripting-core's JS and Luau manifest readers, and the types in `sdk/types/postretro.d.ts` / `.d.luau`.
- **Bake:** `_p` joins `BUNDLE_SUFFIXES` and the `_h` interleave in `bake_materials_into`. Slot widening touches:
  - `format_allowed_for_slot`;
  - `specular_slot_is_surface_map` and its test;
  - the relief gate in `loaded_texture.rs`;
  - `surface_relief_from_rg8_levels` (it assumes 2 bytes per texel);
  - `build_surface_chain_impl` (channels = 2);
  - `mip_level_bytes`;
  - the bake `STAGE_VERSION`.
- **Harness:** `world_specular_test.rs` already runs the real `world_static_specular` through GPU readback. Extend it with a UV, a texel grid and the style.
- **Strongest direction rival:** split the work. Ship sharpness alone, either capped at today's 96 or per-pixel, and handle the snap separately. Bundling wins on two counts. Without the snap, sharpness's headline use crawls. And both changes edit the same two shader sites, so the Mac `Off` and register-cliff measurement runs once.
- **Storage rival:** sharpness in the green of `_s.png`, plus a migration command. It adds no new file, but it forecloses a colour `_s.png` for tint and needs a permanent equality guard.
- **First slice:** texel-centre reconstruction in the forward pass plus the readback test. It falsifies the seam-band and degenerate-Jacobian assumptions before any format work.

## Open questions
- Which material-uniform word or bits carry has-sharpness and the style — **delegated**: the executor picks within the constraint in Decisions and reports it.
- Name and contents of the dev test material and map — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| profile key | mirrors `ModReflectionStyle` (e.g. `ModSpecularStyle`) | `render.specular.style` | `render.specular.style` | `render.specular.style` | n/a |
| values | `Pixel`, `Smooth` | `"pixel"`, `"smooth"` | `"pixel" \| "smooth"` | `"pixel" \| "smooth"` | n/a |
| sibling PNG | — | `{name}_p.png` (case-insensitive) | — | — | n/a |
