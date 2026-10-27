# coloured-sh-bounce — research

Read at `25db4c78f` (`origin/main`). Findings that inform the brief; the brief's Decisions are the record.

## Estimator today

- `sh_bake.rs` `BOUNCE_ALBEDO` (0.45) has one production read: `sample_radiance_rgb` returns `radiance * BOUNCE_ALBEDO / π` at the hit. The other reads are in tests (`sh_bake.rs` tests, `sh_bake/bounce_tests.rs`), which will need the table or the grey constant.
- Callers: `bake_probe_rgb_with_moments`, reached from `bake_probe` (id 34; `sh_group.rs` warm groups and the cold whole-volume path), and `bake_probe_indirect_rgb` (id 27; `delta_sh_bake.rs`). Both pass a `RaytracingCtx`.
- `bake_probe_direct_rgb` (direct SH), the animated-direct, billboard-scatter, lightmap and weight-map bakes never touch `BOUNCE_ALBEDO`. `RaytracingCtx` is shared with direct SH, so a table on it costs those callers a field, not a behaviour change.
- `Hit` holds point, normal and distance. `closest_hit_in_leaf` has the `BvhPrimitive`, whose `material_bucket_id` is `face.texture_index` (`bvh_build::collect_primitives`); its doc says "currently the face's texture index; the struct shape is stable if bucket criteria expand". A face with `geometry::NO_TEXTURE` (u32::MAX) or an index past the table is "no texture" (the reflection capture's own rule in `reflection_probes/materials.rs`).
- `ray_triangle_hit` is double-sided, normal flipped toward the ray; a back-face hit takes the same texture's mean.
- The SH bake runs before AtlasPreparation, on pre-cut faces; the cut does not change a face's texture.
- Grey's history: the hue was a tooling limit (`plans/done/lighting-animated-sh/index.md` Q4: "Texture images are never loaded during the bake"). The 0.45 magnitude has no recorded measurement. It was introduced as 0.5 (9293a410e), set to 0.45 in a plan-review consistency edit (e6c8cf777, "was inconsistently 0.7/0.5"), then in code (b1fcbbcd2). The plan's "tuned to avoid over-brightening" sentence (Albedo fallback for Task A) arrived with that edit, not with a look test. The world's 0.33 indirect scale is an owner mood choice (308ab8f99, "Make the default mood darker", 1.0 → 0.33). So the A/B judges the retune by look, not against a calibrated grey.

## Stage order and the texture table

- The symbol is `pipeline/stage_registry.rs` `ORDERED_STAGES`. Order: … Geometry, BvhBuild, CellVisibility, NavMesh, ShBake, DeltaShBake, … ChunkLightList, AtlasPreparation, … SdfAtlasBake, TextureMips, ReflectionProbes, ClusterDirectory, Packing. `tui/tui_steps.rs` lists the same steps.
- Geometry interns world names (`geometry.rs`, encounter order), then `kinematic_geometry::encode_kinematic_geometry_section` appends mover names, all inside the Geometry stage. At ShBake the table holds world and mover names.
- `pipeline.rs` runs `encode_environment_volumes_section` (fluid-only appends) right before TextureMips, then `bake_world_texture_mips(names, texture_root, prm_root)`, `bake_model_textures`, `bake_sprite_textures`, and one `log_texture_byte_summary`. The comment there restates the §Fluid surfaces commitment.
- `bake_world_texture_mips` creates its own `StageTextureBytes` and hands it back with `name_to_key`. The fluid tail needs to fold into the material stage's accounting rather than start a fresh one.
- `name_to_key` is a `HashMap<String, [u8; 32]>`. Pack builds TextureCacheKeys by walking `texture_names` in order, and reflection probes read the map by name. An early and a late phase can therefore merge maps without moving indices.
- The byte report charges a bundle once per cache key, and the first name to reach it owns the line: "world bundles bake first, so a shared address reports under its world material name" (`build_pipeline.md` §Baked texture mips, `StageTextureBytes`).
- `bake_world_texture_mips` reads every slot's PNG bytes on every build, to derive the filename key, even on a sidecar hit. A hit then reads and parses the whole `.prm` (`PrmFile::from_bytes_partial`). An unreadable source PNG fails the build today; a name with no slot sources gets a zero key. A hit therefore holds mip 0 of the diffuse and `_e` slots on every warm build, and stats need no decode (§Derive, don't store).
- `TextureValidation` (early stage) reads PNG metadata only; no pixel decode.
- `texture_mips.rs` has no `StageCache` key; sidecars are content-addressed.

