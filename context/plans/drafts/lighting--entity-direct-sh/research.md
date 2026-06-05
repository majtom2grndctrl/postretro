# Research — Baked Static Direct SH for Entities

Code-grounding for the spec. All citations verified against source at draft time. Ephemeral — superseded by code once shipped.

## The disjoint-lighting baseline (why entities are dim)

- **SH is indirect-only by contract.** `sh_bake.rs:34` — `/// Indirect-only: lightmap carries the direct term; folding direct into SH would double-count it at runtime.` `BOUNCE_ALBEDO = 0.45` (`sh_bake.rs:35`). The delta SH baker restates it: `delta_sh_bake.rs:8-12` — animated direct lives in `lm_anim`, delta carries bounce only, "folding it into the delta too would double-count it."
- **Static surfaces own direct via lightmaps.** `rendering_pipeline.md` §4 "Static direct": prl-build ray-casts per-texel irradiance + dominant direction into a directional lightmap atlas from `static_light_map`-typed lights. Lightmap direct bake: `lightmap_bake.rs:843` `light_contribution_and_direction`, soft visibility `lightmap_bake.rs:1227` `soft_visibility`, `lightmap_bake.rs:1386` `segment_clear`.
- **Entities can't use lightmaps (they move).** The M10 mesh fragment is SH-indirect-only: `skinned_mesh.wgsl` `fs_main` returns `base_color.rgb * indirect` where `indirect = sample_sh_indirect(...)` — "No direct light loop yet — that is the dynamic-direct task's additive group (group 2, unallocated)." Billboards likewise SH-only (`billboard.wgsl:264` `sample_sh_indirect`). Net: entity = SH-indirect only; static surface = lightmap-direct + SH-indirect → entities read dim next to lightmapped geometry.

## The shared atlas (why direct must NOT fold in)

- **One atlas, two consumers.** `sh_total_atlas` is the composed indirect total (base-indirect + animated-indirect-delta). Sampled at **group 3 b1** by `forward.wgsl` (`forward.wgsl:170-184` decls; `sample_sh_indirect` at `forward.wgsl:430-457`, called at `forward.wgsl:693`) AND at **group 4 b1** by `skinned_mesh.wgsl` (`skinned_mesh.wgsl:118-121` decls; `sample_sh_indirect` at `:228`; call at `:263`) AND billboard group 3 b1 (`billboard.wgsl:73`). Same `OctahedralShVolumeSection` data behind both.
- **Consequence:** folding direct into `sh_total_atlas` would leak direct into `forward.wgsl`'s static path, double-counting against the lightmap. The new direct data MUST live in a SEPARATE atlas only the dynamic shaders sample. `forward.wgsl` stays pointed at the indirect-only total — it is not touched.

## The delta compose path (why it's the wrong vehicle)

- **Delta is sparse CSR by affinity cell, animated only.** `delta_sh_volumes.rs` / `delta_sh_bake.rs:13-25`: CSR `affinity_offsets`/`affinity_lights` map each 4×4×4 affinity cell (`AFFINITY_FACTOR = 4`) to overlapping animated lights; one dense 64-probe sub-block per (cell, light). The SH compose compute pass (`sh_compose.wgsl`, `render/sh_compose.rs`) evaluates per-light animation curves each frame and sums the delta into the base atlas. New direct is STATIC (no curve) and DENSE (all probes) → does not belong in the per-frame CSR loop.
- **Compose footprint log tracks CSR only.** `sh_compose.rs:297` `fn log` emits `delta_subblocks`/`affinity_offsets`/`affinity_lights`/`animation_descriptor_indices` bytes — atlas textures are not counted. A dense direct atlas roughly doubles dense SH atlas bytes; that growth is invisible to this log today.

## Reusable bake primitives (assembly, not new infra)

