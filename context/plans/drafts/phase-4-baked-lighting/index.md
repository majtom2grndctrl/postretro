# Phase 4 — Lighting Foundation

> **Status:** draft — architectural direction locked. Stages 1–3 (FGD, translator, canonical format) are implementation-ready. Stages 4–5 (SH irradiance volume baker and output format) are decided in this document; remaining refinement is sizing choices (probe density, ray count) that belong in the execution plan, not the format spec.
> **Phase:** 4 (Lighting Foundation) — see `context/plans/roadmap.md`. Covers the full lighting pipeline end-to-end: baked indirect (SH irradiance volume) and dynamic direct (clustered forward+ with shadow maps). One spec, one pipeline.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `context/reference/light-entities-across-engines.md`
> **Prerequisite:** Phase 3.5 (Rendering Foundation Extension) — vertex format with packed normals + tangents, per-cell draw chunks, and GPU-driven indirect draws must ship first. The lighting path layers onto that architecture.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the format spec. Research happens in-draft; decisions land in the spec as they're made; implementation consumes the plan; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Specify Postretro's lighting system end-to-end: how mappers author lights in TrenchBroom, how the compiler translates them into a canonical format, how the baker turns canonical lights into an SH irradiance volume, how the runtime samples indirect light, and how the same canonical lights drive dynamic direct shading. One spec, one pipeline, one place where format decisions live.

Four axes, one plan:

1. **Input** — mapper authoring (FGD) → parse → translator → canonical light format.
2. **Indirect bake** — SH L2 irradiance volume baked from canonical lights over static geometry.
3. **Direct shading** — clustered forward+ consumption of canonical lights plus transient gameplay lights, with shadow maps for dynamic shadow-casters.
4. **Output** — PRL section shape for the SH volume; runtime sampling path in the world shader.

The translator is decoupled from the parser: future map-format support (e.g., UDMF) adds a sibling module against the same canonical types, so the baker and everything downstream never learn the source format.

---

## Context

Phase 3 ships flat uniform lighting with no baked light sources. Phase 3.5 adds GPU-driven indirect draws, per-cell chunks, HiZ, and the vertex format (position + UV + packed normal + packed tangent) that lighting depends on — but continues to apply flat ambient. Phase 4 replaces flat ambient with the full lighting pipeline: SH L2 irradiance volume for indirect + clustered forward+ dynamic lights for direct + shadow maps + tangent-space normal maps. One phase, one deliverable.

The current FGD (`assets/postretro.fgd`) defines `env_fog_volume`, `env_cubemap`, and `env_reverb_zone`. No light entities exist yet — Phase 3 was runnable without them. This plan adds them as the front of the pipeline.

Postretro's convention is to research established solutions before inventing. Several references inform the spec:

- **id Tech 4 (Doom 3 / Quake 4)** — irradiance volumes with per-area probe storage. GDC talks and open-source id Tech 4 ports document the approach end-to-end. The closest architectural match.
- **Frostbite and modern AAA** — SH L1/L2 probe storage in regular 3D grids, trilinear interpolation, validity masks. The direct precedent for the chosen SH L2 + regular grid approach.
- **Source Engine** — ambient cubes (six-axis per-probe RGB). Considered and rejected: SH L2 gives smoother reconstruction at similar storage cost.
- **ericw-tools `LIGHTGRID_OCTREE`** — sparse octree probe samples as a BSPX lump. The long-standing Quake-family answer. Considered and rejected: adaptive density is more complex for modest gain when the target map size is small and indoor.
- **PBR lighting schemas** — structural alignment target for the canonical light format (falloff, cone, intensity axes). Postretro is not shipping PBR materials, but matching the structural shape means the format isn't painted into a corner.
- **Rust ecosystem** — crates for SH projection (`spherical_harmonics`, hand-rolled per rendering_pipeline.md lineage) and ray-triangle intersection (`embree-rs`, `bvh`, hand-rolled). Preferred over writing from scratch if solid.

---

## Scope

### In scope

