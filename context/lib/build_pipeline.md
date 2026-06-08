# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** maps are authored in TrenchBroom. Engine canonical unit: 1 unit = 1 meter. PRL is the sole runtime map format.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

Maps are authored in TrenchBroom, compiled to PRL with prl-build:

```
TrenchBroom (.map) ──► prl-build (postretro-level-compiler) ──► PRL file (.prl) + .prm sidecars

Engine loads PRL + .prm sidecars at runtime (PNGs for UI only)
```

prl-build builds a BSP tree as a compiler intermediate, generates portal geometry, builds a global BVH over all static triangles, and packs runtime data into a custom binary format. BSP drives spatial partitioning and portal generation at compile time; the runtime consumes cells, portals, and BVH arrays. Engine loads via the `postretro-level-format` crate.

---

## Supported Map Formats

prl-build accepts idTech2 `.map` files (Quake 1/2 dialect, parsed via shambler/shalrath). Unit scale: 1 unit = 0.0254 m (one inch, exact).

Both Standard (axis-aligned) and Valve 220 (explicit UV axes) texture projections are supported. Shalrath auto-detects per face; they can coexist in one `.map` file.

> **Format adapter boundary:** PRL is the engine's internal coordinate standard; Quake convention is not. The `format/` layer in the level compiler is the adapter boundary — each input format translates its own coordinate axes, angle encoding, and units to engine convention before reaching shared compiler logic. Format-specific helpers belong in the format adapter, not shared code.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `content/<mod>/textures/<collection>/<name>.png` (where `<mod>` is `base` for first-party content or `tests` for fixtures). TrenchBroom requires one subdirectory level. |
| TrenchBroom | Browses the textures directory via the Postretro game config. |
| prl-build | Reads PNGs, decodes them, runs Mitchell-Netravali downsampling in linear color space, and writes per-texture `.prm` mip sidecars to `<workspace>/.build-caches/prm-cache/<blake3-hex>.prm`. Stores a content-addressed blake3 key per texture in the `TextureCacheKeys` PRL section. Authored PNGs are not shipped or read at runtime for world materials. |
| PRL output | `TextureNames` section stores a deduplicated texture name list (verbatim from the `.map`, possibly collection-qualified). `TextureCacheKeys` section stores one 32-byte blake3 per name entry. No pixel data. |
| Engine | Loads `.prm` sidecars at level load via the blake3 keys in `TextureCacheKeys`. Never opens a PNG for world materials. UI textures (splash, HUD) still load directly from PNGs. |

### Texture name resolution (compile time)

TrenchBroom identifies materials by their path **relative to the textures root**, so a `.map` may carry a **collection-qualified** name (e.g. `50-free-textures/concrete_pavement_036`) rather than the bare stem. Hand-authored maps may also use bare stems. `prl-build` (`crates/level-compiler/src/texture_mips.rs`) handles both:

- The name→PNG index (`build_name_to_path_map`) keys each PNG under its path **relative to the texture root** — forward-slashed, lowercased, extension stripped (e.g. `50-free-textures/concrete_pavement_036`). It also inserts a **bare-stem alias** (`concrete_pavement_036`) for back-compat, but only when that stem is unique across all collections. On a stem collision the alias is dropped and a `warn!` names both paths, so a bare name never silently resolves to the wrong collection.
- The incoming map name is normalized (lowercase, `\`→`/`, leading `textures/` stripped) so both `collection/stem` and root-inclusive `textures/collection/stem` map to the relative key. Lookup tries the normalized relative name, then falls back to the bare last path segment.
- `_s`/`_n` siblings are derived by appending to **the same form that resolved the diffuse**, so siblings come from the same collection.

A material name with a space (e.g. a collection dir `Level Eleven Games Sci-Fi Texture Pack v1`) is double-quoted in the `.map` by TrenchBroom. shalrath has no quote handling, so the parser (`crates/level-compiler/src/parse.rs`) runs a pre-parse pass that strips the quotes and swaps interior spaces for a path-illegal sentinel byte, keeping the material field one token. The sentinel is decoded back to a real space at the single texture-read boundary, so every downstream stage sees the human-readable name.

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|----------------|
| `light` | point | Omnidirectional light | `light` (intensity), `_color` (RGB), `_falloff_range` (falloff distance, required), `_light_size` (bake-only emitter radius for soft shadows; absent → 0.25, authored 0 → hard shadow), `delay` (falloff model), `style` (animation), `_phase` (style cycle offset), `_bake_only` (bakes but no runtime presence; default 0), `_cast_entity_shadows` (opt-in for dynamic-entity shadow-map pool eligibility; default 0). `_dynamic` retired in Task 1b of `sdf-static-occluder-shadows`; `is_dynamic` is now internal/seam-only — no v1 authoring surface (no light moves yet). |
| `light_spot` | point | Spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `angles` (direction). Shares `_light_size`. |
| `light_sun` | point | Directional sun light | + `angles` (direction vector), `_angular_diameter` (bake-only soft-shadow source angle in degrees; absent → 0.5, authored 0 → hard shadow) |
| `fog_volume` | brush | Per-region fog; geometry behaviour auto-detected — axis-aligned brushes (every face normal ±X/±Y/±Z) become an ellipsoid inscribed in the AABB; non-axis-aligned brushes become a plane-bounded convex hull | `density`, `glow`, `edge_softness` (plane-bounded only), `falloff` (axis-aligned only), `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `fog_lamp` | point | Spherical halo fog emitter; default warm amber | `density`, `glow`, `radius` (sphere radius; sizes AABB), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `fog_tube` | point | Capsule-strip fog emitter; default cool blue-white | `density`, `glow`, `radius` (capsule radius), `height` (capsule length), `pitch` / `yaw` (capsule axis), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `billboard_emitter` | point | Billboard particle emitter | `rate` (particles/sec; default 6), `lifetime` (seconds; default 3), `spread` (cone half-angle radians; default 0.4), `buoyancy` (-1=falls, 0=floats, >0=rises; default 0.2), `drag` (velocity damping/sec; default 0.8), `sprite` (collection name; default "smoke"), `initial_velocity_x/y/z` (default 0/0.8/0), `color_r/g/b` (linear; default 1/1/1), `spin_rate` (radians/sec; default 0) |
| `prop_mesh` | point | Map-placed skinned-model entity | `model` (content-relative glTF path; required — absent or unresolvable logs a warning, load continues) |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |
| `worldspawn` | special | Scene-wide render settings | `script` (path to entry `.ts` script, relative to `.map` file; compiled by `prl-build`), `data_script` (path to data script file; TS compiled to JS via scripts-build, Luau passed through; absent = no data script), `ambient_color` (RGB ambient floor), `fog_pixel_scale` (volumetric pass resolution divisor; default 4, range 1–8), `_lightmap_density` (lightmap bake density, meters per texel; default 0.04; finer = higher resolution; `--lightmap-density` CLI overrides; non-finite/≤0 warns and falls back to default), `initialGravity` (world gravity in m/s²; negative = downward; required; standard Earth = -9.81) |

### Entity resolution

- **`light`, `light_spot`, `light_sun`** — validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Static lights feed the SH irradiance volume baker and the directional lightmap baker. Dynamic lights feed the runtime direct lighting buffer. Compilation fails on validation errors.
- **`fog_volume`** — resolved at load time to world-space AABBs, shape, and fog parameters. Uploaded as a compact storage buffer (up to 16 entries). Per-sample test: shape membership (AABB as conservative bound), then optional half-space clip plane (normal points into the removed region). No BSP traversal at runtime.
- **`billboard_emitter`** — resolved at level load via the built-in classname dispatch table. The engine spawns an ECS entity with a `BillboardEmitterComponent` configured from the map's KVPs. See §Built-in classname routing below.
- **`prop_mesh`** — resolved at level load via the built-in classname dispatch table. The engine spawns a `Transform` + `MeshComponent { model }` entity at `entity.origin`; the renderer loads and uploads the model into its handle→model cache once per distinct path. See §Built-in classname routing below.
- **`env_cubemap`** — marks a position for offline cubemap baking. Bake tool is out of initial scope.
- **`env_reverb_zone`** — resolved to BSP leaves at load time. Each leaf gets spatial reverb parameters for the audio subsystem.

---

## Built-in Classname Routing

The level loader resolves FGD `classname` values against an engine-side handler table (`ClassnameDispatch`) at level load. This table is populated once at engine init by `register_builtins()` and is never cleared on level unload — built-in handlers describe engine types, not per-level state.

For each map entity:
1. Look up `entity.classname` in the built-in handler table.
2. If found: instantiate the configured components, apply the KVP map, spawn the ECS entity at `entity.origin`, copy `_tags`.
3. If not found: `log::debug!` and skip — unregistered classnames are valid in maps that don't use them.

Invalid KVP values log a warning naming the key and entity origin, fall back to the documented default, and load continues.

**Current built-in types:** `billboard_emitter`, `prop_mesh`.

**Two-sweep dispatch.** After the built-in pass, the loader runs a second sweep against script-registered entity types declared on `setupMod()`'s `entities` return field. The built-in pass returns the set of classnames it attempted to handle; the second sweep skips any classname in that set. Built-ins win on collision even when the built-in handler failed to spawn (e.g. registry exhausted) — a classname is owned by exactly one of the two paths for the lifetime of the level. Collisions log a `warn!` once per classname. The second sweep matches placements against each descriptor's `canonicalName`; descriptors with no `canonicalName` are skipped (marker-only archetypes — see `scripting.md §2`). Any placement whose classname is not matched by either sweep and is not in the engine-special exclusion set (`worldspawn`, `player_spawn`) logs a `warn!` once per classname per sweep, naming the placement origin. See `context/lib/scripting.md §2` for the data context lifecycle that populates the descriptor table consumed by the second sweep.

---

## Surface Material Derivation

Texture name prefix maps to a material enum. Drives footstep sounds, bullet impacts, and decals. The engine provides the prefix-to-material lookup mechanism; which prefixes exist is a game content concern. The table grows as content requires it.

Example: `metal_floor_01` → Metal, `concrete_wall_03` → Concrete. See `resource_management.md` §3 for the full mechanism and behavior hooks.

Unknown prefix falls back to a default material with a warning at load time.

---

## PRL Compilation

### Compiler pipeline

```
parse .map → BSP construction → brush-side projection → portal generation → exterior leaf culling → geometry → BVH → lightmap bake → octahedral irradiance volume bake → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction.
2. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward.
3. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded.
4. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Always runs; portals are stored in every PRL for runtime traversal.
5. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
7. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
8. **Lightmap bake.** UV-unwraps world geometry into a lightmap atlas. Ray-casts per-texel irradiance and dominant incoming light direction from all static lights against the global BVH. Static `static_light_map` shadows are baked as **soft area-light visibility** (stratified shadow-ray sampling of each emitter, multiplied into irradiance), not a hard 1-texel gate; an authored `_light_size`/`_angular_diameter` of `0` short-circuits back to a single hard ray. Atlas dimensions are bounded (cap 8192², which requires matching device `max_texture_dimension_2d` support — checked at renderer init); default density is 0.04 m/texel, and a per-map `_lightmap_density` worldspawn KVP opts a map into finer density. On overflow the baker retries at a coarser texel density (halving resolution) a bounded number of times before failing the build. Each retry emits a warning — the fallback is visible in logs, not silent. A second per-light warning fires when an emitter is too small to soften at the atlas density (sub-texel penumbra). Skipped when the map has no static lights.
   - **Soft-shadow bake cost.** Soft visibility multiplies each stage's per-(hit × light) shadow-ray cost by the area-sample count, so penumbra-heavy maps pay a multi-fold bake-time increase over the hard-gate path. Adaptive escalation (a 4-ray probe set, escalating to the full count only in penumbras) keeps fully-lit/fully-shadowed texels cheap and bounds that cost. The lightmap stage is cached, so the increase is paid once per input change; the SH indirect-bounce delta path is cache-less but low-frequency. `--soft-shadow-samples` (default 32) sets the escalated full-sample count: raising it invalidates the cached lightmap stage and re-bakes; the uncached animated weight-map stage recomputes from scratch every build. Adaptive-escalation thresholds stay fixed constants regardless, so the bake stays deterministic.
9. **Octahedral irradiance volume bake.** Bakes static-light indirect irradiance into octahedral atlas tiles and isotropic Chebyshev depth moments. When animated lights are present, also bakes the sparse indirect-only delta tile companion for runtime composition.
10. **Pack.** Writes all sections to the `.prl` binary format.

### PRL section IDs

PRL header `version` is 4. Loading a file with any other version fails.

| Section | ID | When present |
|---------|-----|-------------|
| BspNodes | 12 | Always |
| BspLeaves | 13 | Always |
| Portals | 15 | Always |
| TextureNames | 16 | Always |
| Geometry | 17 | Always |
| AlphaLights | 18 | Always |
| Bvh | 19 | Always |
| ShVolume | 20 | Retired legacy L2 SH irradiance payload; stale files are rejected by section-internal version |
| LightInfluence | 21 | When compiled with lighting |
| Lightmap | 22 | Always (placeholder atlas when a map has no static lights) |
| ChunkLightList | 23 | Always; per-chunk static-light index lists for specular culling |
| AnimatedLightChunks | 24 | When compiled with animated lights |
| AnimatedLightWeightMaps | 25 | When compiled with animated lights; per-texel weight maps for the compose pass |
| LightTags | 26 | When at least one light carries a tag; one space-delimited tag-list string per AlphaLight record (empty string = untagged) |
| DeltaShVolumes | 27 | When the map has at least one animated light; per-light sparse octahedral irradiance delta tiles |
| DataScript | 28 | When `data_script` KVP present on `worldspawn`; compiled script bytes + original source path |
| MapEntity | 29 | When the map has at least one non-light, non-worldspawn entity; per-entity classname, origin, angles, tags, and KVP bag for runtime classname dispatch |
| FogVolumes | 30 | Always (12-byte overhead when no fog_volume brushes present; carries fog_pixel_scale and initial_gravity) |
| FogCellMasks | 31 | When at least one fog volume entity is present (fog_volume brush, fog_lamp, or fog_tube) |
| TextureCacheKeys | 32 | Always; one 32-byte blake3 per TextureNames entry pointing at a `.prm` sidecar under `.build-caches/prm-cache/` |
| SdfAtlas | 33 | When the map has SDF static occluder data |
| OctahedralShVolume | 34 | When compiled with lighting; base indirect irradiance as octahedral atlas tiles |
| DirectShVolume | 35 | When the map has static baked lights; dense baked static-direct octahedral irradiance for entities/billboards; BC6H at rest; no depth moments (read from id 34); same tile geometry as OctahedralShVolume; section-internal `DIRECT_SH_VOLUME_VERSION` (no `SH_VOLUME_VERSION` bump — legacy v7 maps still load) |

**Lightmap (id 22):** 28-byte little-endian header (width, height, `texel_density`, `irradiance_format`, `direction_format`, `irr_len`, `dir_len`) plus the irradiance and direction blobs. `irradiance_format` is `0 = Rgba16Float` (uncompressed, `width·height·8` bytes; debug-only) or `1 = Bc6hRgbUfloat` (BC6H block-compressed at rest, `ceil(width/4)·ceil(height/4)·16` bytes; the default — ~8× smaller on disk and in VRAM, hardware-decoded and hardware-filterable at runtime). `from_bytes` reads each blob by the stored `irr_len`/`dir_len`, so the format value alone selects the block math. The direction atlas stays `Rgba8Unorm` octahedral (`direction_format = 0`) on the nearest sampler — octahedral lerp ≠ slerp, so it is never compressed or linearly filtered. BC6H output is lossy, so the lightmap stage is exempt from the byte-identical determinism invariant (correctness is round-trip within tolerance; the cache keys on inputs regardless).

**OctahedralShVolume (id 34):** sibling replacement for legacy `ShVolume` (id 20). The section stores grid origin/cell size/dimensions, one metadata record per probe (`validity`, f16 `E[d]`, f16 `E[d²]`), and one 2D `Rgba16Float` atlas of base irradiance tiles. Default tile geometry is `tile_dimension = 6` including `tile_border = 1`, giving a 4×4 interior. Probe records keep x-fastest linear order: `probe_index = x + y × grid_x + z × grid_x × grid_y`. Tiles are packed into a deterministic near-square 2D atlas: `atlas_tiles_per_row = ceil(sqrt(total_probe_count))`, `tile_x = probe_index % atlas_tiles_per_row`, `tile_y = probe_index / atlas_tiles_per_row`, so atlas dimensions are `(atlas_tiles_per_row × tile_dimension, ceil(total_probe_count / atlas_tiles_per_row) × tile_dimension)`. Interior texel centers decode through the Rust octahedral mapping in `crates/level-format/src/octahedral.rs`; the 1-texel border copies the opposite edge with the orthogonal coordinate reversed across the octahedral wrap. Sky-miss sentinel distance remains `4 × length(cell_size)` for the isotropic Chebyshev moments. `SH_VOLUME_VERSION` is a section-internal version (not the PRL header version); version 7 is the near-square atlas packing header, and stale pre-migration `.prl` files are rejected rather than silently accepted.

**DirectShVolume (id 35):** baked static-direct octahedral irradiance for entities and billboards. Same tile geometry and probe ordering as `OctahedralShVolume` (id 34); carries no depth moments (read from id 34). Stored BC6H at rest. Emitted only when the map has static baked lights. Section-internal `DIRECT_SH_VOLUME_VERSION`; does not bump `SH_VOLUME_VERSION`, so legacy v7 maps continue to load. Runtime: sampled by skinned-mesh and billboard shaders, gated by `has_direct`; forward and fog pipelines bind but do not sample it.

**DeltaShVolumes (id 27):** sparse CSR companion for animated-light indirect deltas. The affinity-cell structure is unchanged: `affinity_factor = 4`, `affinity_dims = ceil(base_dims / 4)`, CSR `affinity_offsets`, flat `affinity_lights`, and one dense 64-probe sub-block per CSR entry in x-fastest in-cell order. Version 3 replaces each probe's old 28-half SH coefficient payload with one row-major `Rgba16Float` octahedral delta tile using the same default `tile_dimension = 6`, `tile_border = 1`, interior mapping, and wrap-border convention as `OctahedralShVolume`. The delta bake is indirect-only; animated direct lighting lives in `lm_anim`, so adding direct terms here would double-count. `DELTA_SH_VOLUMES_VERSION` is section-internal and stale pre-migration sections are rejected. Delta bakes are invoked directly from the compiler rather than through the build cache, so this migration has no cache key or stage version to bump.

### Runtime visibility

Portal traversal is the sole visibility path: per-frame flood-fill from the camera leaf with frustum narrowing at each portal. The runtime falls back to per-leaf AABB frustum culling for solid-leaf, exterior-camera, and no-portals cases. See `rendering_pipeline.md` §2.

---

## Build Cache

Disk-backed content-hash cache that lets `prl-build` skip the two expensive bake stages when their inputs are unchanged.

**Location.** `.build-caches/prl-cache/` at the workspace root (the parent directory containing `Cargo.toml`). Created automatically on first build. Safe to delete at any time — the next build recreates it. The cache root `.build-caches/` also contains `prm-cache/` (texture mip sidecars; see §Baked texture mips).

**Participating stages.** Lightmap bake and SH volume bake, plus the animated-light weight-map and SDF-atlas stages. Parse, BSP, portals, geometry, and BVH run uncached — they are fast enough that caching yields no measurable speedup.

**Cache grain (lightmap + SH).** These two channels are cached *per element*, not per whole stage, so editing one light refreshes only the affected entries:

- **Lightmap — per-light layers.** Each static light's contribution (linear irradiance + unnormalized weighted direction + coverage, full-precision) is a separate `"lightmap_layer"` entry, keyed on that light's params, its influence-bounded geometry slice, density/sample-count, and the atlas layout. The compositor sums the layers (in global light order) and normalizes once, reproducing the monolithic `bake_face_chart` byte-for-byte (pre-BC6H). Exact in both warm and cold builds.
- **Lightmap — composited section (second level).** A `"lightmap_section"` entry memoizes the encoded `Lightmap` (id 22) section itself, keyed on the ordered per-light layer fingerprints plus the encode parameters (texel density, irradiance format). A no-edit rebuild hits it and skips reading the layers, compositing, and BC6H-encoding entirely — the per-light layers are the recompose fallback when any light, geometry, or atlas input changes. Pure memoization of an already-exact pipeline output, so it cannot perturb byte-identity; warm-only, like the layers.
- **SH — per-probe-group entries.** The probe grid is partitioned into 4³-probe groups; each is a `"sh_group"` entry baked over its probe subset with a *bounded reaching-light set* (`falloff_range` dilated by a finite reach cutoff), then assembled (byte-copy placement) into the volume. Bounding the light set is what localizes a light edit; it also makes warm SH a benign approximation (out-of-reach lights drop — dimmer-or-equal, never miscolored). The soft-visibility sample-lattice seed mixes each light's **global** `static_lights` index (not its position in the bounded slice), so a kept light gets the same rotation whether the bake sees the full set (cold) or the bounded set (warm) — that is what makes "dimmer-or-equal, never brighter" hold strictly. The cold `--no-cache` path runs the exact whole-volume bake instead. SH rays trace full geometry, so any geometry edit re-bakes every group.

**Warm vs cold builds (dev-default / release-on-purpose).** The interactive default is a *warm* (cached) build: fast iteration, exact direct lightmap, approximate indirect SH. The `--release` flag selects the *cold* exact build — every stage baked exact — and is the only artifact a final map should ship from. `--release` is the intent-named ship mode; mechanically it bypasses the cache exactly like `--no-cache` (it implies `--no-cache`; passing both is fine and identical). A warm build trades exactness for speed and is not shippable. The split is per channel. The direct lightmap is exact in both modes: a cached lightmap is byte-identical to a full bake (pre-compression). Indirect SH is exact only in a release/cold build. A warm build bakes SH at a finer-than-whole-volume grain, bounding each region's light set — a benign approximation, dimmer-or-equal in far-bounce regions, never miscolored. A warm build emits a one-line warning naming `--release` as the ship flag. Judge final indirect lighting on a release build. Run production and release bakes with `--release` (or `--no-cache`).

**Key composition.** `blake3(stage_id || stage_version_le_bytes || input_hash)`.

| Component | Form |
|-----------|------|
| `stage_id` | string literal — `"lightmap_layer"` (per-light), `"lightmap_section"` (composited-section memo), `"sh_group"` (per-probe-group), `"animated_lm_weight_maps"`, or `"sdf_atlas"` |
| `stage_version` | `u32` constant in each stage's module, bumped manually when that stage's algorithm or payload format changes. Each stage owns its own constant and version-bumps independently — the per-light-layer and per-group-SH formats version separately from each other and from the legacy whole-stage bakes |
| `input_hash` | `blake3(postcard(StageInputs) || postcard(StageConfig))` — covers the serialized data the stage reads |

**Stage version bump rule.** Bump a stage's `STAGE_VERSION` when its output computation changes (algorithm, sampling, formula, or atlas packing). The substrate invalidates every entry for that stage on the next build. Do not bump for unrelated changes. Each stage's current value lives as a `u32` constant in its own module — the source is authoritative; this doc does not pin the number.

**Determinism invariant.** Byte-identical output for identical inputs — with two scoped carve-outs. The guarantee holds for the direct lightmap before compression and for the cold whole-volume SH bake (the ship path). New code in `lightmap_bake.rs` or `sh_bake.rs` must preserve it. Avoid common non-determinism sources: `HashMap` iteration feeding output ordering, non-order-preserving parallel reductions. **Exempt:** (1) lossy compressed output (BC6H irradiance) — correctness is round-trip within tolerance, not byte-equality; (2) indirect SH baked finer than the whole volume (warm incremental builds) — a deliberate bounded approximation; the cold whole-volume bake stays exact. Either way the cache stays correct: it keys on inputs, not outputs. Every bake is self-consistent — same inputs, same bytes.

**CLI flags.**

| Flag | Effect |
|------|--------|
| `--cache-dir <PATH>` | Use a custom cache directory instead of `.build-caches/prl-cache/` at the workspace root |
| `--cache-max-size <SIZE>` | LRU size budget for the cache, swept at build start (default 2 GiB). Accepts a byte count or a binary-unit suffix (`2GiB`, `512MiB`, `1.5GiB`) |
| `--no-cache` | Disable the cache entirely — neither read nor write, no directory created (no prune either) |
| `--release` | Produce a shippable map: the exact ship path (exact monolithic lightmap + exact whole-volume SH). Intent-named ship mode; implies `--no-cache` (passing both is fine and identical). The interactive default is a warm build — ship only `--release` artifacts. |
| `--soft-shadow-samples <N>` | Soft-shadow penumbra escalated full-sample count (default 32). Folds into the lightmap stage's cache key (raising it invalidates the cache and re-bakes); the uncached animated weight-map stage recomputes from scratch. Run `prl-build --help` for the full flag list. |

**Entry format.** One file per entry, named by the hex key. `get()` validates integrity before returning payload; mismatch is a soft failure (warning, cache miss).

**Eviction.** LRU size cap, enforced by a sweep at the start of every cached build (before the bake writes a fresh generation). When the directory exceeds the budget (`--cache-max-size`, default 2 GiB), the least-recently-used entries are deleted oldest-first until the total fits. Recency is the entry's mtime: `get` bumps it on every hit and `put` sets it on write, so a long-stable entry (hit every build, never rewritten) stays warm while orphaned generations — the tail content addressing leaves behind whenever an input changes — age out and get reclaimed. The sweep is off the bake path (one directory listing plus a few unlinks) and best-effort: any I/O error is logged and the build proceeds. In-flight `*.tmp` stage files are never touched. `--no-cache`/`--release` skip the cache (and the sweep) entirely. A corrupted entry is still discarded as a cache miss without touching other entries. The cache remains safe to delete manually at any time.

---

## Baked texture mips

Per-texture mip-chain sidecars live alongside the stage-output cache. prl-build writes them; the engine reads them at level load.

**`.prm` files.** Each sidecar bundles up to three material slots — diffuse, specular, and normal — each optional. Content-addressed by `blake3(diffuse PNG content)` when a diffuse slot is present; otherwise `blake3(tag_byte || first_present_PNG)`. The `tag_byte` prevents hash collisions between specular-only and normal-only single-slot textures. Stored at `<workspace>/.build-caches/prm-cache/<hex>.prm`. Cross-mod dedupe is intended: identical PNG bytes produce the same `.prm` regardless of which mod authored them.

**Wire format.** Header + per-slot blocks + packed mip payload. Wire layout lives in `postretro-level-format::prm`. Note: `.prm` uses a `u8` `STAGE_VERSION` (not the stage-cache `u32` convention) — the header owns its own version semantics.

**Filtering.** Mitchell-Netravali separable filter (B = C = 1/3) in linear space throughout. sRGB diffuse decoded via 256-entry LUT before filtering, re-encoded via IEC 61966-2-1. Specular filtered as linear R8. Normal filtered linearly then renormalised per output texel; `(0, 0, 1)` substituted when magnitude < 1e-4. Output is then BC5-encoded (RG channels only; the shader reconstructs Z).

**Cache invalidation.** Filename keys on diffuse content only (stable addressing). `bundle_hash` in the header covers `slot_mask` + every present slot's raw PNG bytes. A world-material cache hit requires a matching bundle hash and structurally valid payload for every declared slot; truncated or corrupt declared slots trigger a full rebake and atomic overwrite (tempfile `<hex>.prm.tmp.<pid>` → `std::fs::rename`). Model baking preserves a structurally valid richer world bundle at the shared diffuse address even though its bundle hash includes sibling slots. A `stage_version` mismatch in the header triggers rebake. To force a full retexture rebuild, delete `.build-caches/prm-cache/`.

**Runtime.** Level load resolves each `TextureNamesSection` entry's blake3 key from `TextureCacheKeysSection`, opens the corresponding `.prm`, and uploads each slot's mip chain directly. A zero key (`[0u8; 32]`) substitutes per-slot placeholders silently. A corrupt or missing `.prm` substitutes per-slot placeholders and logs a `warn!`; load continues. Sampler `lod_max_clamp` is set to `mip_count - 1` per texture.

**Model textures.** `prop_mesh` model base-color textures bake the same way, content-driven from the model placements in the map — no CLI flag, mirroring how world materials follow from `TextureNames`. prl-build resolves each placed model's glTF base-color PNG(s) and bakes a diffuse-only `.prm`, content-addressed by `blake3(base-color PNG)` — byte-identical to a diffuse-only world sidecar. A richer world bundle with the same diffuse bytes owns the shared filename and is never replaced by the model bake. Model rendering still consumes only the diffuse slot and substitutes neutral specular and normal placeholders. Unlike world materials, no PRL section carries model keys: the runtime content-hashes the same PNG when it loads the glTF and opens `<key>.prm` directly, so the compiler only has to make the sidecar exist. The glTF base-color path resolver is shared by runtime and compiler through the `gltf-resolve` feature of `postretro-level-format`. Missing or malformed glTF fails the whole model load, so the model is skipped. Only an unresolved, missing, or unreadable base-color PNG or material degrades to the texture placeholder. Compiler resolution and bake failures warn; compilation continues. This removes the prior coupling where a model's runtime texture depended on a hand-staged, gitignored cache file that nothing regenerated.

---

## Non-Goals

- Runtime level compilation
- WAD file support
- Runtime lightmap baking