- **SH baker direct + visibility + projection:**
  - `light_contribution_lambert(light, surface_point, surface_normal) -> Vec3` (`sh_bake.rs:555`) — per-light Lambert radiance.
  - `soft_visibility(...)` (`lightmap_bake.rs:1227`, re-exported/used in `sh_bake.rs` via `use crate::lightmap_bake::soft_visibility`, `sh_bake.rs:25`) — stratified soft shadow rays.
  - `segment_clear(ctx, from, to) -> bool` (`sh_bake.rs:517`) — hard occlusion test.
  - `accumulate_sh_rgb(acc: &mut [f32;27], dir, value, weight)` (`sh_bake.rs:666`) — projects an RGB value along `dir` into L2 SH (27 coeffs = 9 bands × RGB).
  - `apply_cosine_lobe_rgb(acc: &mut [f32;27])` (`sh_bake.rs:781`) — Ramamoorthi-Hanrahan zonal cosine-lobe convolution (`COSINE_LOBE_L0=π`, `L1=2π/3`, `L2=π/4`). Comment `sh_bake.rs:774-776`: "After convolution the coefficients reconstruct irradiance directly; the runtime shader needs no per-fragment A_l multiply." This is the SAME convention the indirect bake uses, so the runtime sampler math is unchanged.
  - `sh_basis_l2(dir) -> [f32;9]` (`sh_bake.rs:649`), `evaluate_sh_rgb` (`sh_bake.rs:676`), `pack_octahedral_irradiance_tile(coefficients, valid, tile_dimension, border)` (`sh_bake.rs:728`) — coefficients → octahedral tile, identical to the indirect path.
- **Lightmap baker uses the same primitives** — `soft_visibility` + `light_contribution_and_direction` (`lightmap_bake.rs:843`). Same static-light source, same shadow model → reuse keeps direct-at-probe parity with the lightmapped room.

## Static-light source and order

- `StaticBakedLights::from_lights(lights)` (`light_namespaces.rs:54,59`) — the static-tier filter the SH baker already consumes (`ShBakeCtx.static_lights`, `sh_bake.rs:61`). Direct-at-probe MUST iterate this SAME set/order. Animated direct already lives in `lm_anim` — including animated lights here re-introduces double-counting (the exact failure `delta_sh_bake.rs:8-12` forbids).
- `MapLight` fields confirming static/shadow routing: `shadow_type: ShadowType::StaticLightMap`, `is_animated`, `is_dynamic`, `bake_only`, `cast_shadows`, `light_size`, `angular_diameter` (`sh_bake.rs:1492-1533` test literal; `map_data::MapLight`).

## Probe-point projection convention (the central design decision)

- A probe is a point in air — no surface normal at bake time. The indirect bake handles this by integrating a 256-ray sphere (`RAYS_PER_PROBE=256`, `sh_bake.rs:32`) and projecting incoming radiance per ray direction via `accumulate_sh_rgb`, then `apply_cosine_lobe_rgb`. Direct has no diffuse-bounce sphere integral — it is a small set of point/spot/sun lights.
- Proposed convention (spec §): for each static light reaching the probe, compute incident radiance (`light_contribution_lambert` form, but with the probe as the receiver point) × `soft_visibility` shadow factor, and `accumulate_sh_rgb` it along the light's **incident direction** (probe→light) as a delta/cosine lobe; then `apply_cosine_lobe_rgb` so the stored coeffs are irradiance-convolved exactly like indirect. At runtime the existing cosine-lobe convolution + the receiver fragment's own normal produce the per-fragment response — same sampler, same math. Getting the lobe direction/normalization wrong is what breaks brightness parity (open question for the owner).

## Determinism + cache contract (LOCKED)

- **Byte-identical output required.** `sh_bake.rs:1468-1557` — two cache tests: `sh_volume_bake_produces_byte_identical_output_on_repeated_runs` (`:1476`) and the determinism guard at `:1463-1465`. The build-stage cache keys on input hash and serves stored output bytes verbatim; non-determinism silently breaks it.
- **Determinism rules:** no RNG; soft-visibility jitter is index-derived via `soft_visibility_seed(probe_index, ray_index, light_index)` (`sh_bake.rs:937`, pure-function tests `:1681-1695`) — seeds perturb sampling but "never alter which geometry is traced" (`sh_bake.rs:957`). Fan-out is order-preserving `into_par_iter().map().collect()` — no `par_iter().reduce()` over floats, no HashMap iteration to assemble output (`sh_bake.rs:1470-1475`).
- **Warm group cache.** The driver has two paths (`main.rs:518-531`): warm `sh_group::bake_sh_volume_grouped` (per-probe-group, bounded reaching-light set, approximate) and cold `sh_bake::bake_sh_volume` (`--no-cache`, exact ship source of truth). The new direct bake must slot into the SAME cache scheme, keyed on geometry + static-light set + probe layout.