- Full lighting pipeline spec: FGD entities, Quake-map translator, canonical light format, SH irradiance volume baker, runtime SH sampling, clustered forward+ dynamic lighting, shadow maps, normal map rendering, PRL section shape.
- FGD file at `assets/postretro.fgd` adding `light`, `light_spot`, `light_sun`.
- `postretro-level-compiler/src/format/quake_map.rs` translator module. Pattern is `format/<name>.rs` per source format; each format's internal structure is its own decision.
- Parser wiring: `prl-build` extracts light entity properties into a property bag and dispatches to the translator.
- Validation rules (errors block compilation; warnings log and proceed).
- Quake `style` integer → `LightAnimation` preset conversion. Canonical format is preset-free; the translator owns the Quake style table.
- SH irradiance volume baker: probe placement, radiance evaluation with shadow raycasting, SH L2 projection, validity masking, PRL section writer.
- Runtime SH volume sampling: parse PRL section to 3D texture, trilinear sampling in world shader.
- Normal map loading and tangent-space shading in the world shader.
- Clustered light list compute prepass: cluster grid definition, per-cluster light index lists, fragment-shader walk.
- Shadow map pipeline: CSM for directional, cube shadow maps for point/spot, sampling in the world shader.
- Test map coverage extending `assets/maps/test.map`.
- Documentation update to `context/lib/build_pipeline.md` §Custom FGD table.

### Out of scope

- UDMF map-format support. Separate initiative. The `format/<name>.rs` architecture established here accommodates it without refactor.
- `env_projector` and texture-projecting lights.
- IES profiles / photometric data.
- Area lights (rectangle, disk). Point / spot / directional cover the target feature set.
- Second-bounce indirect. The SH volume captures direct-to-static bounces; multi-bounce is a follow-up if visuals demand it.
- Runtime dynamic probe updates (DDGI-style). The SH volume is baked, read-only at runtime.
- Runtime evaluation of light animation curves. The baker bakes animation into probe sample curves at compile time; runtime evaluation of dynamic light animations is a Phase 5 follow-up.
- Hardware ray tracing. wgpu does not expose it; shadow maps cover runtime shadowing.
- Mesh shaders. wgpu does not expose them; GPU culling uses compute + indirect draws.
- Exhaustive academic literature review. "Survey what's known and pick a direction," not "produce a novel contribution."
- Benchmarking probe baking performance. Decisions are made on design grounds; benchmarking belongs in the execution plan once sizing questions come up.

---

## Pipeline

```
TrenchBroom authoring (FGD)
  → .map file
    → prl-build parser (extract property bag)
      → format::quake_map::translate_light (validate, convert units, expand presets)
        → CanonicalLight in MapData.lights
          ├─→ SH irradiance volume baker (static-geometry raycast, SH L2 projection)
          │     → SH section in .prl
          │       → runtime trilinear sample (fragment shader, indirect term)
          └─→ runtime direct light buffer (canonical lights + transient gameplay lights)
                → clustered light list compute prepass
                  → cluster walk in fragment shader (direct term)
                    → shadow map sampling per shadow-casting light
```

Each stage below is one section of the spec. All stages are decided.

---

## Stage 1 — Mapper authoring (FGD)

Three entities: `light`, `light_spot`, `light_sun`. Mappers author with familiar Quake FGD syntax; SmartEdit renders `_color` as a color picker, `delay` as a dropdown, `_fade` as a text field.

### Property → canonical mapping

| FGD Property | Type | Maps to Canonical | Default / Required |
|--------------|------|-------------------|-----|
| `light` | integer | `intensity` | 300 |
| `_color` | color255 (0–255 RGB) | `color` (normalized to 0–1 linear) | 255 255 255 |
| `_fade` | integer (map units) | `falloff_range` | **Required** for Point/Spot; ignored for Directional |
| `delay` | choices | `falloff_model` (0=Linear, 1=InverseDistance, 2=InverseSquared) | 0 (Linear) |
| `_cone` | integer (degrees) | `cone_angle_inner` (converted to radians) | 30 (Spot only) |
| `_cone2` | integer (degrees) | `cone_angle_outer` (converted to radians) | 45 (Spot only) |
| `style` | integer (0–11) | `animation` (preset → sample curves) | 0 (no animation) |
| `mangle` or `target` | vector or target name | `cone_direction` | **Required for Spot; error if missing** |

