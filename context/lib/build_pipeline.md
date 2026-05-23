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
| PRL output | `TextureNames` section stores a deduplicated texture name list. `TextureCacheKeys` section stores one 32-byte blake3 per name entry. No pixel data. |
| Engine | Loads `.prm` sidecars at level load via the blake3 keys in `TextureCacheKeys`. Never opens a PNG for world materials. UI textures (splash, HUD) still load directly from PNGs. |

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|----------------|
| `light` | point | Omnidirectional light | `light` (intensity), `_color` (RGB), `_fade` (falloff distance, required), `delay` (falloff model), `style` (animation), `_phase` (style cycle offset), `_dynamic` (static-baked vs. runtime dynamic; default 0 = static) |
| `light_spot` | point | Spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `angles` (direction) |
| `light_sun` | point | Directional sun light | + `angles` (direction vector) |
| `fog_volume` | brush | Per-region fog; geometry behaviour auto-detected — axis-aligned brushes (every face normal ±X/±Y/±Z) become an ellipsoid inscribed in the AABB; non-axis-aligned brushes become a plane-bounded convex hull | `density`, `edge_softness` (plane-bounded only; world-unit fade band inward from brush faces; 0 = hard cutoff), `scatter` (scatter fraction toward camera; default 0.6), `falloff` (axis-aligned only; radial falloff exponent; default 2.0), `_tags`, `clip` (`"none"` default / `"plane"`), `clip_pitch` / `clip_yaw` (half-space normal into removed region), `clip_offset` (center-relative offset; 0 = cut through center) (ambient color is SH-derived; no `color` KVP) |
| `fog_lamp` | point | Spherical halo fog emitter; default warm amber | `density`, `radius` (sphere radius; sizes AABB), `radial_falloff`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `fog_tube` | point | Capsule-strip fog emitter; default cool blue-white | `density`, `radius` (capsule radius), `height` (capsule length), `pitch` / `yaw` (capsule axis), `radial_falloff`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `billboard_emitter` | point | Billboard particle emitter | `rate` (particles/sec; default 6), `lifetime` (seconds; default 3), `spread` (cone half-angle radians; default 0.4), `buoyancy` (-1=falls, 0=floats, >0=rises; default 0.2), `drag` (velocity damping/sec; default 0.8), `sprite` (collection name; default "smoke"), `initial_velocity_x/y/z` (default 0/0.8/0), `color_r/g/b` (linear; default 1/1/1), `spin_rate` (radians/sec; default 0) |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |
| `worldspawn` | special | Scene-wide render settings | `script` (path to entry `.ts` script, relative to `.map` file; compiled by `prl-build`), `data_script` (path to data script file; TS compiled to JS via scripts-build, Luau passed through; absent = no data script), `ambient_color` (RGB ambient floor), `fog_pixel_scale` (volumetric pass resolution divisor; default 4, range 1–8), `initialGravity` (world gravity in m/s²; negative = downward; required; standard Earth = -9.81) |

### Entity resolution

- **`light`, `light_spot`, `light_sun`** — validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Static lights feed the SH irradiance volume baker and the directional lightmap baker. Dynamic lights feed the runtime direct lighting buffer. Compilation fails on validation errors.
- **`fog_volume`** — resolved at load time to world-space AABBs, shape, and fog parameters. Uploaded as a compact storage buffer (up to 16 entries). Per-sample test: shape membership (AABB as conservative bound), then optional half-space clip plane (normal points into the removed region). No BSP traversal at runtime.
- **`billboard_emitter`** — resolved at level load via the built-in classname dispatch table. The engine spawns an ECS entity with a `BillboardEmitterComponent` configured from the map's KVPs. See §Built-in classname routing below.
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

**Current built-in types:** `billboard_emitter`.

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
parse .map → BSP construction → brush-side projection → portal generation → exterior leaf culling → geometry → BVH → lightmap bake → SH volume bake → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction.
2. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward.
3. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded.
4. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Always runs; portals are stored in every PRL for runtime traversal.
5. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
6. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
7. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
8. **Lightmap bake.** UV-unwraps world geometry into a lightmap atlas. Ray-casts per-texel irradiance and dominant incoming light direction from all static lights against the global BVH. Atlas dimensions are bounded; on overflow the baker retries at a coarser texel density (halving resolution) a bounded number of times before failing the build. Each retry emits a warning — the fallback is visible in logs, not silent. Skipped when the map has no static lights.
9. **Pack.** Writes all sections to the `.prl` binary format.

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
| ShVolume | 20 | When compiled with lighting |
| LightInfluence | 21 | When compiled with lighting |
| Lightmap | 22 | Always (placeholder atlas when a map has no static lights) |
| ChunkLightList | 23 | Always; per-chunk static-light index lists for specular culling |
| AnimatedLightChunks | 24 | When compiled with animated lights |
| AnimatedLightWeightMaps | 25 | When compiled with animated lights; per-texel weight maps for the compose pass |
| LightTags | 26 | When at least one light carries a tag; one space-delimited tag-list string per AlphaLight record (empty string = untagged) |
| DeltaShVolumes | 27 | When the map has at least one animated light; per-light delta SH probe grids |
| DataScript | 28 | When `data_script` KVP present on `worldspawn`; compiled script bytes + original source path |
| MapEntity | 29 | When the map has at least one non-light, non-worldspawn entity; per-entity classname, origin, angles, tags, and KVP bag for runtime classname dispatch |
| FogVolumes | 30 | Always (12-byte overhead when no fog_volume brushes present; carries fog_pixel_scale and initial_gravity) |
| FogCellMasks | 31 | When at least one fog volume entity is present (fog_volume brush, fog_lamp, or fog_tube) |
| TextureCacheKeys | 32 | Always; one 32-byte blake3 per TextureNames entry pointing at a `.prm` sidecar under `.build-caches/prm-cache/` |