## Shared material stage

The chosen shape: materials resolve before any lighting bake, and every bake consumer samples them through one shared material module.

- **Consumers.** SH bounce needs per-texture means before ShBake. Reflection probes already sample albedo and emissive by footprint after TextureMips. `drafts/surface-radiance-cache` patch radiance is albedo × irradiance during the lighting bake ("Albedo is whatever the bake uses when this builds"); its patch size is delegated, so patch albedo may need sub-texture footprints, which only the mips give. The roadmap's surface lights (a light entity templated across every face of a named texture; after this brief, not yet drafted) need the emissive slot, and an emissive colour, at the same point.
- **`SurfaceMaterials` today** (`reflection_probes/materials.rs`). Used only by reflection probes: `reflection_probes/mod.rs`, `shading.rs` (a field of the shading context) and `capture_exclusion_test_fixtures.rs`. Built from texture names, `name_to_key` and the `.prm` root; loads each texture's diffuse and emissive chains lazily per index (`OnceLock`), sampling at the mip the texel footprint selects. `MipChain::from_slot` accepts `Rgba8UnormSrgb` only. A missing sidecar, `NO_TEXTURE`, an index past the table, or no diffuse returns the renderer's magenta/black checker. Lighting's fallback is 0.45 grey, so the fallback becomes the consumer's choice.
- **Means.** The exact linear average of every full-resolution texel not masked as glow (§Glow exclusion), summed in f64 in a fixed texel order, derived each build from the sidecar's mip 0 (§Derive, don't store). No downsampled level: those are 8-bit sRGB, and Mitchell-Netravali downsampling overshoots (§Diffuse chain).
- **Rival: a means-only pass ahead of ShBake, TextureMips left in place.** It keeps the §Compiler pipeline order and touches no byte report. It loses on two counts: it adds a second PNG decode path beside the mip decode, and `surface-radiance-cache` redoes it if patch albedo needs footprints, because means cannot serve a sub-texture footprint. It is equal when that cache's patches are at least one texture tile in size, since a per-texture mean is then the right patch albedo.
- **Checks under the shape.**
  - Byte report: one `StageTextureBytes` threads the material stage, the fluid tail, then models and sprites, and logs once at the late boundary. One report holds.
  - "World bundles bake first": the material stage is the first to charge any key, so a shared address still reports under its world name. Holds.
  - `build_pipeline.md` §Fluid surfaces: fluid surfaces still build after geometry and movers and append past every world name; the fluid tail bakes them, and no SH key sees them. The commitment holds. Its wording "just before TextureMips" still holds: TextureMips keeps the fluid tail.
  - Code order (`ORDERED_STAGES`) runs the mip bake after every lighting stage. `build_pipeline.md` §Compiler pipeline never names it; only step 13 implies it. This shape moves the world-and-mover half ahead of ShBake because SH bounce now reads texture content, and promotion adds the material stage to that list.
- **The move lands here.** `SurfaceMaterials` and its footprint sampling move into the shared module in this brief, not with the first consumer outside reflections: `surface-radiance-cache` and surface lights arrive next, and the reflection-probe digest checks the move byte for byte.
- A texture error now fails before the long bakes rather than after them.

## Diffuse chain and the mean