### FGD template

```fgd
@BaseClass = Light
[
    light(integer) : "Intensity" : 300
    _color(color255) : "Color" : "255 255 255"
    _fade(integer) : "Falloff Distance" : 60000
    delay(choices) : "Falloff Model" : 0 =
    [
        0 : "Linear"
        1 : "Inverse Distance (1/x)"
        2 : "Inverse Squared (1/x²)"
    ]
    style(integer) : "Animation Style" : 0
]

@PointClass base(Light)
    color(255 200 0)
    size(-8 -8 -8, 8 8 8)
    = light : "Point Light"
[
    origin(origin)
]

@PointClass base(Light)
    color(255 150 0)
    size(-8 -8 -8, 8 8 8)
    = light_spot : "Spotlight"
[
    origin(origin)
    _cone(integer) : "Inner Cone Angle" : 30
    _cone2(integer) : "Outer Cone Angle" : 45
    mangle(string) : "Direction (pitch yaw roll)" : ""
    target(target_destination) : "Target Entity" : ""
]

@PointClass base(Light)
    color(255 100 0)
    size(-8 -8 -8, 8 8 8)
    = light_sun : "Directional Light"
[
    origin(origin)
    mangle(string) : "Direction (pitch yaw roll)" : ""
]
```

`assets/postretro.fgd` mirrors the texture pipeline precedent. TrenchBroom game configuration references this path.

---

## Stage 2 — Parse and translate

### Architecture

```
postretro-level-compiler/src/
  format/
    mod.rs              — module root
    quake_map.rs        — translate_light() for Quake-family .map entities
  map_data.rs           — CanonicalLight and canonical types (shared)
  parse.rs              — extracts property bag from shambler, dispatches to translator
```

The parser performs thin property extraction only: for each light entity, pull key-value pairs from shambler into `HashMap<String, String>` and hand off. The translator has no shambler dependency — it operates on the property bag plus origin plus classname. Future formats (`format/udmf.rs`) add sibling modules against the same canonical types; the parser dispatches by source format.

Translator signature:

```rust
pub fn translate_light(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<CanonicalLight, TranslateError>;
```

### Validation rules

Errors block compilation. Warnings log and proceed with defaults.

| Case | Error / Warning | Handling |
|------|-----------------|----------|
| Point/Spot light missing `_fade` | **Error** | Compilation fails. Mapper must specify falloff distance. |
| Spot missing both `mangle` and `target` | **Error** | Compilation fails. Mapper must aim spotlight. |
| Spot with `target="nonexistent"` | **Error** | Compilation fails: "target entity 'X' not found." |
| Invalid property format (non-numeric `_fade`, malformed `mangle`) | **Error** | Compilation fails with property name. |
| `light` = 0 | **Warning** | Intensity is zero; light contributes nothing. |
| Missing `_color` | **Warning** | Defaults to white. |
| Spot missing `_cone` / `_cone2` | **Warning** | Defaults to 30° / 45°. |
| Missing `style` | **Warning** | Defaults to no animation. |
| Spot with `_cone` > `_cone2` | **Warning** | Outer smaller than inner; proceed as specified. |

### Translator notes

- `light` is unitless. Typical Quake-family range is 0–300; the baker (Stage 4) may normalize against chosen bake output, but the range is translator convention for Quake source maps, not a canonical format constraint.
- `_fade` required for deterministic baking. Guideline: `_fade ≈ light × 200` (e.g., `light 300` → `_fade 60000`); adjust per map scale.
- Spotlight direction via `mangle` (pitch yaw roll in degrees, engine space) or `target` (entity name). If both provided, `target` takes precedence.
- Cone degrees → radians conversion happens at the translation boundary. Canonical format is radians-only.
- `style` 0–11 map to `LightAnimation` sample curves. The translator owns the Quake style table (classic preset strings like `aaaaaaaaaa` for constant, `mmnmmommommnonmmonqnmmo` for flicker) and converts them to normalized brightness sample vectors. Styles 12+ reserved for future use.
- Property name variation: accept both `light` and `_light` (Quake community naming variations across tools).