**ShVolume (id 20) probe record:** byte layout, field offsets, PROBE_STRIDE, and f16 encoding are documented in the `ShProbe` and `PROBE_STRIDE` doc comments in `crates/level-format/src/sh_volume.rs`. Non-obvious contract: sky-miss sentinel distance = `4 × length(cell_size)` — four times the full 3D cell diagonal, chosen so any valid in-cell ray distance is always smaller. `SH_VOLUME_VERSION` is a section-internal version (not the PRL header version); bump it whenever the per-probe record layout changes, so the loader rejects stale `.prl` files with a clear error.

### Runtime visibility

Portal traversal is the sole visibility path: per-frame flood-fill from the camera leaf with frustum narrowing at each portal. The runtime falls back to per-leaf AABB frustum culling for solid-leaf, exterior-camera, and no-portals cases. See `rendering_pipeline.md` §2.

---

## Build Cache

Disk-backed content-hash cache that lets `prl-build` skip the two expensive bake stages when their inputs are unchanged.

**Location.** `.build-caches/prl-cache/` at the workspace root (the parent directory containing `Cargo.toml`). Created automatically on first build. Safe to delete at any time — the next build recreates it. The cache root `.build-caches/` also contains `prm-cache/` (texture mip sidecars; see §Baked texture mips).

**Participating stages.** Lightmap bake and SH volume bake. Parse, BSP, portals, geometry, and BVH run uncached — they are fast enough that caching yields no measurable speedup.

**Key composition.** `blake3(stage_id || stage_version_le_bytes || input_hash)`.

| Component | Form |
|-----------|------|
| `stage_id` | string literal — `"lightmap"` or `"sh_volume"` |
| `stage_version` | `u32` constant (`STAGE_VERSION`) in each stage's module; bumped manually when the baking algorithm changes |
| `input_hash` | `blake3(postcard(StageInputs) || postcard(StageConfig))` — covers the serialized data the stage reads |

**Stage version bump rule.** Bump a stage's `STAGE_VERSION` when its output computation changes (algorithm, sampling, formula). The substrate invalidates every entry for that stage on the next build. Do not bump for unrelated changes.

**Determinism invariant.** Both cached stages produce byte-identical output for identical inputs. Any new code in `lightmap_bake.rs` or `sh_bake.rs` must preserve this. Common non-determinism sources to avoid: `HashMap` iteration feeding output ordering, non-order-preserving parallel reductions.

**CLI flags.**

| Flag | Effect |
|------|--------|
| `--cache-dir <PATH>` | Use a custom cache directory instead of `.build-caches/prl-cache/` at the workspace root |
| `--no-cache` | Disable the cache entirely — neither read nor write, no directory created |

**Entry format.** One file per entry, named by the hex key. `get()` validates integrity before returning payload; mismatch is a soft failure (warning, cache miss).

**Eviction.** No policy-driven eviction. Delete `.build-caches/prl-cache/` manually when it grows too large. A corrupted entry is discarded as a cache miss without touching other entries.

---

## Baked texture mips

Per-texture mip-chain sidecars live alongside the stage-output cache. prl-build writes them; the engine reads them at level load.

**`.prm` files.** Each sidecar bundles up to three material slots — diffuse, specular, and normal — each optional. Content-addressed by `blake3(diffuse PNG content)` when a diffuse slot is present; otherwise `blake3(tag_byte || first_present_PNG)`. The `tag_byte` prevents hash collisions between specular-only and normal-only single-slot textures. Stored at `<workspace>/.build-caches/prm-cache/<hex>.prm`. Cross-mod dedupe is intended: identical PNG bytes produce the same `.prm` regardless of which mod authored them.

**Wire format.** Header + per-slot blocks + packed mip payload. Wire layout lives in `postretro-level-format::prm`. Note: `.prm` uses a `u8` `STAGE_VERSION` (not the stage-cache `u32` convention) — the header owns its own version semantics.

**Filtering.** Mitchell-Netravali separable filter (B = C = 1/3) in linear space throughout. sRGB diffuse decoded via 256-entry LUT before filtering, re-encoded via IEC 61966-2-1. Specular filtered as linear R8. Normal filtered linearly then renormalised per output texel; `(0, 0, 1)` substituted when magnitude < 1e-4. Output is then BC5-encoded (RG channels only; the shader reconstructs Z).

**Cache invalidation.** Filename keys on diffuse content only (stable addressing). `bundle_hash` in the header covers `slot_mask` + every present slot's raw PNG bytes. On rebuild, prl-build re-hashes source PNGs and compares against the stored `bundle_hash`; mismatch triggers a full rebake and atomic overwrite (tempfile `<hex>.prm.tmp.<pid>` → `std::fs::rename`). A `stage_version` mismatch in the header also triggers rebake. To force a full retexture rebuild, delete `.build-caches/prm-cache/`.

**Runtime.** Level load resolves each `TextureNamesSection` entry's blake3 key from `TextureCacheKeysSection`, opens the corresponding `.prm`, and uploads each slot's mip chain directly. A zero key (`[0u8; 32]`) substitutes per-slot placeholders silently. A corrupt or missing `.prm` substitutes per-slot placeholders and logs a `warn!`; load continues. Sampler `lod_max_clamp` is set to `mip_count - 1` per texture.

---

## Non-Goals

- Runtime level compilation
- WAD file support
- Runtime lightmap baking