- `build_diffuse_chain_impl` decodes RGB through the sRGB LUT and alpha linearly, re-encodes mip 0, then repeats `downsample_2x_f32`. Its comment says the re-encode round-trips the source bytes exactly, "verified empirically by the unit test". The nearest test, `gamma_correct_constant_input_is_invariant`, allows ±1 LSB, so no test pins exactness. Checked this session: `linear_to_srgb_u8` of every `build_srgb_to_linear_lut` entry returns its own byte for all 256 values, and `linear_to_unorm_u8(a / 255)` returns `a`, with the same f32 formulas. Mip 0 is therefore the `decode_png_rgba` output byte for byte, at source dimensions, in both slot 0 and slot 3. The reused-vs-rebake row uses a fixture covering every byte value, which pins it.
- `downsample_2x_f32` is separable Mitchell-Netravali (B = C = 1/3) with clamp-to-edge taps, and halves by `(w / 2).max(1)`. So the 1×1 level is not the mean: negative lobes, edge replication, odd sizes truncated each level, and 8-bit sRGB storage. Average the full-resolution decode directly.
- `pad_rgba8_clamp_edge` is called only from `build_normal_bc5_chain_impl`. It never touches the diffuse chain.
- `build_diffuse_chain` also bakes the `_e` slot (same sRGB decode, filter and encode), so a mean accumulated inside it would run for emissive too. Computing it beside the chain, from mip 0 of slots 0 and 3, keeps it on the diffuse, gives it the `_e` mask, and serves rebake and hit alike.
- An f64 sum of at most 65535² texels with values in [0, 1] loses nothing that survives the 8-bit snap. A fixed texel order makes it independent of worker count.

## Snapping

- **Step: 8-bit sRGB.** Each channel of the linear mean encodes to 8-bit sRGB (`linear_to_srgb_u8`), and the bake uses its decode through `build_srgb_to_linear_lut`. That pair round-trips every byte exactly (§Diffuse chain). The SH keys fold the 8-bit values; the bake's albedo is a pure function of them and the saturation constant, so bytes always agree with the key. Every decoded value lies in [0, 1].
- **Why that step.** It is the precision authors paint in, so the bounce never resolves a difference the texture cannot express. One step at full brightness is 0.0089 linear, under 1% relative, near the visible threshold for a whole pixel. The bounce reaches the screen smaller than that: the indirect scale (0.33) and the receiver's albedo multiply it, and direct light adds beside it, so a step in albedo moves the final pixel by less than its own relative size. The steps are finer in the darks in linear terms (0.0003 at the bottom, 0.0012 at sRGB 50), which suits the dark mood: dark textures, which carry most of the mood, snap closest.
- **One-texel edits.** A one-texel change on a 64×64 texture moves a channel's mean by at most 1/4096 ≈ 0.00024 linear, less than the smallest step (≈ 0.0003). It can therefore move a snapped mean by at most one step, and only when the mean sits within that distance of a rounding boundary. At `campaign-test`'s typical means (about 0.2 linear, a step of about 0.0035) that is under one in ten even for a black-to-white texel; a realistic touch-up is rarer still.
- **Boundary.** Quantization has a boundary: an edit that moves no visible colour can still carry a mean across a rounding boundary and re-key. The hit guarantee is for an edit that leaves every snapped mean unchanged; the fixture picks a one-texel edit whose mean stays inside its bucket, and a regional repaint that moves it.
- **Order.** Snap each channel of the raw linear mean first, then apply saturation. Saturating first and snapping the result would also agree with the key, but a small saturation change could then leave every snapped value in place and hit, which breaks "changing the constant re-keys".
- The grey fallback is not a mean and bypasses snap and saturation, so an all-unresolved bake keeps today's floats exactly.

## Glow exclusion

