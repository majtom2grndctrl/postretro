# specular-texel-sharpness

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Static specular ownership), §7.3, §7.3a (Style), §13 · `context/lib/resource_management.md` §3, §4.1, §4.6 · `context/lib/build_pipeline.md` §Baked texture mips · `context/lib/scripting.md` §2 (Mod render profile) · `context/lib/player_options.md` · read at e919c0e0

## Problem
The owner wants per-texel specular sharpness, so one texture can mix matte and glossy texels; today the exponent is one value per material, from the name prefix. Texel-snapped highlights come with it, because sharpness needs them to be safe. Normal and intensity already hold constant across a texel, but the highlight's position and view vector vary per pixel. So today's glossy prefixes (metal 64, glass 96) already draw a smooth highlight inside each texel that crawls with the camera, against the grid every other surface read snaps to. Sharpness maps would put the tightest highlights wherever authors paint them. When done, by default every world and mover highlight takes its shape from one point per texel, softened only across the seam band texture reads already use. A mod can keep today's per-pixel highlights, and authors paint sharpness in a grayscale sibling PNG.

## Decisions
- **Texel-centre highlight.** Under the pixel style, every geometric input to a highlight comes from the world position of the reconstructed texel centre on the albedo lattice. That covers light direction, view direction, distance falloff, range cutoff and spot cone. The reconstructed texel centre is the same point `sample_post_retro` samples. Each site keeps its height: forward static specular uses the true plane, mover specular the Surface Depth hit (its view direction included). A degenerate Jacobian (edge-on fragment, collapsed UV chart) falls back to the per-pixel position. Shadowmask visibility, SDF visibility and SDF light selection stay per pixel, so specular shadows keep matching diffuse ones.
- **Default pixel applies to existing content.** Shipped materials change look without any new map, most visibly default-prefix terrain (exponent 32) and wood (16). The owner chose this.
- **The look is the mod author's, as for fluid reflections.** `render.specular.style` accepts `"pixel" | "smooth"`, default pixel. It follows the mod render profile's rules (`scripting.md` §2) and applies live. Smooth is today's per-pixel evaluation. Smooth combined with sharpness maps is the author's accepted trade: sharp texels crawl. It is a mod setting, never a player option (`player_options.md`). Capture, which loads no manifest, takes a `specular_style` scene key beside `reflection_style`.
- **Sites in scope.** Forward world static specular (owners and the SDF selection) and the mover pass's selected-static specular. These are the only places world specular is evaluated (`research.md` census). The style does not reach billboards, which already evaluate at the sprite centre with their own exponent.
- **Authoring: `{name}_p.png`, "specular power".** A grayscale linear PNG, resolved like `_s`/`_n`/`_e`/`_h`, under the colour-space rules `_h` uses. Its dimensions must match the diffuse, and `_s`/`_h` when present, as a hard bail like `_h` (`resource_management.md` §4.6). `_s.png` is unchanged.
- **Sharpness is absolute.** Exponent = 2^(8·p/255), spanning the 1–256 range `Material::shininess` documents. The prefix exponent remains the fallback for any material with no `_p`.
- **Storage: a widened specular slot.** A material with `_p` bakes slot 1 as `Rgba8Unorm`: R intensity (zero without `_s`), G inverted height (the stored mid-gray plane without `_h`, so the relief band is empty, §4.6), B sharpness, A unused. The SPECULAR slot bit is set when any of `_s`/`_h`/`_p` is present. Materials without `_p` keep their format, sidecar bytes and key, because `_p` folds into the content address only when present. `STAGE_VERSION` stays 3; the widening is additive, as `_h`'s was (`build_pipeline.md` §Baked texture mips). Diffuse and emissive alpha stay untouched.
- **Has-depth and has-sharpness come from the loaded slot's format.** Both hold for a four-channel slot. Has-sharpness keeps its value in every Surface Depth state: On, Off, a flat band, and a prefix with no depth tuning. The 32-byte material uniform and its full rewrite stay. The style is global, so it rides the world and mover passes' per-frame uniforms, not the material uniform.
- **Cost.** Smooth costs no more than the build before this change. Pixel's cost is measured and reported. The mover shader's register cliff (`rendering_pipeline.md` §7.3) is a hard gate for both styles.
- **§13 widens by one entry: specular sharpness.** It rides the specular slot's third channel. §13's "no roughness inputs" clause is amended to admit a per-texel Blinn-Phong exponent. §13 still excludes an energy-conserving BRDF, metallic inputs, and image-based specular outside fluids (`rendering_pipeline.md` §13, `index.md` §2).
- **First consumer.** A dev test material with an authored `_p` gradient, placed in a dev map on a static face and on a mover. `content/dev`'s mod keeps the default pixel style.
- **Non-goals.**
  - Specular antialiasing (Toksvig, mip-aware exponents): a separate follow-up, informed by the minification observation below.
  - Shininess in `materials.toml`: `emissive-volumes` names that file as its eventual home.
  - `_p` generation in texture-tool or `gen_specular.py`: sharpness is an artistic choice that no prefix heuristic can guess.

