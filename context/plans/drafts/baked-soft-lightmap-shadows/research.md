# Research notes — baked soft shadows

Source-grounded facts behind the spec. Line numbers are as of drafting; treat as starting points.

## Bake (crates/level-compiler/src/lightmap_bake.rs)
- Per-texel loop ~680–724: for each texel, world pos via `chart_texel_world_position`, normal = `chart.normal` (one normal per face, no per-texel interpolation). Loops `static_lights`, `irr += contribution` only if `shadow_visible`. Writes `irradiance[idx*4..+4]` (alpha 1.0) + luminance-weighted `direction[idx]`. Arrays: `irradiance` (4 f32/texel), `direction` (Vec3/texel), `coverage` (bool/texel).
- `static_lights` filter ~257–263: `inputs.lights.entries()...filter(|l| l.shadow_type != ShadowType::Sdf)`.
- `shadow_visible(bvh: &Bvh<f32,3>, primitives: &[BvhPrimitive], geometry: &GeometryResult, surface_point: Vec3, surface_normal: Vec3, light: &MapLight) -> bool` ~830–855. Early-returns `true` if `!light.cast_shadows`. `origin = surface_point + surface_normal * RAY_EPSILON`. Target: Point/Spot → `light.origin` (f64→f32); Directional → `surface_point + (-cone_direction).normalize() * DIRECTIONAL_LIGHT_RAY_LENGTH_METERS`. Calls `segment_clear`.
- `segment_clear(bvh, primitives, geometry, origin, target) -> bool` ~857–898 — **the reusable primitive**; builds a `bvh::ray::Ray`, traverses, per-triangle `ray_triangle_hit(origin, dir, a, b, c) -> Option<f32>` (Möller-Trumbore, double-sided). `max_distance = length - RAY_EPSILON`.
- `RAY_EPSILON = 1.0e-3` (~50). `DIRECTIONAL_LIGHT_RAY_LENGTH_METERS = 10000`.
- `light_contribution_and_direction(light, surface_point, surface_normal) -> (Vec3, Vec3)` ~730–789. Point/Spot compute `to_light`, `dist`, `ndotl=max(0,N·L)`, `falloff(light,dist)`; Spot adds `spot_cone(light,-l)`; Directional uses `-cone_direction`, no distance. Backface early-out `ndotl<=0 → (ZERO, l)`.
- `dilate_edges(irradiance, direction, coverage, atlas_w, atlas_h)` ~924; runs `CHART_PADDING_TEXELS` passes.

## Light model (crates/level-compiler/src/map_data.rs)
- `LightType { Point, Spot, Directional }` ~134–142 (sun = Directional).
- `MapLight` ~206–296: `origin: DVec3`, `light_type`, `intensity: f32`, `color: [f32;3]`, `falloff_model`, `falloff_range: f32`, `cone_angle_inner/outer: Option<f32>`, `cone_direction: Option<[f32;3]>`, `animation: Option<LightAnimation>`, `cast_shadows: bool`, `bake_only`, `is_dynamic`, `casts_entity_shadows`, `is_animated`, `tags: Vec<String>`, `shadow_type: ShadowType`. **No size/radius/angular field** — must be added.

## Chart raster (crates/level-compiler/src/chart_raster.rs)
- `CHART_PADDING_TEXELS: u32 = 2` (~20). `ChartPlacement { x, y }`. `chart_interior_dims`, `chart_texel_world_position`.

## Format (crates/level-format/src/lightmap.rs)
- `LightmapSection { width, height, texel_density, irradiance: Vec<u8>, direction: Vec<u8>, mode: LightmapMode }`. `LightmapMode { Shadowed (default), Unshadowed }`. 28-byte header; optional 8-byte "LMOD" trailer only when `Unshadowed`. `IRRADIANCE_TEXEL_BYTES=8` (Rgba16F), `DIRECTION_TEXEL_BYTES=4` (Rgba8 oct). **Section id 22** (`SectionId::Lightmap`, lib.rs). Spec keeps `Shadowed`, no trailer, no layout change.

## Animated path
- Weight-map bake (crates/level-compiler/src/animated_light_weight_maps.rs ~199–235): same `shadow_visible` gate; occluded texels `continue` (no entry). SDF lights skipped ~216.
- `AnimatedLightWeightMapsSection { chunk_rects, offset_counts, texel_lights }`, **section id 25**. `TexelLight { light_index: u32, weight: f32, direction_oct: [u16;2] }` (12 B). Soften = scale `weight` by soft visibility.
- Compose: crates/postretro/src/shaders/animated_lightmap_compose.wgsl writes `animated_lm_atlas` (Rgba16Float storage, binding 6). Accumulates `c * b * entry.weight`. No change needed.

## Runtime (crates/postretro/src/lighting/lightmap.rs, shaders/forward.wgsl)
- Group 4 bindings: `BIND_IRRADIANCE=0`, `BIND_DIRECTION=1`, `BIND_SAMPLER=2`, `BIND_ANIMATED_ATLAS=3`. Sampler = Nearest, `NonFiltering`; all textures `filterable: false`. Irradiance `Rgba16Float`, direction `Rgba8Unorm`. wgpu allows 16 bindings/group → 12 free.
- forward.wgsl: group-4 decls ~183–194; static-direct term ~671–705 (`lm_irr * scale + lm_anim`, bumped-Lambert via `decode_lightmap_direction` ~284–298); isolation modes 0–9, `use_lightmap = iso ∈ {0,2,5}` ~643.
- Pipeline layout groups 0–5 in render/mod.rs ~1474.

## Determinism (no RNG in bake)
- `sh_bake.rs:40` "No RNG — identical input yields byte-identical output." Fibonacci-sphere `sphere_directions(count, seed)` with `SAMPLING_LATTICE_OFFSET = 0x5048_4542_414b_4552` ("PHBAKER"). Mirror this for area-sample patterns.
- `compiler-bake-determinism` (draft): risk is per-process `RandomState` hash iteration feeding output order — avoid `std` HashMap/HashSet ordering in the new code path.

## Conflict map
- `octahedral-irradiance-atlas` (ready) owns SH sections 20/27 only — **no collision** with lightmap section 22.
- `script-driven-light-bake`, `remove-style-key` — adjacent (light animation metadata), no format overlap.
- `compiler-bake-determinism`, `incremental-bake-per-element` — adjacent (bake internals/caching); keep the new sampler deterministic and cache-key-friendly.