### Test map content

`assets/maps/test.map` gains:

```
// spotlight — inverse-squared falloff, warm color
{
"classname" "light_spot"
"origin" "800 200 96"
"light" "200"
"_color" "255 200 100"
"_fade" "40000"
"_cone" "25"
"_cone2" "50"
"mangle" "-45 0 0"
"delay" "2"
}

// directional — cool light
{
"classname" "light_sun"
"origin" "960 128 500"
"light" "150"
"_color" "200 200 255"
"mangle" "-60 45 0"
}

// red point light — inverse-distance falloff
{
"classname" "light"
"origin" "1100 128 96"
"light" "250"
"_color" "255 50 50"
"_fade" "50000"
"delay" "1"
}
```

---

## Stage 3 — Canonical light format

The compiler translates every supported map format into this canonical form. The baker has no source-format awareness; it sees only `Vec<CanonicalLight>`.

Structural shape aligns with PBR lighting conventions (light type, position/direction, color, intensity, falloff, cone) so the format isn't painted into a corner. Units are not physical — retro aesthetic allows non-physical falloff models — but the axes are the same axes a PBR format would use.

```rust
pub enum LightType {
    Point,          // omnidirectional
    Spot,           // cone, uses cone_angle_inner/outer + direction
    Directional,    // parallel directional (e.g., sunlight); ignores falloff_range
}

pub enum FalloffModel {
    Linear,          // brightness = 1 - (distance / falloff_range)
    InverseDistance, // brightness = 1 / distance, clamped at falloff_range
    InverseSquared,  // brightness = 1 / (distance²), clamped at falloff_range
}

/// Primitive animation: time-sampled curves over a cycle.
/// Format-agnostic — Quake light styles, Doom sector effects, or hand-authored
/// curves all translate into this shape. `None` fields mean "constant across cycle."
pub struct LightAnimation {
    pub period: f32,                  // cycle duration in seconds
    pub phase: f32,                   // 0-1 offset within cycle
    pub brightness: Option<Vec<f32>>, // multipliers sampled uniformly over cycle
    pub hue_shift: Option<Vec<f32>>,  // HSL hue offset 0-1, sampled uniformly over cycle
    pub saturation: Option<Vec<f32>>, // saturation multipliers sampled uniformly over cycle
}

pub struct CanonicalLight {
    // Spatial
    pub origin: DVec3,                    // position (engine space, meters)
    pub light_type: LightType,

    // Appearance
    pub intensity: f32,                   // brightness scalar, unitless
    pub color: [f32; 3],                  // linear RGB, 0-1

    // Falloff (Point and Spot only; ignored for Directional)
    pub falloff_model: FalloffModel,
    pub falloff_range: f32,               // distance at which light reaches zero; must be > 0

    // Spotlight parameters (Spot only; None for Point and Directional)
    pub cone_angle_inner: Option<f32>,    // radians, full brightness
    pub cone_angle_outer: Option<f32>,    // radians, fade edge
    pub cone_direction: Option<[f32; 3]>, // normalized aim vector; Directional uses this too

    // Animation (None = constant light)
    pub animation: Option<LightAnimation>,
}
```

`MapData` gains `lights: Vec<CanonicalLight>`.

### Design notes

- **`LightAnimation` as a format primitive.** Quake light styles (`Flicker`, `Candle`, `FastStrobe`) are translator output, not canonical format vocabulary. Each style preset becomes a brightness/hue/saturation sample vector. Future formats translate into the same primitive. The canonical format has no awareness of any specific source format's vocabulary.
- **Cone angles in radians.** Canonical format is engine-internal; FGDs expose degrees to mappers and the translator converts.
- **`falloff_range`.** PBR-conventional naming.
- **`FalloffModel` enum retained despite PBR alignment.** PBR uses physical inverse-square only; the retro aesthetic needs linear and inverse-distance as well for authored looks. Structural alignment with PBR is about *axes*, not *physics*.
- **`LightType::Directional` (not `Sun`).** Directional is a graphics primitive; "Sun" implies a specific use case. Does not require or imply global illumination — probe-based lighting samples directional lights the same way it samples point lights.
- **Intensity unitless.** Baker may normalize against chosen bake output; refined after probe visuals.
- **`bake_only` / `cast_shadows` split.** Canonical lights feed both the bake (static-only, raycast-shadowed) and the runtime direct path (dynamic, shadow-mapped). A follow-up may add per-light flags if authors need a bake-only or runtime-only subset — not in the initial scope.