- **Rule.** A texel is left out of the mean only if it renders as glow: its `_e` texel has a non-zero RGB channel and its material's emissive strength is above 0. No `_e`, zero strength, or every texel masked takes the full average.
- **Why.** The mean stands for reflectance. `tools/gen_emissive.py` keeps a diffuse texel's own sRGB colour wherever its luminance clears a cutoff (0.35 for `neon_`, 0.75 otherwise) and writes black elsewhere, so on `neon_`-style textures the glow is painted into the diffuse as well. Averaged in, it would make a neon panel bounce its glow colour, lighting its neighbours through SH, which `resource_management.md` §4.5 rules out for emissive maps. The planned surface lights add the glow back as emission; a mean that already carried it would count it twice.
- **RGB, not alpha.** `gen_emissive.py` writes non-glow texels as opaque black (alpha 255), and the forward shader reads `_e`'s `.rgb` and adds it, ignoring alpha. A mask on alpha would mark every texel and fall back to the full average.
- **Content and strength.** `Material::emissive_strength` is 4.0 for `neon_` and 0 for every other prefix (`render-data/material.rs`), and the forward shader scales `_e` by it, so an `_e` on a zero-strength prefix adds nothing and those texels render as plain diffuse. Masking them would drop a surface's bright texels from its mean and make it bounce darker than it looks. Gating on strength makes the mask match what renders; the reflection capture already gates its emissive the same way (`SurfaceMaterials::sample`, `emissive_strength > 0.0`). Today's content has `_e` only on `neon/neon_dim_panel`, `neon/neon_glow_panel` and the fluid `fluids/lava_64`, which no SH key sees.
- **Strength at the material stage.** Strength is a pure function of the texture name: `derive_material(name, …).emissive_strength()`, already called by `SurfaceMaterials::load` and `stage_key::cache_key`. `bake_world_texture_mips` iterates the names, so each is in hand. The sidecar's filename key is content-only, so two names with the same bytes share a sidecar yet may differ in strength: stats are per name, not per sidecar. A change to the strength table moves the means, and the SH keys fold the means, so it re-keys without a new key input.
- **Source path (`25db4c78f`).** `resolve_texture_bundle_paths` finds `<base>_e` among `BUNDLE_SUFFIXES`. `bake_world_texture_mips` reads its bytes on every build (they enter the filename key and bundle hash), bails before the cache check when its dimensions differ from the diffuse's, and on a rebake decodes it with `decode_png_rgba` into the fourth `.prm` slot (`slots_arr[3]`, `PrmSlots::EMISSIVE`, `Rgba8UnormSrgb`). A texture without `_e` has no mask.
- **Edits.** A mask change moves the mean only through which texels count; an `_e` intensity change on already non-zero texels moves nothing, so the SH keys hit even though the sidecar key changes. On a zero-strength prefix no `_e` edit moves the mean.

## Saturation

Bounce albedo `a = s·c + (1 − s)·L`, where `c` is the snapped linear mean, `L = w·c` with Rec. 709 linear weights `w = (0.2126, 0.7152, 0.0722)` (the repo's luminance weights: `render-cpu/flash_clamp/value.rs`, `bloom_extract.wgsl`), and `s` is the bake-wide constant in [0, 1].

- **No channel above the mean's maximum.** Every `w_j ≥ 0` and `Σ w_j = 1`, so `L` is a convex combination of the channels: `min(c) ≤ L ≤ max(c)`. Each `a_i` is a convex combination of `c_i` and `L`, both `≤ max(c)`, so `a_i ≤ max(c) ≤ 1`, and `a_i ≥ min(c) ≥ 0`. Later multi-bounce therefore gains no energy. A single channel can rise (a red wall's blue goes up toward `L`), so "per channel dimmer-or-equal than `c`" is not claimed; only the bound by `max(c)`. It needs `s ≤ 1`: `s > 1` extrapolates and can exceed `max(c)`.
- **Luminance preserved.** `w·a = s·L + (1 − s)·L·Σw = L`. With a white light every hit's irradiance is equal per channel, so the probe's luminance is `Σ E_h · L_h` at any `s`: identical in exact arithmetic, separated only by float rounding. A coloured light breaks this, so the fixture row uses a white light.
- **Neutral at 0.** `s = 0` gives `a = (L, L, L)`; with a white light every channel runs the same arithmetic, so the probe reads R = G = B.
- **Bit-exact at 1.** Written as `s·c + (1 − s)·L`, `s = 1` returns `c` exactly (`1·c + 0·L`). The form `L + s·(c − L)` does not.
- **Precision.** Id 34 ships BC6H unless `--uncompressed-irradiance`, so luminance and neutrality rows read the baked probe values before encoding.

## Alpha cutout

- No world alpha cutout exists. `forward.wgsl` and `depth_prepass.wgsl` contain no `discard`; the world forward pipelines set `blend: None` (`renderer_init_pipelines.rs`). Only ghost and fluid-reflection shaders discard.
- So every BVH face draws opaque, and a mean over RGB that ignores alpha matches what the eye sees. Fluid faces are not in the BVH (§Fluid surfaces: SH and BVH are byte-identical without volumes).

## Cache keys

