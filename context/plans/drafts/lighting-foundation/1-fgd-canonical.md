# Sub-plan 1 — FGD, Translator, Canonical Format

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** all compiler-side authoring and translation work. FGD entities, parser wiring, translator module, canonical light format. No engine changes, no baker, no runtime.
> **Crates touched:** `postretro-level-compiler`, plus `assets/postretro.fgd` and `assets/maps/test.map`.
> **Depends on:** Milestone 4 (BVH Foundation) sign-off. No technical dependency on Milestone 4 within this sub-plan, but the rule is that nothing in `lighting-foundation/` starts until Milestone 4 ships.
> **Blocks:** sub-plan 2 (the SH baker consumes `MapData.lights`).

---

## Description

Three deliverables, sequenced:

1. **FGD entities** in `assets/postretro.fgd` so mappers can author lights in TrenchBroom.
2. **Translator module** at `postretro-level-compiler/src/format/quake_map.rs` that converts mapper-facing FGD properties into the canonical light format. Owns the Quake `style` preset table and the degrees-to-radians conversion at the boundary.
3. **Canonical light format** in `postretro-level-compiler/src/map_data.rs` — the format-agnostic struct that the baker (sub-plan 2) and runtime direct path (sub-plan 3) both consume.

Output: every test map's `MapData.lights` is populated correctly; validation errors block compilation; warnings log and proceed.

---

## FGD entities

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
| `mangle` | vector (pitch yaw roll degrees) | `cone_direction` | **Required for Spot; error if missing** |
| `target` | target name | — | **Deferred to Milestone 6** (entity system needed to resolve names to origins); error if set: "use `mangle` for spotlight direction" |

### FGD template

```fgd
@BaseClass = Light
[
    light(integer) : "Intensity" : 300
    _color(color255) : "Color" : "255 255 255"
    _fade(integer) : "Falloff Distance"
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
    // target entity direction is deferred to Milestone 6 (requires entity system)
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

## Translator architecture

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
| Spot missing `mangle` | **Error** | Compilation fails. Mapper must aim spotlight via `mangle`. |
| Spot with `target` set | **Error** | Compilation fails: "`target` not supported until Milestone 6; use `mangle` for spotlight direction." |
| Invalid property format (non-numeric `_fade`, malformed `mangle`) | **Error** | Compilation fails with property name. |
| `light` = 0 | **Warning** | Intensity is zero; light contributes nothing. |
| Missing `_color` | **Warning** | Defaults to white. |
| Spot missing `_cone` / `_cone2` | **Warning** | Defaults to 30° / 45°. |
| Missing `style` | **Warning** | Defaults to no animation. |
| Spot with `_cone` > `_cone2` | **Warning** | Outer smaller than inner; proceed as specified. |

### Translator notes

- `light` is unitless. Typical Quake-family range is 0–300; the baker (sub-plan 2) may normalize against chosen bake output, but the range is translator convention for Quake source maps, not a canonical format constraint.
- `_fade` required for deterministic baking. Guideline: `_fade ≈ light × 200` (e.g., `light 300` → `_fade 60000`); adjust per map scale.
- Spotlight direction via `mangle` (pitch yaw roll in degrees, engine space) only. `target` entity resolution is deferred to Milestone 6 — the entity system is needed to look up entity origins by name. If `target` is present, emit an error directing the mapper to use `mangle`.
- Cone degrees → radians conversion happens at the translation boundary. Canonical format is radians-only.
- `style` 0–11 map to `LightAnimation` sample curves. The translator owns the Quake style table (classic preset strings like `aaaaaaaaaa` for constant, `mmnmmommommnonmmonqnmmo` for flicker) and converts them to normalized brightness sample vectors. Styles 12+ reserved for future use.
- Property name variation: accept both `light` and `_light` (Quake community naming variations across tools).

---

## Canonical light format

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

## Test map content

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

## Acceptance criteria

- [ ] `assets/postretro.fgd` defines `light`, `light_spot`, `light_sun` with Quake-standard properties in TrenchBroom-compatible FGD syntax. FGD loads in TrenchBroom without errors; entity browser shows all three; SmartEdit renders color picker / dropdown / text field correctly.
- [ ] `postretro-level-compiler/src/format/quake_map.rs` implements `translate_light()` per the signature above. No shambler dependency. Owns Quake `style` → `LightAnimation` preset conversion. Converts cone angles degrees → radians at the translation boundary.
- [ ] `parse.rs` recognizes `light`, `light_spot`, `light_sun`, extracts properties into `HashMap<String, String>`, calls the translator, populates `MapData::lights`. Errors block compilation; warnings log.
- [ ] Canonical types (`LightType`, `FalloffModel`, `LightAnimation`, `CanonicalLight`) defined in `map_data.rs`. `MapData` gains `lights: Vec<CanonicalLight>`.
- [ ] Every row of the validation table is covered. Errors block; warnings log.
- [ ] `assets/maps/test.map` includes point + spot + directional lights, non-white color, and non-zero `delay`. Map compiles without errors.
- [ ] `context/lib/build_pipeline.md` §Custom FGD table includes rows for `light`, `light_spot`, `light_sun`.
- [ ] Unit tests for translator:
  - Valid point / spot (via target) / spot (via mangle) / directional → canonical conversion.
  - Point/spot missing `_fade` → error.
  - Spot missing both `mangle` and `target` → error.
  - `mangle` with non-numeric values → error.
  - Multi-naming: both `light` and `_light` property names accepted.
  - `style` = 1 → `LightAnimation` with non-None brightness curve; `style` = 0 → `animation: None`.
- [ ] No runtime engine changes in this sub-plan. Canonical lights available via `MapData::lights` for sub-plan 2's baker.
- [ ] `cargo test -p postretro-level-compiler` passes
- [ ] `cargo clippy -p postretro-level-compiler -- -D warnings` clean

---

## Implementation tasks

1. Create `assets/postretro.fgd` with `light`, `light_spot`, `light_sun` per the template above. Verify in TrenchBroom: copy to game config folder, open editor, confirm entity browser + SmartEdit property widgets behave without errors.

2. Create `postretro-level-compiler/src/format/mod.rs` and `format/quake_map.rs`. Implement `translate_light()` per signature. Include Quake style preset table and degrees-to-radians conversion.

3. Add canonical types (`LightType`, `FalloffModel`, `LightAnimation`, `CanonicalLight`) to `postretro-level-compiler/src/map_data.rs`. Add `lights: Vec<CanonicalLight>` field to `MapData`.

4. Extend `postretro-level-compiler/src/parse.rs`: recognize light classnames, extract property bag, dispatch to translator, propagate errors and warnings.

5. Extend `assets/maps/test.map` with the three example entities above.

6. Write translator unit tests covering every validation rule and the style preset conversion.

7. Update `context/lib/build_pipeline.md` §Custom FGD table.