## Format (new section id, no version bump)

- **Section id registry:** `level-format/src/lib.rs` — `OctahedralShVolume = 34` (`lib.rs:191`, decode arm `:218`), `DeltaShVolumes = 27` (`lib.rs:144`, decode `:211`). Add a NEW id mirroring these.
- **Do NOT bump `SH_VOLUME_VERSION`** (currently `7`, `sh_volume.rs:29`). The loader hard-rejects mismatched versions (`sh_volume.rs:226-230`) → a bump forces a full rebake of all v7 `.prl` content. A new section id is additive — old maps simply lack it.
- **Identical tile geometry.** The direct atlas must use `DEFAULT_IRRADIANCE_TILE_DIMENSION` / `DEFAULT_IRRADIANCE_TILE_BORDER` (`octahedral` module, imported `sh_bake.rs:11-15`) so `sh_sample.wgsl`'s `sample_sh_indirect_corners_depth_aware` math works unchanged.

## Renderer loader + bind

- `ShVolumeResources { pub bind_group, pub bind_group_layout }` (`render/sh_volume.rs:89-91`); `new(device, queue, section: Option<&OctahedralShVolumeSection>, ...)` (`sh_volume.rs:270`); BGL entries `sh_bind_group_layout_entries()` (`:580`), visibility `FRAGMENT | COMPUTE`. Bound group 3 forward/billboard/fog, group 4 mesh.
- Mesh pipeline group map (`rendering_pipeline.md` §9): 0 camera · 1 material · **2 reserved (dynamic-direct)** · 3 instance · 4 SH atlas. New direct atlas needs a slot the dynamic shaders can bind WITHOUT colliding with group 2's reservation — candidate is a sibling binding inside the SH resources group (mesh group 4 / forward+billboard group 3) OR a new group. Decision deferred to spec §; group 2 must stay free for the genuinely-dynamic light loop.

## Debug toolbar plumbing

- `indirect_scale: f32` lives in the forward `Uniforms` (byte offset 92..96, `debug_ui/mod.rs:189` layout comment; `render/mod.rs:381`). Setter `set_indirect_scale` clamps 0..1 (`render/mod.rs:3669`); getter `:3663`. egui slider `debug_ui/mod.rs:170-173` (`Slider::new(.., 0.0..=1.0)` → `renderer.set_indirect_scale`).
- `LightingIsolation` enum (`render/mod.rs:278`): `Normal=0`, `DirectOnly=2`, `IndirectOnly=3`, `StaticSHOnly=6`, etc.; `ALL_VARIANTS: [_;10]` (`:294`); cycle `next()` (`:310`); labels `:326`. Panel dropdown `debug_ui/mod.rs:184-194`. Forward reads `iso` (`forward.wgsl:691`) and forces `indirect_scale=1.0` in IndirectOnly/StaticSHOnly modes.
- **Gap:** the dynamic shaders DON'T read `indirect_scale` — `skinned_mesh.wgsl:41` binds a TRIMMED `CameraUniforms { view_proj }` only, not the full `Uniforms`. Wiring a direct-scale + isolation into the mesh/billboard path means extending what those shaders read (a uniform field or a small dedicated uniform), itself a small task.

## Driver wiring point

- SH bake invoked in `main.rs:518-531`; delta in `:537-550`. Section packed via `pack.rs` (`pack_level` signature includes `sh_volume: &OctahedralShVolumeSection`, `delta_sh_volumes: Option<&...>`, `pack.rs:341-347`; emitted with `section_id: SectionId::OctahedralShVolume as u32` at `pack.rs:436`, delta at `:469`). The new direct section threads through the same driver→pack seam.