- `sh_group::group_cache_key` folds reach cutoff, group dims, reaching lights, `geometry_content_hash`, layout and validity. `delta_sh_cache::delta_sh_entry_cache_key` folds the geometry hash, affinity coords, spacing, valid mask, seed axis and the keyed light. Neither folds texture content. Versions: `SH_GROUP_STAGE_VERSION`, `INDIRECT_DELTA_SH_STAGE_VERSION`.
- `geometry_content_hash` postcards the whole `GeometryResult`, texture names included, so a retexture already re-keys SH; a PNG edit does not. Its other consumers are `billboard_direct_scatter_bake`, `direct_sh_bake` and `chunk_light_list_bake`. Folding means into it would re-key stages that read no albedo.
- Fluid-only names append after every pre-ShBake hash, so they never reach SH keys today; folding only static-world-face indices keeps that.
- Precedent: `reflection_probes/stage_key::cache_key` folds, per texture index some static-world face uses, the name, the `.prm` key and emissive strength. Its SH input is the packed SH bytes, so it re-keys automatically. It also folds `PRODUCTION_INDIRECT_WEIGHT`.
- Precedent for a code constant in a key: the lightmap texel-step threshold (`build_pipeline.md` §Build Cache, `lightmap_section`).
- Restoring an old saturation value hits only while the earlier entries survive eviction. Entries are one file per key; the sweep spares each map's last record and evicts the rest oldest first down to `--cache-max-size` (default 2 GiB). Under budget, the restore row holds.

## Warm and cold

"Warm equals cold" holds for id 27 only. `build_pipeline.md` §Build Cache: delta entries are exact (byte-identical warm and cold), while warm id 34 is a bounded-reach approximation, per channel dimmer-or-equal to cold. With albedo the same per-texture factor scales every kept light's term in both paths, so dimmer-or-equal still holds per channel. What the brief adds is that the means must not depend on whether a sidecar was reused or rebaked.

## Derive, don't store

