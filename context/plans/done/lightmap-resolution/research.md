# Lightmap Resolution — Research Notes

Investigation grounding for `index.md`. Anchors verified against `main` (wgpu 29.0.1). Line numbers are a snapshot — confirm before editing.

## Why this is worth doing

- **The cap is the routine binding constraint, not an edge case.** Both occlusion-test and campaign-test bake to 4096² at the *default* 0.04 m/texel — they sit exactly at the cap with zero headroom. Any request finer than 0.04 overflows and the retry coarsens. soft_shadow_test bakes 1024² (small maps are fine). So the cap throttles fidelity on every non-trivial level.
- **Resolution is the root fix for blocky contact shadows.** Penumbra is ~1 texel at contact: 4 cm at 0.04, ~2 cm (2 texels) at 0.02, ~1 cm (4 texels) at 0.01 — stops reading blocky around 0.01. Two prior experiments (bake-side subtexel AA; runtime bicubic, parked on `proto/bicubic-lightmap`) only *mask* the faceting; they can't add resolution.
- **The cap is under-justified.** Its comment cites the 8192 wgpu floor as the bound, then sits at half of it. No VRAM, portability, or aesthetic rationale anywhere in code/docs. Lightmap resolution is orthogonal to the retro look (geometry/albedo/palette carry that).

## Anchors — compiler / format

- `MAX_ATLAS_DIMENSION = 4096` (`crates/level-compiler/src/lightmap_bake.rs:62`, `const`, not `pub`). The real binding cap. `MIN_ATLAS_DIMENSION = 64` (`:57`) doc already says "Power-of-two for BC6H block alignment."
- `DEFAULT_TEXEL_DENSITY_METERS = 0.04` (`lightmap_bake.rs:21`, `pub const`).
- `shelf_pack` (`:546-603`): `target_side = ceil(sqrt(total_chart_area))`, `.next_power_of_two()`, `.min(MAX_ATLAS_DIMENSION)`; both width and height end power-of-two and cap-clamped. Grow-and-retry loop; `AtlasOverflow` when height exceeds cap and width is already capped.
- `STAGE_VERSION = 6` (`:54`). Bump-convention comment at `:23-53`; bump-enforcing test ~`:2914-2927`. A format/cap change must bump to 7. **The cap is not in `input_hash`, so without a bump, previously-coarsened maps serve stale cache** (cache key built once in `main.rs:308` from the original requested density).
- Coarsen retry: `main.rs:338-383`, `MAX_RETRIES = 3` (`:341`), `density * 2.0` (`:368`). `--lightmap-density` parse `:846-857`, default `:805`, into `LightmapConfig` `:283-286`.
- Cache key = `blake3(stage_id || STAGE_VERSION || input_hash)`; `input_hash = blake3(postcard(LightmapInputs) ++ postcard(LightmapConfig))`. `LightmapConfig` = `lightmap_density` + `area_sample_count`. Bake has no RNG → byte-deterministic.
- Section emit: `bake_lightmap` (`:297-352`) → `encode_irradiance_rgba16f` (`:1408-1422`, 8 B/texel, 4× f16 LE) + `encode_direction_rgba8` (`:1424-1436`, 4 B/texel octahedral).

## Anchors — level-format (PRL section)

- Section id 22 (`SectionId::Lightmap`). Header 28 bytes LE (`crates/level-format/src/lightmap.rs`, `to_bytes` `:143-170`, `from_bytes` `:172-251`): `width, height, texel_density, irradiance_format, direction_format, irradiance_byte_count, direction_byte_count`, then irradiance blob, direction blob, optional 8-byte `"LMOD"` mode trailer.
- **`IRRADIANCE_FORMAT_RGBA16F = 0` (`:68`)** — doc explicitly: *"the field is versioned so future bakes can introduce compressed variants (BC6H, RGBM) without a new section ID."* This is the designed insertion point. `DIRECTION_FORMAT_OCT_RGBA8 = 0` (`:72`). Both bare `u32` consts; `from_bytes` hard-rejects `!= 0` (`:187-198`) — must widen.
- Strides: `IRRADIANCE_TEXEL_BYTES = 8` (`:10`), `DIRECTION_TEXEL_BYTES = 4` (`:16`). Both blobs raw/uncompressed at rest (confirmed `:163-164`).

## Anchors — runtime / wgpu

