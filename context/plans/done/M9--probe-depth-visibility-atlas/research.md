# Research notes — Probe depth/visibility atlas (bake)

Investigation grounding for `index.md`. Not part of the spec. Identifiers confirmed
against source at draft time (branch `claude/m9-milestone-progress-m0g6d`).

## Code-grounding (every identifier the spec names)

### SH volume section (the section to extend)
`crates/level-format/src/sh_volume.rs`:
- `ShProbe { sh_coefficients: [f32; 27], validity: u8 }` — one base probe record.
- `SH_VOLUME_VERSION: u32 = 3` — section-internal version, first u32 of payload. Bumped on layout change; loader rejects mismatched versions.
- `PROBE_STRIDE: u32 = 112` (27 f32 + 1 u8 + 3 pad). Docstring: "forward-compat scaffolding: future per-probe base data (e.g. DDGI distance fields) can grow the stride without breaking the loader." This is the exact growth path for depth moments.
- `ShVolumeSection { grid_origin, cell_size, grid_dimensions, probe_stride, probes, animation_descriptors }`. `HEADER_SIZE = 48`.
- `to_bytes()` / `from_bytes()` — header (48 B) + per-probe records (`probe_stride` B each, z-major/y/x) + animation descriptor table.
- Loader tolerance (line ~299): `o += probe_stride as usize;` after reading the first `PROBE_STRIDE` bytes — "Skip the full on-disk stride, including padding and any future per-probe data beyond the minimum PROBE_STRIDE." So a new-format file with a larger stride is readable by an old runtime, which reads only SH coeffs + validity and skips the rest.
- Reject rule (line ~251): `probe_stride < PROBE_STRIDE` is an error; `probe_stride > PROBE_STRIDE` is accepted.

### Sibling-section precedent (the rejected alternative)
`crates/level-format/src/delta_sh_volumes.rs`:
- `DeltaShVolumesSection`, `DeltaLightGrid { aabb_origin, cell_size: f32, grid_dimensions, probes }`, `DeltaShProbe { sh_coefficients_f16: [u16; 27] }`. `DELTA_SH_VOLUMES_VERSION: u8 = 1`. `f16` storage via `lightmap::f32_to_f16_bits`.
- This is a separate grid per light, so a sibling section made sense there. The depth atlas shares the base SH grid 1:1, so co-location is the better fit.

### SectionId enum
`crates/level-format/src/lib.rs`:
- `#[repr(u32)] enum SectionId`. `ShVolume = 20`, `DeltaShVolumes = 27`, last entry `TextureCacheKeys = 32`. Next free id = 33 (only needed if a sibling section were chosen — it is not).
- `from_u32` match arms must be updated for any new id.

### The bake stage (where depth moments are produced)
`crates/level-compiler/src/sh_bake.rs`:
- `bake_sh_volume(inputs: &ShBakeCtx, config: &ShConfig) -> ShVolumeSection` — top-level baker.
- `STAGE_VERSION: u32 = 1` — "Bump this when the SH baking algorithm changes. Invalidates all existing cache entries for this stage." Adding depth moments to this bake = bump.
- `ShBakeCtx { bvh, primitives, geometry, tree, exterior_leaves, static_lights, animated_lights }`; `RaytracingCtx { bvh, primitives, geometry }` is the shared raycast context.
- `ShConfig { probe_spacing: f32 }` — feeds the cache input hash.
- `ShInputs { static_lights, animated_lights, geometry, exterior_leaves }` — serialized for the cache key.
- `RAYS_PER_PROBE: u32 = 256`. `sphere_directions(count, seed)` — deterministic Fibonacci lattice, `SAMPLING_LATTICE_OFFSET`. No RNG.
- `bake_probe_indirect_rgb(ctx, probe_pos, lights)` — the per-probe loop: for each of 256 directions, `sample_radiance_rgb` then `accumulate_sh_rgb`.
- `closest_hit(ctx, origin, dir, max_distance) -> Option<Hit>` where `Hit { point, normal, distance }`. The hit `distance` is already computed per ray — depth moments are `mean(d)` and `mean(d²)` over the 256 rays, essentially free to accumulate in the same loop.
- `sample_radiance_rgb` already calls `closest_hit(... f32::INFINITY)` per ray. The depth-moment accumulation reuses this same hit; a sky miss (`None`) contributes a far/max distance.
- Determinism guard test: `sh_volume_bake_produces_byte_identical_output_on_repeated_runs` — fans probes via `into_par_iter().map().collect()` (order-preserving). Any new depth accumulation must stay order-stable.