### Scripting surface
```ts
export default defineMod({
  id: "my-game",
  name: "My Game",
  version: "1.0.0",
  render: {
    // "pixel" (default): one highlight per texel. "smooth": per-pixel highlights.
    specular: { style: "smooth" },
  },
});
```
The Luau mirror ships too: `render = { specular = { style = "smooth" } }`, typed beside `reflections` in both SDK type files.

## Acceptance
### Automated
**Texel-constant highlight**
- [ ] Under pixel style on the real shader, fragments at different positions inside one magnified texel produce identical specular for a point, a spot and a directional owner light. Adjacent texels with different highlight geometry differ. A fragment in the seam band falls between its two neighbours. This includes a texel crossed by the light's range boundary and one crossed by the spot's cone edge. Smooth style fails the first clause.
- [ ] Under pixel style, fragments inside one magnified texel with different shadowmask or SDF visibility each produce that texel's specular scaled by their own visibility.
- [ ] An edge-on fragment and a collapsed-UV fragment produce finite specular equal to the per-pixel evaluation.
- [ ] The mover pass reconstructs the texel centre with the same function as the forward pass, and a parity test pins it. The mover pass reads sharpness exactly as forward does.
- [ ] Under pixel style on the real mover shader, fragments inside one magnified texel produce identical specular for a point, a spot and a directional selected-static light, each scaled by its own per-pixel shadow, with the march on and off.
**Style**
- [ ] Smooth style on a material with no `_p` renders specular byte-identical to the build before this change, in both the forward and mover passes, against a frozen copy of the old function.
- [ ] Each authored style reaches the renderer. Omitted and malformed values give pixel, and a malformed value warns. A malformed `render.specular` leaves an authored `render.reflections.style` intact, and the reverse, in TS and Luau.
- [ ] A failed, rejected or stale staged init keeps the previous style. A committed init with no start script restores pixel (pin P6).
- [ ] The style commits beside the reflection style at boot mod init and on every committed staged reload. Frame capture reads it only from its scene key. An omitted `specular_style` gives pixel, and an unknown value fails the scene parse.
- [ ] A style committed with no level loaded, before a full-renderer rebuild, or before suspend and resume reaches the next installed level's static faces and movers on its first frame (pin P4).
- [ ] A staged init that commits a style while a level load is in flight reaches that level when it publishes (pin P5).
- [ ] A live style change alters the next frame's specular on static faces and movers without rebuilding any pipeline or bind group. The planar water mirror draws them in the committed style on the same frame as the main view.
- [ ] Forward's shared group-0 uniform stays 128 bytes, and billboard and wireframe shaders are unchanged.
- [ ] Under smooth, Surface Depth Off then On keeps smooth highlights. A style change while Surface Depth is Off leaves the march off (pins P1, P2).
- [ ] Billboard specular is byte-identical under pixel and smooth.
- [ ] A repository gate fails if an option name, settings key or `options.*` slot names a specular style or sharpness, as it does for reflection style.
- [ ] The scripting-surface example, run as a test-fixture manifest in TS and Luau, selects smooth. `content/dev`'s manifest leaves pixel.
**Sharpness read**
- [ ] On a four-channel slot, under each style, sharpness byte 0 shades as exponent 1, 255 as 256, and 128 as 2^(8·128/255). A one- or two-channel slot on the same face shades at the prefix exponent.
- [ ] Surface Depth Off then On keeps per-texel sharpness throughout and restores the march. This holds for `_p`+`_h`, for `_p` without `_h`, and for a `_p` material whose prefix has no Surface Depth tuning (pin P3).
**Bake**
- [ ] A material with no `_p` bakes the same sidecar bytes and key as before this change, and an existing sidecar is a cache hit.
- [ ] `_p` alone, `_p`+`_s`, `_p`+`_h` and all three each bake four-channel with R, G and B as decided. Each sets the SPECULAR slot bit and reads back through the runtime `.prm` reader.
- [ ] Without `_h`, a four-channel material on a prefix with Surface Depth tuning, at Surface Depth On, uploads a relief row byte-equal to the same prefix with no height map (pin P7).
- [ ] With `_h`, a four-channel material loaded through the runtime loader sets has-depth. At every uploaded mip its relief band equals the two-channel band from the same height, with sharpness bytes that differ from the height bytes.
- [ ] A `_p` whose dimensions disagree with the diffuse, `_s` or `_h` fails the build, naming both files. Matching dimensions pass.
- [ ] A `_p` carrying sRGB or iCCP, or a non-unit gAMA, fails colour-space validation.
- [ ] One mip down, a 2×2 block of sharpness bytes 0 and 255 in equal parts filters to about 128. This is a regression guard on the third channel's mip stride.
- [ ] Texture byte accounting reports four bytes per texel for a four-channel specular slot at every mip.
- [ ] A diffuse with `_p` bakes to a different sidecar key from the same diffuse alone, and `_p` alone bakes to a non-zero key of its own. Neither overwrites the other.
- [ ] TrenchBroom's game config excludes `*_p`, so sharpness maps never appear as textures. The material-input gate's pinned suffix lists name `_p`.
**Docs**
- [ ] `docs/level_design.md` documents `{name}_p.png`: its exponent mapping, linear colour space and matching dimensions. `docs/scripting-reference.md` documents `render.specular.style`.
### Manual
- [ ] GPU captures under a static light against the previous build. Under pixel, default-prefix terrain and wood highlights become texel blocks with antialiased seams, and concrete is visually unchanged. Under smooth, chosen by the capture scene key, the render matches the previous build.
- [ ] A slow camera move across a glossy face under pixel shows no movement inside any texel.
- [ ] The `_p` gradient material shows tight highlights on sharp texels and broad ones elsewhere, identically on the static face and on the mover.
- [ ] On a carved Surface Depth material with `_p`, top and side faces each hold one highlight value per texel face.
- [ ] Under pixel, a static shadow edge crossing a glossy texel cuts its highlight on the same line it cuts the diffuse.
- [ ] Minification, measure and report (not a gate): whether high-sharpness texels sparkle at distance.
- [ ] On the Mac target at Surface Depth `Off`: smooth costs no more than the previous build, pixel's cost is reported, and in both styles the mover shader's register count stays below the documented cliff.