- Requested limits (`render/mod.rs:969-975`): only `max_bind_groups`, sampled/storage texture counts, storage-buffer size are raised; **`max_texture_dimension_2d` falls through to the wgpu-29 default 8192**. Add it explicitly here, clamped to `adapter.limits()`.
- Adapter pre-checks bail with named errors (`:980-1026`): `TEXTURE_COMPRESSION_BC` (already required, `:938`, for BC5 normals), texture-count limits, storage-buffer size, and `atlas_format_filterable` (F3, `lighting/lightmap.rs:220-225`, call site `:1019-1026`). Mirror this for the explicit 8192 requirement.
- Device-limit-aware (graceful disable) pattern: `sh_volume.rs:283-300` reads `device.limits().max_texture_dimension_2d`, logs `[Renderer]` + disables on overflow.
- Irradiance texture: `Rgba16Float`, `Extent3d(width,height,1)`, `mip_level_count: 1`, `TEXTURE_BINDING | COPY_DST`, uploaded via `create_texture_with_data` (`lighting/lightmap.rs:227-251`). Direction: `Rgba8Unorm` (`:253-277`). BGL: irradiance `Float{filterable:true}` (binding 0), direction `Float{filterable:false}` (1), nearest sampler (2), animated atlas (3), linear sampler (4). Irradiance + animated sample through the linear sampler; direction through nearest.
- BC5 block-size precedent: `loaded_texture.rs:140` `ceil(w/4)·ceil(h/4)·16`.

## wgpu 29.0.1 BC6H facts (source-confirmed)

- `TextureFormat::Bc6hRgbUfloat` (unsigned HDR — natural for non-negative irradiance) and `Bc6hRgbFloat` (signed). **RGB only, no alpha.** Current irradiance is `Rgba16Float` with alpha written 1.0 (unused) — dropping it is fine.
- `sample_type` → `Float { filterable: true }` with **no extra feature** (unlike 32-bit float, which needs `FLOAT32_FILTERABLE`). Hardware-decoded to float before filtering → the existing linear sampler works unchanged.
- Requires `Features::TEXTURE_COMPRESSION_BC` — already required and granted.
- Block 4×4, 16 B/block = **1 B/texel** vs 8 B/texel for `Rgba16Float` → 8× at-rest reduction (disk and resident VRAM; BC6H stays compressed in VRAM). `Extent3d` dims must be 4-aligned (atlas is already power-of-two).

## Cost math (whole-level resident; PVS frees no VRAM)

| | 4096² | 8192² |
|---|---|---|
| Irradiance Rgba16Float (8 B) | 134 MB | 537 MB |
| Irradiance BC6H (1 B) | 17 MB | 67 MB |
| Direction Rgba8 (4 B) | 67 MB | 268 MB |
| Lightmap VRAM, BC6H irradiance | ~84 MB | ~335 MB |

BC6H makes 8192² irradiance cheaper than today's uncompressed 4096². Disk (.prl) tracks the same ratio — uncompressed 8192² would add ~600 MB/level; BC6H keeps it near today's footprint.

## Durable constraints the plan honors

- `Rgba16Float`/HDR linear-filterability is a hard runtime requirement, fail-fast at init, no software fallback (`rendering_pipeline.md §4`). Direction stays nearest (octahedral lerp ≠ slerp). BC6H keeps irradiance filterable; doesn't touch direction.
- Whole-level VRAM residency, no streaming (`resource_management.md §7.2, §8`). §8 forbids runtime render-to-texture/generation → BC6H encode must be offline/bake, not runtime transcode.
- Renderer owns GPU; subsystems get opaque handles — format choice stays inside `lighting/lightmap.rs`.
- Bake cache keyed by `STAGE_VERSION` + `input_hash`; encode must be byte-deterministic (`build_pipeline.md`).

## Corrections vs the original brief

- The "device-limit-aware cap" framing was imprecise: the bake is a CLI with no GPU device. The cap is a constant (8192, matching guaranteed device support); the `device.limits()` read is a *runtime* safety check (mirror `sh_volume.rs`), not a bake-time one.
- The BC6H comment is at `lightmap_bake.rs:56`, not `lightmap.rs:56`. The actionable BC6H extension point is the versioned `irradiance_format` field in level-format `lightmap.rs:65-68`.
- `TEXTURE_COMPRESSION_BC` is already required and granted (BC5 normals) — no new feature request needed.
- BC6H is RGB-only — the plan drops the unused irradiance alpha.
