# Sub-plan 1 — FGD, Translator, Map Light Format

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** all compiler-side authoring and translation work. FGD entities, parser wiring, translator module, map light format. No engine changes, no baker, no runtime.
> **Crates touched:** `postretro-level-compiler`, plus `assets/postretro.fgd` and `assets/maps/test.map`.
> **Depends on:** Milestone 4 (BVH Foundation) sign-off. No technical dependency on Milestone 4 within this sub-plan, but the rule is that nothing in `lighting-foundation/` starts until Milestone 4 ships.
> **Blocks:** sub-plan 2 (the SH baker consumes `MapData.lights`).

---

## Description

Three deliverables, sequenced:

1. **FGD entities** in `assets/postretro.fgd` so mappers can author lights in TrenchBroom.
2. **Translator module** at `postretro-level-compiler/src/format/quake_map.rs` that converts mapper-facing FGD properties into the map light format. Owns the Quake `style` preset table and the degrees-to-radians conversion at the boundary.
3. **Map light format** in `postretro-level-compiler/src/map_data.rs` — the format-agnostic struct that the baker (sub-plan 2) and runtime direct path (sub-plan 3) both consume.

Output: every test map's `MapData.lights` is populated correctly; validation errors block compilation; warnings log and proceed.

---

## FGD entities

Three entities: `light`, `light_spot`, `light_sun`. Mappers author with familiar Quake FGD syntax; SmartEdit renders `_color` as a color picker, `delay` as a dropdown, `_fade` as a text field.

### Property → canonical mapping

| FGD Property | Type | Maps to MapLight | Default / Required |
|--------------|------|-------------------|-----|
| `light` | integer | `intensity` | 300 |
| `_color` | color255 (0–255 RGB) | `color` (normalized to 0–1 linear) | 255 255 255 |
| `_fade` | integer (map units) | `falloff_range` | **Required** for Point/Spot; ignored for Directional |
| `delay` | choices | `falloff_model` (0=Linear, 1=InverseDistance, 2=InverseSquared) | 0 (Linear) |
| `_cone` | integer (degrees) | `cone_angle_inner` (converted to radians) | 30 (Spot only) |
| `_cone2` | integer (degrees) | `cone_angle_outer` (converted to radians) | 45 (Spot only) |
| `style` | integer (0–11) | `animation` (preset → sample curves) | 0 (no animation) |
| `_phase` | string (0.0–1.0) | `animation.phase` | 0.0 (sync with cycle start) |
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
    _phase(string) : "Animation Phase Offset (0.0-1.0)" : "0.0"
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
  map_data.rs           — MapLight and shared light types (shared)
  parse.rs              — extracts property bag from shambler, dispatches to translator