---

## Stage 4 — SH irradiance volume baker

Postretro bakes indirect illumination into a regular 3D grid of SH L2 probes. Direct illumination is *not* baked — it is evaluated at runtime by the clustered forward+ path. This split lets dynamic lights co-exist with baked indirect without lightmap complexity, and keeps probe data read-only at runtime.

### Spatial layout: regular 3D grid

Probes sit on an axis-aligned grid that spans the level's AABB with a configurable cell size (default: 1 meter). The grid is the full coverage — no sparse octree, no per-leaf alignment. Rationale:

- Trivial to index: `(x, y, z)` grid coordinate maps directly to a 3D texture texel.
- Hardware trilinear filtering in the fragment shader, zero work in shader code.
- Target maps are small and indoor — octree adaptivity saves little.
- Probes inside solid geometry are flagged invalid; invalid probes are never sampled (see validity mask below).

Compiler flag `--probe-spacing <meters>` overrides the default. Tighter spacing near floors is handled by a second vertical-tier override in future work, not in the initial cut.

### Per-probe storage: SH L2

Nine SH basis coefficients per color channel × three channels = **27 f32 per probe**. SH L2 captures directional incoming radiance with enough fidelity for smooth indirect shading, and the reconstruction math is a single dot product per channel in the fragment shader.

Rejected alternatives:
- **Plain RGB** — loses directional information; flat indirect looks wrong on curved or angled surfaces.
- **Ambient cube** — 18 f32 per probe for comparable quality; SH L2 wins on smoothness.
- **SH L1** — 12 f32 per probe; cheaper but noticeably blurrier on test scenes with colored directional indirect.

### Validity mask

Each probe has a `u8` validity flag: `0` = invalid (inside solid), `1` = valid (usable). Validity is determined at bake time by sampling the BSP tree at the probe position — solid leaves produce invalid probes. Runtime sampling uses the mask to fall back to nearby valid probes when the trilinear footprint crosses a wall.

**Leak mitigation.** A mean-distance-to-nearest-surface field per probe direction (as used in DDGI) is a follow-up if simple validity masking proves insufficient on the test maps. The initial cut ships validity-only.

### Bake algorithm

For each valid probe:

1. Fire **N stratified sample rays** from the probe (default `N = 256`) distributed over the sphere.
2. For each ray, intersect against the static geometry's triangle set. Miss → sky/ambient. Hit → evaluate direct light at the hit point (shadow raycasts from each canonical light, sum Lambert contributions), then attenuate by surface albedo approximation to approximate one bounce.
3. Project the incoming radiance samples into SH L2 coefficients.
4. Store coefficients in the probe grid; write validity flag.

Ray count, BVH choice, and parallelism strategy are execution details — the plan fixes the algorithm shape, not the sizing. Rust crate options: `bvh` for acceleration, `rayon` for parallelism.

### Animation baking

Lights with animation curves bake into a sample vector per probe: `period` seconds discretized into `sample_count` entries (default 11 samples/cycle). At runtime, the shader reads the current sample and blends with the next. Memory overhead: `probes × animated_lights × samples × 4 bytes`. A 60 × 60 × 20 grid (72k probes) with 5 animated lights and 11 samples is ~16 MB — acceptable upper bound for a large level. Small levels pay proportionally less.

The initial cut may defer animation baking — a static-only first revision that ignores `LightAnimation` is acceptable if it simplifies the first end-to-end path. Execution plan decides.

### Shadow strategy

Bake-time raycast occlusion. Each canonical light contribution at a probe is modulated by a shadow ray from the light position (or direction, for `Directional`) to the probe. Visible → full contribution; occluded → zero. This is the full cost during the bake, but the bake happens once per compile.

