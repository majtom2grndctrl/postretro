# specular-texel-sharpness — plan of record

mode: resumable
status: approved
read at: 1c7fc9e3a

Branch: `feature/specular-texel-sharpness`. The brief was read at e919c0e0 (tint at e7438238). Every symbol the Decisions and Path cite was re-read at 1c7fc9e3a, after `emissive-volumes` (#686), E16 status effects and E25 level seams landed. No Decision premise is false. The corrections below are stale Path claims and clarifications that keep each Decision's and AC's meaning.

## Corrections

### Forward pass
- Brief: forward `fs_main` returns `base_color.rgb * total_light`. → Since emissive-volumes it returns `base_color.rgb * total_light + emissive` (`forward.wgsl:1585`), with strength and the volume factor folded into `emissive`. Planning around it: an untinted texel keeps `specular_sum` inside `total_light` (`:1388`) exactly as today. A tinted texel adds its specular after the albedo multiply, before `+ emissive`.
- Brief: the forward pass has no free sampled-texture binding. → Confirmed. FRAGMENT samples 16 of 16: group 1 holds 4 (base, emissive, spec, normal), group 3 holds 3, group 4 holds 5, group 5 holds 4. Emissive-volumes added a uniform (group 4 binding 8), not a texture. The specular slot is group 1 binding 2, sampled through the binding-5 aniso sampler, as the brief assumes. The widened slot needs no binding change.
- Path: the mover keeps its own copy of the reconstruction, fingerprint-pinned to forward's. → `material_shading.wgsl` is already concatenated into both pipelines (`pipeline_layout.rs` `SHADER_SOURCE`, `kinematic_brush.rs` `SHADER_SOURCE`). Planning around it, four pure, binding-free functions live there:
  - the texel-UV reconstruction factored out of `sample_post_retro`;
  - the specular-point builder;
  - the smoothness decode;
  - the tint mix.

  Both `sample_post_retro` copies (`forward.wgsl:577`, `kinematic_brush.wgsl:485`) call the factored UV function. Parity therefore holds by construction. `kinematic_brush_shader_matches_forward_post_retro_sampling` updates its call list. A new parity test asserts that both composed sources call the shared functions and that neither consumer redeclares them, as the `surface_depth.wgsl` seam is pinned. AC "same function plus a parity test" holds more strongly.
- Precedent `fluid_reflection_texel_axes` / `fluid_reflection_cell_centre` (`fluid_reflection_ray.wgsl:78`, `:90`) is current. Its zero-axes return on a degenerate Jacobian makes the point collapse to the fragment position exactly, which is the AC's per-pixel fallback. Its cell centre snaps to `floor + 0.5`. The new builder instead maps `sample_post_retro`'s reconstructed UV `(seam + aa) / dims`, so the highlight ramps across the seam band (research §Why reconstruction).

### Mover pass
- Today the mover takes L from the Surface Depth hit (`depth.world_position`) and V from `in.world_position` (`kinematic_brush.wgsl` `fs_main`, `accumulate_dynamic_direct`). Smooth keeps that split byte for byte. Pixel takes L, V, falloff, range and cone from the snapped point, as the Decision says.
- Mover point height on a side hit (clarification, owner-approved 2026-11-20). Rule: one point per texel, shared by all its visible faces, which differ only by normal; the point sits on the true plane in forward and at the texel's quantized top height in the mover. A per-face point (the centre of the exposed side wall) was rejected: it splits a texel into separate pieces, needs a neighbour height load at the mover register cliff, and differs by under a texel except under a light almost touching the face. The Decision gives the mover "the Surface Depth hit". On a top hit, every fragment of a texel face has the same height. On a side hit, the hit slides down the wall, so a point at the hit's own height would vary within one side face. Planning around it:
  - The mover point is the struck texel's centre, raised by that texel's quantized top height. On every top hit this equals the hit height exactly.
  - A side face then holds one value, which differs from its top's only through the normal.
  - Forward uses the true plane, so the question does not arise there (pin P9).
  - The top height comes from one `surface_depth_texel` load at the struck texel, outside the march, so `surface_depth.wgsl` itself does not change.
- `accumulate_dynamic_direct` folds the per-pixel shadow into `attenuation` and gates specular on the per-pixel influence `continue` and `n_dot_l > 0`. Planning around it (pin P10):
  - Pixel-style specular keeps the shadow as its own factor.
  - It runs its own range, cone and N·L tests at the point.
  - It is not skipped by the per-pixel influence early-out.
  - Diffuse keeps its gates.
- Tint split must preserve summation order. Today specular adds into `total` interleaved with each light's diffuse. A second accumulator receives specular only when the texel is tinted. An untinted texel accumulates in today's order, so the byte-identity rows can hold.
- Mover specular reaches only promoted selected-static records. Directional lights never promote (`rendering_pipeline.md` §4). The AC's directional mover case is proven by readback through the loop's directional branch. In-engine mover rows use a promotable point or spot light.
- No harness runs `accumulate_dynamic_direct` today. `kinematic_brush_backface_tests.rs` is the precedent for extracting mover functions by name into a GPU harness. Task 2 builds a mover specular harness that stubs the loop's bindings and shadow samplers.

### Style chain
- Transport, forward. The style is bit 2 of `Uniforms.spec_shadowmask_flags`, set for smooth, so a zeroed word reads pixel. Its constant goes in `render-cpu` `frame_uniforms.rs` beside bits 0–1, and `build_uniform_data` sets it. Billboard and wireframe already declare that word inert under their own names (`_spec_shadowmask_flags_inert`), so their sources stay unchanged and the uniform stays 128 bytes. The planar mirror copies the main bytes (`mirror_camera_uniform`, `planar_reflection/mod.rs:292`) and so draws the same style on the same frame.
- Transport, mover. `KinematicLightParams._pad2` becomes the style word (`kinematic_brush.rs`, `.wgsl`). `write_light_params` gains the argument (`renderer_render_frame.rs:626`). The main view and the mirror share `light_params_buffer`.
- Retention. The renderer keeps the style in boot state beside `fluid_reflection_style` (`renderer_types.rs:255`) and writes it into both per-frame uploads every frame. Two consequences:
  - `finish_full_init` has nothing to re-apply.
  - The style never enters `LevelBuildSnapshotInputs` (`level_build.rs`), so pins P4 and P5 hold by construction. A test pins that the snapshot does not carry it.
- Commit sites. The reflection and ghost commits are called from `splash_lifecycle.rs:580` (boot) and `staged_manifest_lifecycle.rs:336` (staged reload), and pinned by a source test in `render_profile.rs`. The specular commit joins both sites and that test. `ModGhostTransparency` is the newest precedent beside `ModReflectionStyle`.
- Capture. `CaptureReflectionStyle` (`capture/scene.rs:153`) and its apply in `capture/setup.rs:562` are current. `specular_style` mirrors them. The capture constructor (`renderer_init.rs:303`) starts at pixel.

### Bake and load
- The material bake is split across `texture_mips/{resolution,cache,world}.rs`. `texture_mips.rs` keeps the chain builders, including `build_surface_chain_impl` (`:320`).
- Emissive-volumes added `TextureBundlePaths::bundle`, the collection `materials.toml` is read by. It derives from the slot list `[emissive, diffuse, specular, normal, height]`. `_p` and `_t` join the end of that list, so a `_p`-only bundle still names its collection.
- `bake_materials_into` now returns `MaterialBake`. Its all-absent check and probe byte list take `_p` and `_t`.
- `bundle_hash_for` uses mask bits 0–4 and tags 0x00–0x04. `_p` takes bit 5 and tag 0x05; `_t` takes bit 6 and tag 0x06. Both are append-only, as height is. `filename_key_for` gains single-input arms for each.
- Path omits colour-space validation. → `validate_sibling_color_spaces` selects files by stem suffix (`texture_validation.rs:146`). `_p` and `_t` join that chain. No file under `content/` or `sdk/` ends `_p` or `_t` at HEAD.
- `mip_level_bytes` already counts `Rgba8Unorm` at 4 bytes per texel (`prm_accounting.rs:57`). The byte-accounting AC is a regression test only. `prm_format_to_wgpu` and residency already map `Rgba8Unorm`.
- `format_allowed_for_slot` (`prm.rs:360`) widens slot 1 to `Rgba8Unorm` (tag 1). The sprite plan (`smoke.rs:240` `plan_baked_sprite_array`) keeps its own slot-1 list (R8, Rg8). Sprites never bake `_p` or `_t`, and layered sprite sidecars never share an address with single-layer world bundles, so that list stays. Only its comment, which claims to mirror `format_allowed_for_slot`, changes.
- `SurfaceDepthUniform::resolve` returns `FLAT` for an empty quantized band. A `_p` or `_t` slot without `_h` stores G = 127 at every mip, so it measures flat at every mip and uploads the no-height row (pin P7). The relief reader (`surface_relief_from_rg8_levels`) and the load gate (`specular_surface_relief`, `loaded_texture.rs:194`) take a byte stride so the four-channel form reads G at offset 1 of 4.

### Material
- `Material::shininess` at HEAD is metal 64, glass 96, wood 16, concrete 4, grate 8, default 32, documented as 1–256. Retiring `neon` moved its names to Default, whose exponent is 32, the same as neon's was. No prefix fallback changes.

### Test risk
- The byte-identity rows compare the new shader against frozen WGSL copies of the 1c7fc9e3a functions in one GPU harness. `rendering_pipeline.md` §7.3 notes that Metal fast math may reorder arithmetic. If it breaks bit identity on the Mac, the task stops and reports to the owner. Proof never falls back to a tolerance on a byte-identity row.

## Delegated answers
- **Dev test material and map: names and contents.** The plan uses three generated materials and one map, so each Decision-named consumer and manual row 5 has a fixture with known texel values.
  - **`content/dev/textures/test/metal_smoothness_ramp`**, 16×16.
    - Diffuse: a neutral two-tone grey checker per texel (sRGB 118 and 138), so texel edges read.
    - `_s`: 255 everywhere.
    - `_p`: column x stores round(255·x/15), giving 0, 17, …, 255. The ramp spans the flat band (bytes 0–24, exponent 1), mid values, and the cap band (bytes ≥ 224, exponent 8192).
    - The `metal` prefix's fallback of 64 differs from every ramp column but one.
  - **`content/dev/textures/test/metal_tint_inlay`**, 16×16.
    - Diffuse: a dark green field (sRGB 28, 64, 40) with a bronze inlay (sRGB 150, 96, 44). The inlay is a one-texel ring plus a centred 4×4 block.
    - `_s`: 255 everywhere.
    - `_t`: 0 on the field, 255 on the ring (stored 254), 128 on the block (the mid case).
    - No `_p`, so it shades at metal's 64.
  - **`content/dev/textures/test/concrete_carved_gloss`**, 16×16. Used for manual row 5 and in-engine pins P3 and P11.
    - Layout: 4×4-texel tiles with one-texel mortar lines.
    - Diffuse: grey tiles and dark mortar.
    - `_h`: tiles 200, mortar 56.
    - `_p`: tiles 235, mortar 30.
    - `_s`: 220.
    - The `concrete` prefix carries the deepest Surface Depth tuning.
  - **Generator.** `tools/gen_specular_texel_fixtures.py` writes all exact-value PNGs, with `tools/tests/test_gen_specular_texel_fixtures.py` checking every byte. It is deterministic and draws no randomness. Siblings carry no `sRGB`, `gAMA` or `iCCP` chunk. Exact-value fixtures stay script-generated in `tools/`, never authored by hand or in Material Maker.
  - **Map.** `content/dev/maps/specular-texel-sharpness.map`, with `specular-texel-sharpness.README.md`. It is one sealed room.
    - Lights: one static point light and one static spot. Both are non-animated and promotion-eligible (not dim, short-falloff, directional or SDF), so they own the static faces and promote onto the movers.
    - North wall: the ramp. East wall: the inlay. Floor: the carved material.
    - Movers: two parked `kinematic_mover` brushes, faced with the ramp and the inlay, inside both lights' influence beside the walls.
    - Door beside wall: a third parked `kinematic_mover` faced with `concrete_carved_gloss`, set into a static wall of the same material, with the static point light about half a metre from the seam. It shows the plane-vs-top-height point gap between forward and mover (one to a few texels) under a close light.
    - Comparison strip for manual row 1, under the same lights: one face each of `50-free-textures/default_dirt_011` (Default, 32), `50-free-textures/wood_bark_048` (wood, 16) and `50-free-textures/concrete_pavement_036` (concrete, 4).
    - Texture scale: one texel spans 8 units on every fixture face.
    - README: build and run commands. It also gives two capture scenes that differ only in `specular_style` (pixel and smooth) and the manual walk for each manual row.
  - **Optional showcase.** If the owner wants an art-quality `_p`/`_t` material, it is authored procedurally through `mm-ptex` (`/Users/dhiester/Projects/Personal/mm-ptex`) and exported by Material Maker. That step is owner-run, gates no row, and is never a test fixture.

## AC-to-proof

GPU rows need a Metal or other GPU adapter and run with `POSTRETRO_REQUIRE_GPU=1` on the Mac, so a skipped run cannot pass. Unless named otherwise, readback tests live in `crates/renderer/src/render/world_specular_test.rs` (forward) and the new `kinematic_specular_test.rs` (mover).

| AC | Proof | Status |
|---|---|---|
| **Texel-constant highlight** | | |
| 1. Pixel: identical specular within a magnified texel for point, spot and directional owners. Adjacent texels differ; the seam band falls between. Range-crossed and cone-crossed texels included; march on and off (P9). Smooth fails clause 1. | `pixel_specular_is_constant_inside_a_magnified_texel`, `pixel_seam_band_lies_between_neighbouring_texels`, `pixel_specular_holds_across_range_and_cone_boundaries`, `pixel_point_anchors_on_the_fragment_uv_under_a_march`, `smooth_specular_varies_inside_a_magnified_texel` (Mac GPU) | achievable as stated |
| 2. Pixel: per-fragment shadowmask or SDF visibility scales that texel's specular | `pixel_specular_scales_by_each_fragments_own_visibility` (Mac GPU) | achievable as stated |
| 3. Forward builds the point through one extracted function fed UV, marched UV, position and derivatives; the readback drives it | readback extracts the shared point builder by name from `material_shading.wgsl`; source test `forward_fs_main_builds_the_specular_point_through_the_shared_builder` | achievable as stated |
| 4. Edge-on and collapsed-UV fragments: finite, equal to per-pixel | `degenerate_jacobian_specular_matches_the_per_pixel_evaluation` (Mac GPU) | achievable as stated |
| 5. Mover reconstructs with forward's function (parity test); reads sharpness as forward | `forward_and_mover_share_the_specular_texel_functions` (composed-source parity, in `kinematic_brush.rs` tests) plus mover decode readback | achievable as stated |
| 6. Mover pixel: identical within a texel for point, spot and directional selected-static lights, each × its per-pixel shadow, march on and off; influence/range, cone-edge and terminator texels (P10) | `mover_pixel_specular_is_constant_inside_a_magnified_texel`, `mover_pixel_specular_ignores_per_pixel_gates` (Mac GPU) | achievable as stated |
| **Style** | | |
| 7. Smooth on a no-`_p`/no-`_t` material: byte-identical to the previous build, forward and mover, against a frozen copy | `smooth_specular_matches_the_frozen_pre_change_function` (forward and mover harnesses; frozen WGSL from 1c7fc9e3a) (Mac GPU) | achievable as stated |
| 8. Each authored style reaches the renderer; omitted and malformed give pixel, and malformed warns; `render.specular` and `render.reflections` degrade independently, TS and Luau | `crates/scripting-core/src/runtime/specular_profile_tests.rs` (beside `reflection_profile_tests.rs`, with log capture) plus `render_profile.rs` `every_authored_specular_style_maps_to_its_renderer_style` | achievable as stated |
| 9. Failed, rejected and stale staged init keep the style; committed with no start script restores pixel (P6) | `staged_manifest_lifecycle.rs` tests beside the reflection-style cases | achievable as stated |
| 10. Commits beside reflection at boot and staged reload; capture reads only `specular_style`; omitted gives pixel; unknown fails the parse | `render_profile.rs` commit-site source test (extended); `capture/scene.rs` parse tests; `capture/setup.rs` apply test | achievable as stated |
| 11. Style committed with no level, before full rebuild, or before suspend/resume reaches the next level's first frame (P4) | renderer frame tests `specular_style_reaches_the_first_frame_of_each_install` (full init, first install, unload then install, rebuild then install) plus the App resume replay test beside the reflection one (Mac GPU) | achievable as stated |
| 12. A staged commit during an in-flight load reaches the level at publish (P5) | `level_build_tests.rs` `specular_style_committed_mid_build_reaches_the_published_level`, plus a snapshot-field guard | achievable as stated |
| 13. A live change alters the next frame on static faces and movers with no pipeline or bind-group rebuild; the mirror matches on the same frame | frame test reading back forward and mover pixels across a style change with build counters unchanged; `planar_reflection` test that the mirror uniform and the shared mover params carry the bit (Mac GPU) | achievable as stated |
| 14. Group-0 uniform stays 128 bytes; billboard and wireframe unchanged | existing `UNIFORM_SIZE` and WGSL-stride tests; preflight check that `git diff 1c7fc9e3a -- billboard.wgsl wireframe.wgsl` is empty | achievable as stated |
| 15. Smooth survives Surface Depth Off then On; a style change at Off leaves the march off (P1, P2) | frame test `specular_style_and_surface_depth_switch_independently` (Mac GPU) | achievable as stated |
| 16. Billboard specular byte-identical under pixel and smooth | frame test rendering the billboard pass under each style, comparing bytes (Mac GPU) | achievable as stated |
| 17. Repository gate rejects option names, settings keys and `options.*` slots naming specular style, sharpness or tint | `environment_gates.rs` `player_options_name_no_specular_style_sharpness_or_tint` | achievable as stated |
| 18. The surface example as a TS and Luau fixture manifest selects smooth; `content/dev` leaves pixel | `staged_manifest/surface_example_tests.rs` cases plus a dev-manifest assertion | achievable as stated |
| **Sharpness read** | | |
| 19. Four-channel slot, each style: byte 0 → exponent 1, 255 → 8192, 128 → 2/r⁴ − 2 at r = 127/255; past-cap bytes give 8192 (255 by readback); both decodes floor r⁴ (P12, source gate); no `_p` gives the prefix exponent | CPU reference tests in `render-cpu` (new `specular_texel` module) with a constants-match test against `material_shading.wgsl`; readback `sharpness_decode_matches_the_cpu_reference`; source gate on the r⁴ floor | achievable as stated |
| 20. Surface Depth Off then On keeps sharpness and restores the march: `_p`+`_h`, `_p` alone, untuned prefix (P3) | frame test over the three materials (Mac GPU) | achievable as stated |
| 21. Built at Off, and through Off then On, `_p` keeps sharpness and `_t` keeps tint in both passes (P11) | frame test building at Off then switching (Mac GPU) | achievable as stated |
| **Tint** | | |
| 22. No `_t`: byte-identical to the previous build in both passes, each style; one- and two-channel slots and a `_p` slot with A = 255 | `untinted_specular_matches_the_frozen_pre_change_function` (both harnesses) (Mac GPU) | achievable as stated |
| 23. Marker slots (`_p` B = 1/A = 255; `_t` B = 0/A = 254) read correctly magnified at a seam, minified between mips and at a grazing angle, both passes (P15) | `marker_slots_keep_their_meaning_through_filtering` with real mip-chained textures and the aniso sampler (Mac GPU) | achievable as stated |
| 24. Real one-channel, two-channel and placeholder slots, sampled as textures, shade in raw albedo exactly as before, both passes (P8) | `narrow_specular_slots_shade_in_raw_albedo` (Mac GPU) | achievable as stated |
| 25. `_t` semantics: t = 0 white × intensity; t = 1 identical on dark and bright albedo of one hue; grey/black glints white; mid lies between | CPU reference tests plus readback `tint_mix_matches_the_cpu_reference` (Mac GPU) | achievable as stated |
| 26. Diffuse byte-identical with and without `_t`, and at t = 0 and 1 | readback with the specular term masked off (Mac GPU) | achievable as stated |
| 27. Mover reads tint as forward does, each style | parity test (row 5) plus mover tint readback | achievable as stated |
| **Bake** | | |
| 28. No `_p`/`_t`: same sidecar bytes and key; an existing sidecar is a cache hit | extend `height_absent_bundle_hash_matches_the_pre_height_algorithm` and `heightless_bundle_is_a_cache_hit_and_is_not_rewritten` (`texture_mips` tests) | achievable as stated |
| 29. `_p`, `_p`+`_s`, `_p`+`_h`, all three: four-channel with R, G, B as decided, A = 255 every mip, SPECULAR bit, reads back through the runtime `.prm` reader | `texture_mips/world_tests.rs` `smoothness_bundles_bake_four_channel` | achievable as stated |
| 30. `_t`, `_t`+`_p`, all four: A = min(t, 254); `_t` alone B = 0 every mip and the prefix exponent; identical bundles under two prefixes share one sidecar, each at its own exponent | `world_tests.rs` `tint_bundles_bake_four_channel`; exponent half by readback through the loaded slot | achievable as stated |
| 31. Per-mip markers held beside a hard 0/255 edge (P13) | `texture_mips` `marker_clamp_holds_at_every_mip_beside_a_hard_edge` | achievable as stated |
| 32. Four-channel without `_h`, tuned prefix, On: relief row byte-equal to no height map (P7) | `material_plan` / `surface_depth` uniform test from a loaded four-channel slot | achievable as stated |
| 33. With `_h`, four-channel sets has-depth; per-mip band equals the two-channel band; sharpness bytes differ from height | `loaded_texture.rs` test beside `specular_relief_is_measured_from_the_uploaded_surface_map` | achievable as stated |
| 34. Dimension bails for `_p` (vs diffuse, `_s`, `_h`) and `_t` (vs diffuse, `_s`, `_h`, `_p`), naming both files; matching passes | `world_tests.rs` per pair | achievable as stated |
| 35. Mismatched `_n` builds with or without `_p`/`_t`; mismatched `_s` builds when neither (P14) | `world_tests.rs` | achievable as stated |
| 36. `_p` or `_t` with sRGB, iCCP or non-unit gAMA fails validation | `texture_validation.rs` tests | achievable as stated |
| 37. One mip down, a 2×2 of B = 0/255 filters to about 128 (B-stride guard) | `texture_mips` chain test | achievable as stated |
| 38. Diffuse, normal and emissive bake byte-identical with and without `_p`/`_t` | `world_tests.rs` | achievable as stated |
| 39. Byte accounting reports 4 B/texel per mip for a four-channel specular slot | `prm_accounting` test over a baked four-channel bundle | achievable as stated |
| 40. Key distinctness: diffuse+`_p` ≠ diffuse; `_p` alone non-zero; same for `_t`; `_p` ≠ `_t` from the same bytes; none overwrites another | `texture_mips/cache.rs` tests | achievable as stated |
| 41. TrenchBroom excludes `*_p`, `*_t`; the material-input gate's pinned suffix lists name both | `sdk/TrenchBroom/GameConfig.cfg` edit plus `environment_gates.rs` assertion that the config excludes every pinned non-diffuse suffix; pinned lists updated | achievable as stated |
| **Docs** | | |
| 42. `docs/level_design.md` documents `_p`/`_t` (mapping, tint rule, linear, dimensions, PBR conversion); `docs/scripting-reference.md` documents `render.specular.style` | review at task 8, checked against the row's list | achievable as stated |
| 43. Context docs describe smoothness, the 8192 cap, A tint, both markers, the per-mip clamp, and `_t` beside `_p`; no stale encoding remains; a `_t`-only slot shades at its prefix; `player_options.md` names tint | review plus a grep at preflight for "log", "256", "unused A" and "decided, not yet built" in the touched sections | achievable as stated |
| **Manual** | | |
| M1. Captures under a static light vs the previous build: pixel turns terrain and wood into texel blocks, concrete unchanged; smooth (scene key) matches the previous build | owner, from executor-made captures of the dev map's comparison strip at 1c7fc9e3a and at HEAD | manual |
| M2. Slow camera move across a glossy face under pixel: no movement inside any texel | owner, in-engine | manual |
| M3. `_p` ramp: tight highlights on sharp texels, broad elsewhere, identical on static face and mover | owner, in-engine (mover lit while its light is promoted) | manual |
| M4. `_t` inlay: white on the field, hue on the inlay, identical on static face and mover | owner, in-engine | manual |
| M5. Carved material with `_p`: top and side faces each hold one value per texel face | owner, in-engine on `concrete_carved_gloss`. Observation, not a gate: compare the carved door with its static frame under the close point light. A visible glint difference goes to a follow-up brief, not this one | manual |
| M6. Under pixel, a static shadow edge cuts the highlight on the diffuse's line | owner, in-engine | manual |
| M7. Minification sparkle at distance: measured and reported, not a gate | executor reports; owner judges | manual |
| M8. Mac, Surface Depth Off: smooth and pixel cost vs previous build; mover register count below the documented cliff in both styles | executor on the Mac: clock-pinned Metal System Trace vs a 1c7fc9e3a binary, register screen; owner reviews | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Forward texel-centre reconstruction and readback (first slice).** Factor the texel-UV reconstruction out of `sample_post_retro` into `material_shading.wgsl`, and have forward's copy call it. Add the shared point builder (Jacobian from the albedo lattice, zero axes on degeneracy). Forward `fs_main` builds the point through it from `in.uv`, `shade_uv`, `in.world_position` and the hoisted derivatives, and passes it for L and V to `world_static_specular`. Bit 2 of `spec_shadowmask_flags` selects smooth; no writer exists yet, so pixel is live. Extend `world_specular_test.rs` to a 2D target with UV, a texel grid, per-fragment visibility, the style bit and a frozen 1c7fc9e3a copy. Tests the seam-band and degenerate-Jacobian premises before any format work. Proves rows 1–4 and row 7's forward half. Mac GPU. | integrating executor | — | done: shared `post_retro_texel_uv` + `specular_texel_point` in `material_shading.wgsl`; harness split into `world_specular_harness.rs`, frozen oracle `world_specular_frozen.wgsl`; 10 new tests pass on Metal with `POSTRETRO_REQUIRE_GPU=1`, smooth bit-identical to 1c7fc9e3a across 9 configs; renderer lib 1400 passed; mutation check (marched-UV anchor, floor+0.5, no degeneracy guard) fails the new tests. Task 2 must re-anchor `fluid_post_retro_sampling_matches_the_mover_shader` and `kinematic_ghost_sample_post_retro_matches_the_mover_pass_copy` |
| 2 | **Mover pixel-style specular.** Mover `sample_post_retro` calls the shared UV function. `_pad2` becomes the style word. Restructure the selected-static specular in `accumulate_dynamic_direct` per the Corrections: point at the struck texel's top height, own L, V, falloff, range and cone, the shadow kept as a separate factor, no per-pixel influence or N·L gate. Untinted accumulation order stays as today. Add the parity test and the new mover harness `kinematic_specular_test.rs`. Proves rows 5–6 and row 7's mover half. Mac GPU. Screen the mover register count here, before later tasks build on it. | integrating executor | 1 | |
| 3 | **Style chain.** Scripting-core `ModSpecularStyle`/`ModSpecularProfile`, the JS and Luau readers, `sim` `primitives/manifest.rs` registration and regenerated `sdk/types`. In `render_profile.rs`: the chokepoint and `apply_mod_specular_style`, called at both commit sites and pinned by the source test. Render-cpu: a `SpecularStyle` enum and its `FrameUniforms` bit. Renderer: the boot-state field, its setter, the per-frame forward and mover writes, and the capture constructor default. Capture: `specular_style` in `capture/scene.rs` and `setup.rs`. `environment_gates.rs` gains the option-name test. Example fixture tests. Proves rows 8–18. Frame tests need the Mac GPU. | worker: `crates/scripting-core`, `crates/sim/src/scripting/primitives/manifest.rs`, `sdk/types`, `crates/postretro/src/{startup,capture}`, `crates/render-cpu/src/frame_uniforms.rs`, the renderer's boot-state field and per-frame writes; the integrating executor merges `environment_gates.rs` with task 4 | 1, 2 | |
| 4 | **Bake `_p` and `_t`.** `resolution.rs` (`BUNDLE_SUFFIXES`, lookups, the `bundle` slot list), `cache.rs` (hash bits 5–6, tags 0x05–0x06, single-input keys), `world.rs` (reads, dimension bails, Rgba8 interleave, SPECULAR bit), `texture_mips.rs` (`build_surface_chain_impl` channel count plus per-mip marker clamp), `texture_validation.rs` suffix chain, `prm.rs` `format_allowed_for_slot`, `GameConfig.cfg` excludes, the pinned suffix lists in `environment_gates.rs`, and the `smoke.rs` comment. Proves rows 28–31, 34–38, 40–41. No GPU. Can run concurrently with tasks 1–3 in a worktree. | worker: `crates/level-compiler/src/texture_mips*`, `texture_validation.rs`, `crates/level-format/src/prm.rs`, `sdk/TrenchBroom/GameConfig.cfg`, `environment_gates.rs` suffix lists | — | done (920d2b919 + lint fix): `BundleSources` replaces the positional hash args; slot-1 bake split into `texture_mips/specular_slot.rs`; absent markers forced at every mip. Row 28's tests renamed: `absent_siblings_keep_the_pre_sibling_bundle_hash`, `absent_siblings_keep_their_filename_key`, `bundle_without_smoothness_or_tint_is_a_cache_hit_and_is_not_rewritten`. Row 31 test first proves the unclamped filter rings onto both markers. level-compiler lib 253 + all targets 2643 passed, level-format prm 53, xtask environment_gates 14; clippy `-D warnings` clean |
| 5 | **Load the four-channel slot.** `specular_slot_is_surface_map` accepts `Rgba8Unorm`, and its test changes. `specular_surface_relief` gate. Stride-aware relief reader in `render-cpu` `surface_depth/height.rs`. Accounting test. Proves rows 32, 33 and 39, and the runtime-reader halves of rows 29–30. | worker: `crates/render-cpu/src/surface_depth/height.rs`, `crates/renderer/src/render/{loaded_texture.rs,material_plan.rs}`, `crates/level-format/src/prm_accounting.rs` tests | 4 | |
| 6 | **Sharpness and tint in both passes.** Add the smoothness decode (r⁴ floored at the cap's value) and the tint mix (hue = albedo / max channel, white near zero) to `material_shading.wgsl`. Mirror both in a new `render-cpu` `specular_texel` module with a constants-match test. Forward and mover sample the slot's RGBA once. B ≥ 0.5/255 selects the texel exponent; A < 254.5/255 selects tint and moves specular out of the albedo multiply. Proves rows 19–27 and the sharpness and tint halves of row 5. Mac GPU. Re-screen the mover register count. | integrating executor | 2, 5 | |
| 7 | **Dev fixtures and map.** Generator and its test, the PNGs, map and README per Delegated answers. Compile the map with `prl-build` and run it once to confirm it loads, the materials bake four-channel and the movers draw. | worker: `tools/gen_specular_texel_fixtures.py`, `tools/tests/`, `content/dev/textures/test/`, `content/dev/maps/specular-texel-sharpness.*` | 4 (authoring may start at once; verification after 6) | authored: 10 PNGs + generator (5 byte tests pass), map compiles with 0 warnings, 3 movers, 2 static light candidates; sidecars decode to the planned bytes. Carved tiles are 3×3 inside 4-texel cells (16 texels cannot hold 4×4 tiles plus mortar). `tools/migrate_neon_prefix.py` suffix lists brought in line with prl-build's new `_p`/`_t` bundle suffixes. Remaining: in-engine load after task 6, captures after task 3 |
| 8 | **Docs.** `docs/level_design.md`, `docs/scripting-reference.md`; the routed `context/lib` sections, with the "decided, not yet built" markers dropped. Proves rows 42–43. | integrating executor | 3, 6 | |
| 9 | **Mac measurement and manual capture prep.** Build a 1c7fc9e3a comparison binary, then delete its target. At Surface Depth Off, take clock-pinned Metal System Traces of both styles against it, pilot-validated, with caffeinate and screen-saver checks. Screen the mover register count in both styles; a count past the cliff stops for the owner. Produce the M1 captures from the dev map's two scenes and the M7 report. Then hand M1–M8 to the owner. | integrating executor (owner for the visual rows) | 6, 7 | |

Concurrency: task 4 starts in a worktree beside task 1. Task 3 starts after 2, because its frame tests read the shader's style bit. Task 5 waits on task 4's chain format. The integrating executor merges task 3's and task 4's `environment_gates.rs` changes, which touch distinct tests and lists. It also runs every Cargo check and test after each merge.