```

The parser performs thin property extraction only: for each light entity, pull key-value pairs from shambler into `HashMap<String, String>` and hand off. The translator has no shambler dependency — it operates on the property bag plus origin plus classname. Future formats (`format/udmf.rs`) add sibling modules against the same canonical types; the parser dispatches by source format.

Translator signature:

```rust
pub fn translate_light(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<MapLight, TranslateError>;
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
| Directional missing `mangle` | **Warning** | Defaults to straight down: `"-90 0 0"` (pitch −90°, aim vector `(0, −1, 0)`). |
| `_phase` outside 0.0–1.0 | **Warning** | Clamp to 0.0–1.0; proceed. |
| `_phase` set but `style` = 0 | **Warning** | Phase has no effect without animation; ignored. |

### Translator notes

- `light` is authored in the Quake 0–300 radiosity-energy convention where `300` is the "fully lit room" default. The translator normalizes to the canonical `MapLight.intensity` by dividing by `QUAKE_INTENSITY_REFERENCE = 300.0`, so an authored `light 300` lands at `intensity 1.0` and an authored `light 180` lands at `0.6`. The canonical format is a modern linear `color × intensity` multiplier in 0–1+ range, so all downstream consumers (SH baker, direct light shader) treat intensity as a straight linear factor with no further scaling. Quake-specific authoring conventions stop at the translator boundary.
- `_fade` is authored in map units (Quake units), consistent with all spatial coordinates in `.map` files. The translator converts to engine meters by multiplying by 0.0254 (1 map unit = 1 inch). `falloff_range` in `MapLight` is always engine meters. Guideline: `_fade ≈ light × 200` in map units (e.g., `light 300` → `_fade 60000` ≈ 1,524 m); scale down for tight indoor maps, leave large for outdoor or vista-scale geometry.
- Spotlight direction via `mangle` (pitch yaw roll in degrees, engine space) only. `target` entity resolution is deferred to Milestone 6 — the entity system is needed to look up entity origins by name. If `target` is present, emit an error directing the mapper to use `mangle`.
- Cone degrees → radians conversion happens at the translation boundary. Canonical format is radians-only.
- `style` 0–11 map to `LightAnimation` brightness curves. The Quake translator owns the preset table — classic brightness strings where each character `a`–`z` maps to 0.0–1.0 (26 levels), sampled at 10 Hz in the original games. The translator converts each string to a `Vec<f32>` brightness curve; period = `string_length × 0.1s`. Style 0 = constant (no animation, `animation: None`). Styles 12+ reserved for future use. Future format translators (UDMF, etc.) expand their own presets into the same `LightAnimation` curves — the map light format never sees a preset name.
- `_phase` is a 0.0–1.0 offset within the animation cycle, allowing mappers to desync lights that share the same `style`. Two torches with `style 3` and different `_phase` values flicker independently. Default 0.0 (all lights of the same style sync). Ignored when `style` is 0.
- Property name variation: accept both `light` and `_light` (Quake community naming variations across tools).

---

## Map light format

The compiler translates every supported map format into this format. The baker has no source-format awareness; it sees only `Vec<MapLight>`.

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

/// Curve-based animation over a repeating cycle.
/// Each channel is a Vec of samples distributed uniformly over the period.
/// Runtime linearly interpolates between adjacent samples at the current
/// cycle time. `None` channels hold constant for the cycle.
///
/// Format-agnostic — Quake light styles, Doom sector effects, UDMF curves,
/// or hand-authored data all translate into this shape. Translators own
/// their format's preset vocabulary and expand presets into sample curves.
pub struct LightAnimation {
    pub period: f32,                  // cycle duration in seconds
    pub phase: f32,                   // 0-1 offset within cycle (desync identical presets)
    pub brightness: Option<Vec<f32>>, // intensity multipliers, uniformly spaced over period
    pub color: Option<Vec<[f32; 3]>>, // linear RGB overrides, uniformly spaced over period
}

pub struct MapLight {
    // Spatial
    pub origin: DVec3,                    // position (engine space, meters)
    pub light_type: LightType,

    // Appearance
    pub intensity: f32,                   // linear brightness scalar (0–1+), format-normalized
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

    // Shadow casting
    pub cast_shadows: bool,              // default true; false lets transient gameplay lights (Milestone 6+) opt out
}
```

`MapData` gains `lights: Vec<MapLight>`.

### Design notes

- **`LightAnimation` as a curve primitive.** Follows the modern engine pattern (UE5 timelines, Unity AnimationCurves, Godot AnimationPlayer): sample arrays over a repeating period, linearly interpolated at runtime. Quake light styles are translator output, not map light vocabulary — each preset string expands to a brightness sample curve. Future format translators expand their own presets into the same shape. The map light format never sees a preset name, only curves.
- **`color` channel on `LightAnimation`.** Replaces the earlier `hue_shift` / `saturation` split. Direct RGB curves are simpler to interpolate, simpler to author, and avoid HSL ↔ RGB round-trips. Quake styles produce brightness-only animation (`color: None`); future formats or scripted lights can animate color directly.
- **Linear interpolation between samples.** Matches the simplest modern engine behavior and avoids curve-fitting complexity. If a chunkier retro feel is desired, the runtime can optionally snap to nearest sample — but the data model supports smooth interpolation by default.
- **Phase offset.** Quake's original system lacked phase offsets, causing all lights of the same style to flicker in sync. The `phase` field (0.0–1.0 cycle offset) is standard in modern engines and trivial to evaluate at runtime (`t = fract(time / period + phase)`).
- **Cone angles in radians.** The map light format is engine-internal; FGDs expose degrees to mappers and the translator converts.
- **`falloff_range`.** PBR-conventional naming.
- **`FalloffModel` enum retained despite PBR alignment.** PBR uses physical inverse-square only; the retro aesthetic needs linear and inverse-distance as well for authored looks. Aligning with PBR conventions is about *axes*, not *physics*.
- **`LightType::Directional` (not `Sun`).** Directional is a graphics primitive; "Sun" implies a specific use case. Does not require or imply global illumination — probe-based lighting samples directional lights the same way it samples point lights.
- **Intensity as a linear multiplier.** The canonical format treats `intensity` as a plain linear scalar applied directly to `color` — the same role an intensity/luminance scalar plays in any modern PBR light format. Format-specific authoring conventions (Quake's 0–300 radiosity-energy scale, Doom sector brightness, etc.) are format-specific and stop at the translator boundary. Sub-plan 2's SH baker and sub-plan 3's direct light shader both assume `intensity × color` is the final linear brightness — no downstream consumer applies a second scale.
- **`cast_shadows`.** All FGD-authored lights cast shadows by default (`cast_shadows: true`). No FGD key is needed — the flag exists so transient gameplay lights (Milestone 6+) can opt out programmatically. Sub-plan 4 activates shadow map evaluation against this field. `bake_only` and similar per-light routing flags remain deferred.

---

## Test map content

Light entities go into the generator scripts for `test-3.map` and `occlusion-test.map`, and `test-2.map` gets lights hand-authored directly (out of scope here — done separately).

### `assets/maps/gen_test_map_3.py` → `test-3.map`

Read the script to understand room layout and pick sensible origins. Add at least three light entities covering all three types (point, spot, directional):

- **Point light** — warm color, inverse-squared falloff, placed in a central room
- **Spotlight** — aimed down a corridor or into a doorway, exercises cone culling
- **Directional light** — cool color, represents ambient sky contribution

Each entity appended using the same inline string pattern the script already uses for `info_player_start`. Run `python3 assets/maps/gen_test_map_3.py` after editing to regenerate the `.map` file.

### `assets/maps/gen_occlusion_test_map.py` → `occlusion-test.map`

Read the script to understand the portal zone + arena layout and pick sensible origins. Add at least:

- **Directional light** — suitable for the open arena, steep downward `mangle`
- **Point light** — placed among the arena detail objects or in the portal corridor, exercises falloff in a tight space

Run `python3 assets/maps/gen_occlusion_test_map.py` after editing to regenerate the `.map` file.

### Shared requirements across both scripts

All light entities must include non-white `_color`, non-zero `delay` on at least one point or spot light, and all required properties per the validation table (no missing `_fade` on point/spot). The compiled `.prl` for each map must produce no errors and a non-empty `MapData::lights`.

---

## AlphaLights PRL section

> **Note: This is an interim format.** It will be replaced by a proper entity system in a future milestone. The section ID (18), layout, and name are intentionally marked as alpha to signal impermanence. Do not build stable consumers against this layout — treat it as scaffolding that will be torn out.

The compiler writes a new PRL section so the engine can load lights from the compiled map without needing access to raw `MapData`. This is the minimal wire format needed to unblock sub-plan 3 before the entity system exists.

### Section ID

**18 — AlphaLights.** IDs 12–17, 19, and 20 are taken (see `context/lib/build_pipeline.md` §PRL section IDs). ID 18 is the next available slot.

### Layout

Simple flat layout — no compression, no indirection:

```
AlphaLights section layout
───────────────────────────
u32   light_count          // number of MapLight records that follow

// Repeated light_count times:
f64   origin_x             // world position, engine meters (Y-up)
f64   origin_y
f64   origin_z
u8    light_type           // 0 = Point, 1 = Spot, 2 = Directional
f32   intensity            // unitless brightness scalar
f32   color_r              // linear RGB, 0–1
f32   color_g
f32   color_b
u8    falloff_model        // 0 = Linear, 1 = InverseDistance, 2 = InverseSquared
f32   falloff_range        // meters; meaningful for Point and Spot only
f32   cone_angle_inner     // radians; 0.0 if not Spot
f32   cone_angle_outer     // radians; 0.0 if not Spot
f32   cone_dir_x           // normalized aim vector; 0.0 if Point
f32   cone_dir_y
f32   cone_dir_z
u8    cast_shadows         // 1 = true, 0 = false
// Animation curves are NOT serialized — sub-plan 3 uses static base properties only.
// Animated light support is a Milestone 6+ follow-up (see sub-plan 3 Notes).
```

All fields are little-endian. The record is fixed-size (no variable-length fields), so the engine can index directly by record number. Animation data is omitted deliberately — the direct lighting path (sub-plan 3) uses static base properties only; animated indirect (SH) is sub-plan 7 and will use whatever the entity system provides.

The type name used in both the compiler and engine code is `MapLight`. The compiler builds `Vec<MapLight>` in `MapData::lights`; the serialized PRL record is a subset of that struct (animation curves are omitted for the direct lighting path).

### Compiler responsibility

Sub-plan 1 compiler task list gains:

8. After all lights are translated into `MapData::lights`, serialize them into the AlphaLights section (ID 18) during the pack step. Each `MapLight` maps one-to-one to a serialized record per the layout above.

### Engine responsibility

At level load, the engine parses the AlphaLights section, deserializes the flat record array into `Vec<MapLight>`, and passes it to the GPU upload path (sub-plan 3). If the section is absent (maps compiled before this milestone), the engine falls back to an empty light list with a warning.

---

## Acceptance criteria

- [ ] `assets/postretro.fgd` defines `light`, `light_spot`, `light_sun` with Quake-standard properties in TrenchBroom-compatible FGD syntax. FGD loads in TrenchBroom without errors; entity browser shows all three; SmartEdit renders color picker / dropdown / text field correctly.
- [ ] `postretro-level-compiler/src/format/quake_map.rs` implements `translate_light()` per the signature above. No shambler dependency. Owns Quake `style` → `LightAnimation` preset conversion. Converts cone angles degrees → radians at the translation boundary.
- [ ] `parse.rs` recognizes `light`, `light_spot`, `light_sun`, extracts properties into `HashMap<String, String>`, calls the translator, populates `MapData::lights`. Errors block compilation; warnings log.
- [ ] Light types (`LightType`, `FalloffModel`, `LightAnimation`, `MapLight`) defined in `map_data.rs`. `MapData` gains `lights: Vec<MapLight>`.
- [ ] Every row of the validation table is covered. Errors block; warnings log.
- [ ] `gen_test_map_3.py` emits point + spot + directional lights with non-white color and non-zero `delay` on at least one; `test-3.map` compiles without errors and produces a non-empty `MapData::lights`.
- [ ] `gen_occlusion_test_map.py` emits at least directional + point lights; `occlusion-test.map` compiles without errors and produces a non-empty `MapData::lights`.
- [ ] Both generator scripts are run after editing and the regenerated `.map` files are committed alongside the script changes.
- [ ] `context/lib/build_pipeline.md` §Custom FGD table includes rows for `light`, `light_spot`, `light_sun`.
- [ ] Unit tests for translator:
  - Valid point / spot (via mangle) / directional → canonical conversion.
  - Point/spot missing `_fade` → error.
  - Spot missing `mangle` → error.
  - Spot with `target` set → error (deferred to Milestone 6).
  - `mangle` with non-numeric values → error.
  - Multi-naming: both `light` and `_light` property names accepted.
  - `style` = 1 → `LightAnimation` with non-None brightness curve and correct period; `style` = 0 → `animation: None`.
  - `_phase` = 0.5 with `style` = 1 → `LightAnimation.phase` = 0.5.
  - `_phase` outside 0.0–1.0 → warning, clamped.
  - `_phase` set with `style` = 0 → warning, ignored.
- [ ] AlphaLights PRL section (ID 18) written by the compiler and readable by the engine: compiler serializes `MapData::lights` to the flat record layout above; engine parses the section at level load and produces a `Vec<MapLight>` with correct field values for every record.
- [ ] No other runtime engine changes in this sub-plan. Map lights available via `MapData::lights` for sub-plan 2's baker.
- [ ] `cargo test -p postretro-level-compiler` passes
- [ ] `cargo clippy -p postretro-level-compiler -- -D warnings` clean

---

## Implementation tasks

1. Create `assets/postretro.fgd` with `light`, `light_spot`, `light_sun` per the template above. Verify in TrenchBroom: copy to game config folder, open editor, confirm entity browser + SmartEdit property widgets behave without errors.

2. Create `postretro-level-compiler/src/format/mod.rs` and `format/quake_map.rs`. Implement `translate_light()` per signature. Include Quake style preset table and degrees-to-radians conversion.

3. Add light types (`LightType`, `FalloffModel`, `LightAnimation`, `MapLight`) to `postretro-level-compiler/src/map_data.rs`. Add `lights: Vec<MapLight>` field to `MapData`.

4. Extend `postretro-level-compiler/src/parse.rs`: recognize light classnames, extract property bag, dispatch to translator, propagate errors and warnings.

5. Add light entities to `assets/maps/gen_test_map_3.py` (point + spot + directional) and `assets/maps/gen_occlusion_test_map.py` (directional + point). Read each script to choose sensible origins. Run both scripts to regenerate the `.map` files.

6. Write translator unit tests covering every validation rule and the style preset conversion.

7. Update `context/lib/build_pipeline.md` §Custom FGD table.