Runtime dynamic lights rely on shadow maps (see Stage 6), not probe data.

---

## Stage 5 — PRL section shape

New PRL section for the SH irradiance volume. Section ID to be allocated in `postretro-level-format/src/lib.rs` alongside existing section IDs.

### Layout

All little-endian. Header, then packed probe records.

```
Header (32 bytes):
  f32 × 3    grid_origin      (world-space min corner, meters)
  f32 × 3    cell_size        (meters per cell along x/y/z)
  u32 × 3    grid_dimensions  (probe count along x/y/z)
  u32        probe_stride     (bytes per probe record; 112 for static-only, more with animation)

Probe records (probe_stride bytes each, iterated z-major then y, then x):
  f32 × 27   sh_coefficients  (9 bands × 3 channels)
  u8         validity         (0 = invalid, 1 = valid)
  u8 × 3     padding          (align to 4 bytes)
```

Total static-only probe record: `27 × 4 + 4 = 112 bytes`.

### Runtime upload

27 scalars per probe don't fit in one `Rgba16Float` texel (4 scalars) — need `ceil(27 / 4) = 7` texels minimum. The loader splits probe data across multiple 3D textures at probe-grid resolution, sampled with hardware trilinear. Preferred layout (Unity/Frostbite/DDGI lineage): three slab textures per color channel (9 total), each slab holding three SH bands. Alternative: 7 textures interleaving all 27 scalars. Either is a renderer implementation detail.

The **PRL section is the source of truth**: 27 f32 per probe, contiguous, in baker write order. Runtime splits as it prefers.

Invalid probes upload as zeroed SH coefficients so the trilinear filter degrades across wall boundaries.

### Compatibility

Missing section is not an error. The world shader degrades to flat white ambient when the section is absent, matching Phase 3.5 behavior.

---

## Stage 6 — Runtime direct lighting and shadow maps

Covered here for completeness; the full architectural write-up lives in `context/lib/rendering_pipeline.md` §4 and §7.

### Clustered forward+ light list

Compute prepass runs each frame:

1. Iterate active lights (canonical lights from `MapData::lights` + transient gameplay lights from the entity system).
2. For each cluster in the view-space grid (screen tiles × depth slices), test light volumes against the cluster AABB.
3. Write a packed per-cluster index list to a storage buffer.

Grid sizing and tile dimensions are execution details refined during implementation.

### Shadow maps

- **Directional lights** — cascaded shadow maps (CSM). 3 or 4 cascades; resolution intentionally modest (e.g., 1024² per cascade) to match the aesthetic.
- **Point lights** — cube shadow maps rendered in a single pass via layered rendering where supported, or six passes otherwise.
- **Spot lights** — single shadow map per light.

Not every dynamic light casts shadows. A `cast_shadows: bool` flag on the runtime light struct (not the canonical light) gates rendering a shadow map; static canonical lights derived from FGD may default to true, transient gameplay lights to false.

### Normal maps

Albedo + normal map per texture. Normal maps load as BC5 (RG) when available, interpreted as tangent-space `(x, y)` with `z` reconstructed. Missing normal map falls back to `(0, 0, 1)` — neutral. The vertex shader reconstructs TBN from packed normal and tangent; the fragment shader applies the normal-map perturbation before direct and indirect shading.

---

## Resolved questions

These questions were open in the prior draft and are now decided:

| Question | Decision | Rationale |
|----------|----------|-----------|
| Baker approach | Probes only for indirect; dynamic direct at runtime | Lightmaps add a bake stage, an atlas, a UV channel, and a two-texture sampling path in the shader. SH volume + dynamic direct is lighter. |
| Spatial layout | Regular 3D grid | Trivial indexing, hardware trilinear, low complexity. Octree adaptivity wins little on small indoor maps. |
| Per-probe storage | SH L2 (27 f32/probe) | Smooth reconstruction, small shader cost, industry-standard. Ambient cube rejected as needing nearly as much storage for less smoothness. |
| Probe evaluation | Trilinear interpolation on a 3D texture | Hardware-accelerated; zero shader complexity. |
| Shadow strategy (bake) | Raycast at bake time, per light per probe | Bake is expensive but runs once. Runtime shadow estimation on the volume is not worth the complexity. |
| Shadow strategy (runtime) | Shadow maps per dynamic shadow-caster | CSM for directional, cube for point/spot. Matches aesthetic (chunky edges at modest resolution). |
| Animation baking | Bake per-probe sample vectors; defer to execution plan | Animation support may be cut from the initial revision if it complicates the first pass. |
| PRL section shape | Header + probe records; see Stage 5 | Fixed layout, forward-compatible via `probe_stride`. |