## Path
- **Reconstruction precedent:** `fluid_reflection_cell_centre` and `fluid_reflection_texel_axes` (`fluid_reflection_ray.wgsl`, fluid pipelines only).
  - `surface_depth_resolve` solves the same UV→world Jacobian inline, but only when the march is active.
  - Factor the UV reconstruction out of `sample_post_retro`, so the sample and the position cannot disagree. The mover keeps its own copy, fingerprint-pinned to forward's (`kinematic_brush.rs`).
  - The readback harness extracts functions by name, so the factored function must be one of them.
- **Mover loop.** Diffuse direction and attenuation come from the Surface Depth hit, and one attenuation value also carries the per-pixel shadow. Pixel-style specular needs its own direction, falloff and cone from the texel centre, multiplied by that per-pixel shadow.
- **Style chain precedent.** `ModReflectionStyle` → `renderer_reflection_style` (`render_profile.rs`) → `set_fluid_reflection_style`, which is a live uniform write.
  - Parsing: scripting-core's JS and Luau manifest readers.
  - Types: generated from `crates/sim/src/scripting/primitives/manifest.rs` by `gen-script-types`.
  - Capture: `CaptureReflectionStyle` in `capture/scene.rs`.
  - Option gate: `crates/xtask/src/environment_gates.rs`.
  - Ordering pins P4 and P5: boot state retention (`renderer_init.rs`), and `level_build.rs`'s dispatch-time snapshot.
- **Has-sharpness:** see `research.md` §Uniform constraints.
- **Bake.** `_p` joins `BUNDLE_SUFFIXES`, `bundle_hash_for` (append-only, as for height), `filename_key_for`'s present-count, and the `_h` interleave in `bake_materials_into`. The SPECULAR bit is set today only in the `_s` and `_h` arms. Slot widening touches:
  - `format_allowed_for_slot`
  - `specular_slot_is_surface_map` and its test
  - the relief gate in `loaded_texture.rs`
  - `surface_relief_from_rg8_levels` (2 bytes per texel)
  - `build_surface_chain_impl` (channels = 2)
  - `mip_level_bytes`
  - the plane byte (pin P7)
- **Harness.** `world_specular_test.rs` runs the real `world_static_specular` by GPU readback. Extend it with UV, a texel grid, per-fragment visibility and the style.
- **Strongest direction rival:** split sharpness from the snap. `research.md` §Rivals explains why bundling wins.
- **First slice.** Texel-centre reconstruction in the forward pass plus the readback test. It tests the seam-band and degenerate-Jacobian assumptions before any format work.

## Open questions
- How has-sharpness packs into the 32-byte material uniform — **delegated**: the executor picks within the Decision's constraint and reports it.
- Name and contents of the dev test material and map — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | Capture scene |
|---|---|---|---|---|---|
| profile key | mirrors `ModReflectionStyle` (e.g. `ModSpecularStyle`) | `render.specular.style` | `render.specular.style` | `render.specular.style` | `specular_style` |
| values | `Pixel`, `Smooth` | `"pixel"`, `"smooth"` | `"pixel" \| "smooth"` | `"pixel" \| "smooth"` | `"pixel"`, `"smooth"` |
| SDK types | — | — | `SpecularStyle`, `SpecularRenderProfile` | `SpecularStyle`, `SpecularRenderProfile` | — |
| sibling PNG | — | `{name}_p.png` (case-insensitive) | — | — | — |