- **Exact from the sidecar.** Mip 0 of slots 0 and 3 is the decoded source, byte for byte (§Diffuse chain). A rebake holds the `decode_png_rgba` output; a hit holds the payloads `PrmFile::from_bytes_partial` parsed, and `cache_entry_has_valid_declared_slots` accepts the hit only when every declared slot parsed. Either way the stats see the same bytes, so a reused sidecar and a rebake give equal means.
- **Reads what ships.** `baked/materials/` is the runtime's texture source (`build_pipeline.md` §Build Cache), so bounce reads the bytes the game shows, not a parallel copy that could drift from them.
- **No second tree.** A stored mean needs a format, a key, a version and invalidation, plus a §Build Cache doc touch, and it would duplicate a value one texel pass recomputes. The `.prm` is the wrong home too: the runtime never reads the mean, so a field would bump a shipped format for a bake-only value. Later consumers (patch albedo, surface lights' emissive colour, reflections) add a derived stat to the same table with none of that.
- **Cost.** One pass over mip 0 of each world diffuse and `_e` per build, warm or cold. A warm texture bake did no texel work before; the manual bake-time row measures the new warm time. Persist a stat only once that measurement shows the cost.
- **Lifetime accounting** (`development_guide.md` §1.4, `testing_guide.md` §Resource bounds).
  - Production: the texture-bake loop already holds, per name, every slot's source bytes, plus on a hit the `.prm` bytes and parsed slots, or on a rebake the decoded RGBA, chains and encoded `.prm`. Stats read mip 0 in place and add a few f64 accumulators. No new texel buffer.
  - Buffering: one texture at a time; the loop drops its buffers before the next name, as today.
  - Serialization and persistence: none. No file or cache entry is written for stats.
  - Return: one fixed-size record per texture name (snapped mean and its flags), resident for the build.
  - Cleanup: the record table drops with the pipeline's stage results.
  - Lazily loaded footprint chains (`SurfaceMaterials`' per-index `OnceLock`, used by reflections) keep their lifetime.
  - Proof: a test-only high-water mark of texel bytes the stats path holds, and the no-new-file row. The seam proves only buffers routed through it; the rest are today's loop, unchanged.

## Animated and static parity

- No test on `origin/main` compares an animated light's indirect delta with the static base bounce. The indirect delta tests are exact against their own estimator: `delta_sh_bake.rs` `subblock_stores_indirect_only_not_direct_plus_indirect` (bit-for-bit against `bake_probe_indirect_rgb`) and `hard_light_delta_subblocks_match_full_set_bit_identically`.
- The one delta-versus-base tolerance is `direct_sh_bake.rs` `direct_sh_delta_subtraction_matches_bake_excluding_selected_light`: `DELTA_SUBTRACTION_EPSILON` = 0.02, absolute per channel, on decoded atlas values. The parity row borrows it.
- The base seeds soft visibility by the light's global index; the delta seeds it by the keyed light's own axis (`indirect_light_axis`) at unit radiance. Soft lights therefore differ by sampling noise between the paths; a hard fixture light traces one ray either way.
- Colour is one per-hit multiply in `sample_radiance_rgb`, which both paths share, so it scales both sides of the comparison alike.

## Indirect-scale consumers

`DEFAULT_INDIRECT_SCALE` (`renderer_types.rs`, 0.33) feeds the shared group-0 `Uniforms.indirect_scale`; the diagnostics slider (`debug_ui/mod.rs`, [0, 1]) sets it at runtime. Shaders that sample the SH indirect atlas: those that concatenate `sh_sample.wgsl` (`pipeline_layout.rs`, `fluid_surface.rs`, `kinematic_brush.rs`, `mesh_pass.rs`, `smoke.rs`, `fog_pass.rs`; `sh_volume.rs` only in tests).

| Shader | SH indirect read | Applies `indirect_scale` today | In scope |
|---|---|---|---|
| `forward.wgsl` (world) | `sample_sh_indirect` | yes, `* uniforms.indirect_scale` | yes |
| `fluid_surface.wgsl` | `fluid_sample_sh` | yes, `sh.a * uniforms.indirect_scale` | yes |
| `kinematic_brush.wgsl` (movers) | `sample_sh_indirect` | no; `KinematicLightParams` has no field (ambient floor and two pad words) | yes |
| `skinned_mesh.wgsl` | `sample_sh_indirect` | no; `MeshLightParams` has no field (ambient floor and two pad words) | yes |
| `billboard.wgsl` (via `smoke.rs`) | `sample_sh_indirect` | declares the field in shared `Uniforms`, never multiplies | yes |
| `fog_volume.wgsl` | `sample_sh_indirect_corners_two_without_depth` | no | no: per-volume `ambient_scatter`, `min_brightness`, `tint`, `saturation` |

Not SH consumers: `billboard_direct_scatter_compose.wgsl`, `fluid_reflection_pass.wgsl` and `wireframe.wgsl` declare `indirect_scale` only as a mirrored `Uniforms` field; `fluid_reflection.wgsl` samples the reflection-probe arrays, not SH; `sdf_shadow.wgsl` and `lightmap_sample.wgsl` read SH grid metadata only; `sh_compose.wgsl` writes the atlas. No runtime CPU path samples SH indirect.

Compile side: `reflection_probes/shading.rs` `PRODUCTION_INDIRECT_WEIGHT` is 0.33, equal to `DEFAULT_INDIRECT_SCALE`; `indirect_weight_matches_the_renderer_production_default` parses `renderer_types.rs` to keep them equal.

Guard precedent: `render/tests/shader_tests.rs` asserts the forward source contains `* uniforms.indirect_scale;` and never `select(uniforms.indirect_scale, 1.0`. A per-consumer guard extends that pattern; the mover and mesh uniforms also need their uploaded value tied to the renderer's one scale, which a source guard alone does not prove. `fluid_baked_frame_test` is a GPU frame test that already varies `indirect_scale`.

Look change: movers, skinned meshes and billboards read SH indirect at full weight today, about three times the world's. Unifying drops them to the world's scale. Landing it as its own first commit gives the colour A/B a baseline of grey bounce at one scale, so entity dimming does not confound the colour judgment.

## Expected energy

Linear means of `campaign-test`'s diffuse textures, by brush-side use in the `.map` (a pure-Python PNG decode, sRGB LUT per IEC 61966-2-1; side counts, not area):

| Texture | Sides | Linear mean RGB | Luminance |
|---|---|---|---|
| concrete_stone_030 | 348 | 0.208 0.223 0.207 | 0.218 |
| concrete_stone_022 (two names) | 181 | 0.168 0.171 0.174 | 0.170 |
| concrete_stone_033 | 114 | 0.271 0.293 0.262 | 0.286 |
| concrete_pavement_036 | 60 | 0.332 0.239 0.115 | 0.250 |
| test/concrete_grimey | 54 | 0.034 0.040 0.034 | 0.038 |
| concrete_stone_021 | 48 | 0.441 0.422 0.380 | 0.423 |
| debug_ceiling_grey | 30 | 0.342 0.342 0.342 | 0.342 |
| default_dirt_014 | 29 | 0.121 0.113 0.093 | 0.113 |
| wood_bark_046 | 18 | 0.386 0.280 0.205 | 0.297 |
| default_dirt_011 | 12 | 0.240 0.179 0.147 | 0.190 |
| metal_rough_046a | 6 | 0.108 0.107 0.114 | 0.108 |

The side-weighted luminance is about 0.22, roughly half the 0.45 grey. Matching today's energy would take the indirect scale from 0.33 to about 0.67, inside the slider's [0, 1]. This is a crude estimate (side counts, one map), but it says a retune is likely. Grey 0.45 linear is about sRGB 179/255, lighter than most of these textures. The warm hues (pavement, wood, dirt) are where spill should read. Saturation leaves these luminances unchanged.

Fog samples SH indirect with no indirect scale (`fog_volume.wgsl`, weighted by per-volume `ambient_scatter`), so a retune of `DEFAULT_INDIRECT_SCALE` does not reach it, and its SH-scattered ambient drops by about the same half. Its per-volume `min_brightness` and `ambient_scatter` are the author's controls; this brief does not retune fog.

## Bake-time baseline

Cold bakes on authored maps, Intel Mac, dev profile, `main` 25db4c78f (owner handoff): `campaign-test` total ~21 s, SH Bake 2.6 s, TextureMips 0.1–4 s. `stress-warren-mini` has no recorded baseline at this commit; measure it before the change.

## Interactions

- `drafts/surface-radiance-cache`: "Albedo is whatever the bake uses when this builds". Its patch radiance will take this brief's shared material module; its patch size (delegated there) decides whether it needs footprints or means. It keeps the base/delta parity this brief's one shared estimator holds.
- Roadmap "surface lights": a light entity templated across a marked texture's faces, after this brief. It reads the emissive slot through the shared material module; the glow exclusion keeps its emission from also inflating bounce colour.
- `drafts/surface-radiance-cache/research.md` §Albedo before SH cites TextureMips at ~5.5 s on `campaign-test` (older measurement); the baseline above supersedes it.

## Commitments checked

- `plans/done/lighting-animated-sh/index.md` Q4 (and Task A): grey was chosen because "Texture images are never loaded during the bake". That premise no longer holds.
- `plans/done/incremental-bake-per-element/`: localizes light edits only; "Editing geometry: … all SH groups re-bake". An albedo edit re-keying all SH is in the same class.
- `build_pipeline.md` §Compiler pipeline (Atlas preparation and SH ordering): "These pre-atlas stages consume position-only geometry, so density and scale edits do not invalidate their cache keys". Means are not density or scale, so that conclusion holds, but SH now also reads texture content, and the mip bake's world half moves ahead of it: the stated divergence.
- `build_pipeline.md` §Fluid surfaces and §Baked texture mips (one report, world first): kept (§Shared material stage).
- `build_pipeline.md` §Build Cache Determinism invariant: new code in `sh_bake.rs` preserves byte identity; the means need a fixed summation order.
- `build_pipeline.md` §Build Cache, disposable cache vs runtime-required `baked/materials/`: nothing new persists in either tree (§Derive, don't store).
- `resource_management.md` §4.5: emissive maps are not lights and do not change SH irradiance or neighbouring surfaces. Kept for bounce by masking glow texels out of the mean (§Glow exclusion).
- `context/research/gi-rtx-lite/ip-check.md`: no rule on per-texture mean albedo in the SH bake.

## Byte oracle

- The pre-change bake is digests of ids 34, 27, 35, 41, 45, 47, 48 and the reflection-probe section on the named fixtures, captured on the indirect-scale commit and checked in with the tests.

## Grep gates

- Stats never persisted: any level-compiler file write or cache put taking a stats value.
- One mean computation: callers of the sRGB-to-linear table build outside the shared material module and TextureMips.
- No knob: the strings `bounce`, `albedo` and `saturation` in the worldspawn key list, the FGD and the `prl-build` argument parser.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P-FAIL-ORDER | a texture whose diffuse fails to decode, or whose `_e` dimensions differ from its diffuse | a world or mover texture fails before ShBake; a fluid-only texture fails in the fluid tail, after ShBake and DeltaShBake have run | the build fails in both cases; the stage that reports it differs, observed by a recording stage reporter |
| P-NAME-SPLIT | a texture used by both a world face and a fluid face; a texture used only by a mover | the shared name is interned at Geometry, baked once in the material stage, absent from the fluid tail's names, charged once under its world name; the mover-only name is in the material stage's names, so its sidecar and mean exist before ShBake | no second charge and no duplicate stats record; no static-world face uses the mover-only texture, so SH keys do not fold it |
| P-ZERO-TAIL | map with no fluid-only names | the fluid tail runs on an empty slice | adds no bundle, no log line, no key; the name-to-key map and the single byte report equal the pre-split values; ids 34 and 27 equal the pre-change bake |
| P-ZERO-TEX | map whose name table is empty | the material stage succeeds on an empty slice | the means table is empty, every face is grey, ids 34 and 27 equal the pre-change bake |
| P-ZERO-FACES | map with a name table but no static-world face using any texture | the folded set is empty | SH keys still fold the saturation constant and stage versions; a texture edit hits; a saturation change re-keys |
| P-COMBINED | saturation constant changed and a texture mean changed in one build | both fold into one key | every group and entry re-keys once; reverting only the texture edit still misses; reverting both hits while earlier entries survive; no partial hit |
| P-GREY-MIXED | one map with a resolved red wall and an unresolved face (no texture, an index past the table, a name with no source, or a name with an `_e` but no diffuse) | the unresolved hit takes the grey fallback; the `_e` is not read for a mean when there is no diffuse | each unresolved hit takes exactly 0.45 at every saturation, each resolved hit its mean; unresolved faces bounce as the pre-change bake does |
| P-GATE-ORDER | zero-byte or too-small working-set budget; or invalid light animations; with a world texture that fails to decode | the material stage sits after the gate and validation and before ShBake begins | the gate or validation error is the one reported; no PNG is read and no sidecar is written; the zero-budget projection stays cheap |
| P-SHARED-SIDECAR | two names, same bytes, different emissive strength | the second name in the loop is a sidecar hit on the first's sidecar, in a cold build and in a warm one; order of the two names swaps | strength applies per name at every order; one decode per source; one byte-report charge under the first name |
| P-STALE-SIDECAR | a sidecar present but failing the hit test, absent, or removed after a prior build | the material stage meets it before ShBake | rebake path runs, one decode, sidecar rewritten, mean equal to the intact-sidecar mean, SH keys unchanged |
| P-CACHE-MATRIX | sidecars cold or warm crossed with the stage cache absent (`--no-cache`, `--release`), empty, or populated | the material stage always runs first; the SH stage reads the same table in every cell | ids 34 and 27 are byte-identical across the sidecar axis within each stage-cache mode; no key is built or read in the no-cache cells; the byte report is equal across all cells |
| P-SCALE-RUNTIME | indirect scale changed by the slider after renderer init, with movers and skinned meshes already drawn | the change lands between two frames | the next frame's mover and mesh params, and the world and fluid uniform, hold the new value; fog still ignores it |
| P-ZERO-SH-WORK | textured map with no static lights, or a degenerate SH grid | the material stage precedes an SH stage that does no work | the material stage still runs and its keys reach the merged map; name-to-key, the byte report and sidecars equal the pre-split values; ids 34 and 27 equal the pre-change bake; SH reports skipped, not failed |
| P-ALIAS-NAMES | two distinct interned strings resolving to one source (case, `textures/` prefix, backslash) | both are in the material stage's names, in intern order | one sidecar, one charge under the first name, two stats records with equal means when effective glow strength agrees (owner: existing per-name glow behavior is preserved; different strengths may yield different means), two folded indices |

## Owner-approved adaptive-storage clarification
Section 41 reconstruction magnitude and darkness reference include base indirect, base direct and all delta sections (sh_coarsen.rs). Coloured bounce can change its storage level without changing direct samples. Owner approved preserving exact direct samples before coarsening and retaining the existing adaptive quality/protection/seam contract, rather than requiring identical stored id41 bytes. Keep all other unaffected-section and sidecar byte oracles, compare id41 with the old compiler under the same uniform-storage fixture, and record adaptive bytes and timing changes. This adds no runtime lighting technique or adaptive coarsening for ids27/45.