---

## Acceptance criteria

### For draft → ready

- Canonical format confirmed stable (stages 1–3 unchanged from prior draft, already reviewed).
- SH irradiance volume design confirmed against the new rendering pipeline architecture (stage 4 above).
- PRL section shape sketched concretely enough that the execution plan can anchor on it (stage 5 above).
- Direct-lighting and shadow-map integration points match `context/lib/rendering_pipeline.md` §4 and §7.
- Rust crate options for BVH and SH projection listed; detailed fitness assessment happens in the execution plan.

### For implementation — stages 1–3 (FGD, translator, canonical format)

1. **FGD file created and verified:** `assets/postretro.fgd` defines `light`, `light_spot`, `light_sun` with Quake-standard properties in TrenchBroom-compatible FGD syntax. FGD loads in TrenchBroom without errors; entity browser shows all three; SmartEdit renders color picker / dropdown / text field correctly.

2. **Translator module:** `postretro-level-compiler/src/format/quake_map.rs` implements `translate_light()` per the signature above. No shambler dependency. Owns Quake `style` → `LightAnimation` preset conversion. Converts cone angles degrees → radians at the translation boundary.

3. **Parser integration:** `parse.rs` recognizes `light`, `light_spot`, `light_sun`, extracts properties into `HashMap<String, String>`, calls the translator, populates `MapData::lights`. Errors block compilation; warnings log.

4. **`MapData` extended:** `lights: Vec<CanonicalLight>` field added. Canonical types defined in `map_data.rs`.

5. **Validation:** Every row of the validation table is covered. Errors block; warnings log.

6. **Test map coverage:** `assets/maps/test.map` includes point + spot + directional lights, non-white color, and non-zero `delay`. Map compiles without errors.

7. **Documentation:** `context/lib/build_pipeline.md` §Custom FGD table includes rows for `light`, `light_spot`, `light_sun` (already updated).

8. **Unit tests for translator:**
   - Valid point / spot (via target) / spot (via mangle) / directional → canonical conversion.
   - Point/spot missing `_fade` → error.
   - Spot missing both `mangle` and `target` → error.
   - `mangle` with non-numeric values → error.
   - Multi-naming: both `light` and `_light` property names accepted.
   - `style` = 1 → `LightAnimation` with non-None brightness curve; `style` = 0 → `animation: None`.

9. **No runtime engine changes in stages 1–3.** Canonical lights available via `MapData::lights` for the Stage 4 baker and Stage 6 runtime direct path.

### For implementation — stages 4–5 (SH baker + PRL section)

1. **PRL section allocated:** new section ID in `postretro-level-format/src/lib.rs` for the SH irradiance volume, with read/write round-trip tests matching the existing section pattern.

2. **Baker stage in prl-build:** runs after geometry extraction and before pack. Produces a 3D grid of probe records following the Stage 4 algorithm: stratified sphere sampling, static-geometry raycast, SH L2 projection, validity masking.

3. **Determinism:** identical input `.map` produces identical SH coefficients. Stratified sampling uses a fixed seed.

4. **Default probe spacing** (1 m) and CLI override (`--probe-spacing`) implemented.

5. **Probe validity mask populated** from BSP solid/empty classification.

6. **Bake parallelism** via `rayon` — one task per probe or per probe slab.

### For implementation — stage 6 (runtime lighting)

1. **SH volume loader:** parse PRL section, upload to a 3D texture.

2. **World shader extended:** sample SH volume trilinearly, reconstruct irradiance via SH L2 dot product, replace flat ambient with the result.