### Bake-time BVH = "the Milestone 4 BVH"
`crates/level-compiler/src/bvh_build.rs`:
- `build_bvh(geo) -> (Bvh<f32,3>, Vec<BvhPrimitive>, BvhSection)` — the `bvh` crate SAH BVH. The live `Bvh` + `primitives` are handed to the SH baker so it traverses on the CPU without rebuilding from the PRL. Traversal: `ctx.bvh.traverse(&ray, ctx.primitives)` then per-triangle `ray_triangle_hit` (double-sided Möller-Trumbore).
- This is the same global SAH BVH whose flattened form (`BvhSection`, level-format `bvh.rs`) the runtime uses for GPU cull. "Ray-cast through the M4 BVH" = traverse this `Bvh<f32,3>` via the existing `RaytracingCtx`.

### Compiler orchestration + cache
`crates/level-compiler/src/main.rs` (~370–426): SH stage cache flow.
- `sh_input_hash = blake3(postcard(ShInputs) || postcard(ShConfig))`.
- `sh_key = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &sh_input_hash)`.
- On hit: `ShVolumeSection::from_bytes(bytes)`. On miss: `bake_sh_volume`, then `c.put(&sh_key, &section.to_bytes())`.
- Because the cache stores the whole `ShVolumeSection.to_bytes()`, depth moments inside the section are cached for free under the same key once `STAGE_VERSION` bumps.
`crates/level-compiler/src/cache.rs`: `CacheKey::new(stage_id, stage_version, input_hash)`, `StageCache { get, put }`.
`crates/level-compiler/src/pack.rs` (~362, ~425): `sh_volume.to_bytes()` → `SectionBlob { section_id: ShVolume, version: 1, data }`. No pack change needed for the in-section approach (the section grows internally).

### Runtime upload (consumer context; NOT in scope to change here)
`crates/postretro/src/render/sh_volume.rs`:
- `ShVolumeResources::new(... section: Option<&ShVolumeSection> ...)`.
- `pack_probes_to_band_slices(&sec.probes, sec.grid_dimensions)` repacks 27-coeff probes into 9 per-band `Rgba16Float` byte buffers; `upload_band_texture` makes 9 base 3D textures + 9 total. Band-0 alpha carries validity (M9 spec #1).
- The depth atlas at runtime (spec #3, separate) would be one or two more parallel 3D textures over the same `grid_dimensions` — exactly "alongside the SH bands."
- `crates/postretro/src/render/mod.rs`: `GeometryResources.sh_volume: Option<&ShVolumeSection>` is the load entry. The depth data rides the same `Option<&ShVolumeSection>` if co-located; no new load plumbing.

## Task #1 measurement gate (informs how much the atlas must buy)
`context/plans/done/M9--probe-weight-correctness/`:
- Spec #1 (shipped) fixed near-wall darkening + invalid-probe exclusion with NO new baked data. Its measurement gate (Task 3) records before/after through-wall bleed + near-wall darkening on `occlusion-test`, in a `measurements/` subfolder, with a CPU frame-time delta.
- At draft time the `measurements/` folder exists but I did not assume specific residual numbers. The spec defers to whatever residual spec #1 recorded: the depth atlas only needs to be baked if residual smear remains after the free fixes. That is a gate condition, not a number to hardcode.

## Decision: extend ShVolume vs. sibling section
**Extend ShVolume** (grow the per-probe record via `probe_stride`, bump `SH_VOLUME_VERSION` and `sh_bake::STAGE_VERSION`). Reasons, all source-grounded:
1. `PROBE_STRIDE` docstring literally names "DDGI distance fields" as the intended stride-growth use. Pre-built scaffolding.
2. Loader already skips `probe_stride - PROBE_STRIDE` trailing bytes per probe → forward/backward read compatibility without a sibling section.
3. SH stage cache serializes the whole section → depth moments cached for free.
4. Depth moments share the base SH grid 1:1 (same `grid_origin`/`cell_size`/`grid_dimensions`). Two sections would each carry a grid header that must stay in lockstep — drift risk. One section, one grid = chunk-friendly: a future brick split partitions one probe array.
5. Sibling sections (delta SH) are justified when the grid differs (per-light AABB grids). Here it does not.

Cost of extending: a `SH_VOLUME_VERSION` bump rejects old `.prl` files (already the contract — the version field exists for exactly this), and the per-probe stride grows. Both are within the format's stated design.