3. **Normal map path:** load normal maps alongside albedo, reconstruct TBN in vertex shader, perturb fragment normal before shading.

4. **Clustered light list compute prepass:** iterate active lights, build per-cluster index lists in a storage buffer.

5. **World shader direct term:** walk the fragment's cluster, accumulate direct contributions from each light, sample shadow maps for shadow-casting lights.

6. **Shadow map passes:** CSM for directional, cube map for point, single map for spot. Run before the opaque pass each frame.

7. **Visual validation:** lighting test maps (point, spot, directional; bright and dark corners; curved walls; normal-mapped surfaces) look correct. Indirect light bleeds around corners; direct falloff matches the falloff model; shadows are crisp at the chosen resolution.

---

## Implementation tasks

All stages are now concrete. `/orchestrate` can break this plan into execution chunks.

### Stage 1 — FGD file

1. Create `assets/postretro.fgd` with `light`, `light_spot`, `light_sun` per the template above.
2. Verify in TrenchBroom: copy to game config folder, open editor, confirm entity browser + SmartEdit property widgets behave without errors.

### Stage 2 — Parse and translate

3. Create `postretro-level-compiler/src/format/mod.rs` and `format/quake_map.rs`. Implement `translate_light()` per signature. Include Quake style preset table and degrees-to-radians conversion.
4. Extend `postretro-level-compiler/src/parse.rs`: recognize light classnames, extract property bag, dispatch to translator, propagate errors and warnings.
5. Extend `assets/maps/test.map` with the three example entities above.
6. Write translator unit tests covering every validation rule and the style preset conversion.

### Stage 3 — Canonical format

7. Add canonical types (`LightType`, `FalloffModel`, `LightAnimation`, `CanonicalLight`) to `postretro-level-compiler/src/map_data.rs`.
8. Add `lights: Vec<CanonicalLight>` field to `MapData`.

### Stage 4 — SH baker

9. Allocate a new PRL section ID for the SH irradiance volume.
10. Add probe record and section types to `postretro-level-format` with read/write + round-trip tests.
11. Implement probe placement: regular grid over map AABB at configurable spacing; solidity query against BSP.
12. Implement radiance sampling: stratified sphere rays, triangle intersection against static geometry, per-light shadow raycasts, Lambert evaluation at hit points.
13. Implement SH L2 projection from radiance samples.
14. Parallelize with `rayon`; expose `--probe-spacing` CLI flag.

### Stage 5 — PRL section

15. Wire the SH volume section into the prl-build pack stage.
16. Engine loader parses the section and produces a GPU-ready upload descriptor (no wgpu calls in the loader).

### Stage 6 — Runtime lighting and normal maps

17. Renderer: create SH volume 3D texture, upload from loader data, bind in the world shader.
18. World shader: trilinear SH sample → irradiance reconstruction → replaces flat ambient.
19. Normal map loading: albedo + normal texture pair per material; BC5 preferred, fallback RG8.
20. Vertex shader: reconstruct TBN from packed normal + tangent + bitangent sign.
21. Fragment shader: sample normal map, apply TBN transform, shade with SH irradiance.
22. Clustered light list compute prepass: implement tile/slice grid, per-cluster index list build.
23. World shader direct term: cluster walk, Lambert/Phong direct evaluation, shadow map sampling.
24. Shadow map passes: CSM for directional, cube for point, single map for spot.
25. Lighting test maps: author scenes that exercise indirect bleed, direct falloff, shadow crispness, and normal-map angle variation.

### Docs

26. On ship, migrate the canonical format and pipeline sections into `context/lib/rendering_pipeline.md` §4 (already updated in this refactor).

---

## When this plan ships

Durable architectural decisions migrate to `context/lib/rendering_pipeline.md` (`context/lib/lighting.md` if the section outgrows §4). Candidates for migration:

- Canonical format struct shape and design rationale.
- `format/<name>.rs` architecture for multi-format source support.
- SH volume spatial layout, per-probe storage, validity masking.
- PRL section shape.
- Clustered forward+ cluster grid parameters and shadow map defaults.

The plan document itself is ephemeral per `development_guide.md` §1.5.
