# Phase 4 — FGD Light Entities

> **Status:** ready for implementation — all design decisions finalized and research-backed.
> **Phase:** 4 (Light Probes) — see `context/plans/roadmap.md`
> **Related:** `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `REVIEW.md` (full analysis and rationale)

---

## Goal

Define the Postretro light entity set in the custom FGD so mappers can place light sources in TrenchBroom. `prl-build` reads the entities out of `.map` files and exposes them as a typed in-memory list for downstream compiler stages to consume. Without this plan, the probe baker has nothing to bake from.

---

## Context

The current FGD defines `env_fog_volume`, `env_cubemap`, and `env_reverb_zone`. No `light`, `light_spot`, or `light_sun` entity exists — the project has been runnable without one because Phase 3 shipped flat uniform lighting with no baked light sources.

Phase 4 changes that. The probe baker evaluates lighting per probe sample point, which requires a set of light sources in the map. Light entity definitions live on the mapper side, so they land in the FGD and are parsed at compile time through the existing shambler pipeline. TrenchBroom already understands standard FGD light entity shapes; the work is deciding which shapes Postretro supports and which properties each carries.

ericw-tools and dmap are the reference points for entity shape. Postretro is free to diverge where the retro aesthetic or engine constraints warrant.

---

## Scope

### In scope

- **FGD file:** Create `assets/postretro.fgd` defining `light`, `light_spot`, `light_sun` with TrenchBroom-compatible metadata (color, size, properties in SmartEdit format).
- **Property definitions:** intensity (`light`), color (`_color`), falloff model (`delay`), cone angles (`_cone`, `_cone2` for spotlights), animation style (`style`).
- **Compiler-side parsing:** `prl-build` recognizes light classnames during `.map` parse and produces a typed `ParsedLight` list available to downstream stages (baker in Phase 4, engine in Phase 5). No downstream consumption in Phase 4 — parser output stored in `MapData::lights`.
- **Documentation:** Update `context/lib/build_pipeline.md` §Custom FGD table to add the three new light entity rows.
- **Test coverage:** Extend `assets/maps/test.map` with at least one instance of each entity type (`light`, `light_spot`, `light_sun`), plus variations (colored lights, different falloff modes) to exercise parser.

### Design Decisions & Review Feedback Resolution

**Critical feedback from code review (all addressed):**

1. **Falloff heuristic was vague** → **Fixed:** `_fade` is now **required** in FGD (error if omitted). Mapper guideline: `_fade = light × 200`. Ensures deterministic baker behavior.

2. **Spotlight direction edge cases unspecified** → **Fixed:** Explicit validation rules added to translator (errors for missing mangle/target, invalid mangle format, nonexistent target). Prevents silent failures.

3. **Animation baking cost not quantified** → **Fixed:** Strategy documented (11 samples per cycle, ~220 KB for typical maps). Phase 4.5 baker plan will refine.

4. **`bake_only` flag was premature** → **Fixed:** Removed from Phase 4. Phase 5 designs separate `light_dynamic` entity if runtime lights needed.

5. **Material/emission control too basic** → **Addressed:** `cast_shadows` bool is sufficient for Phase 4. Phase 5 can extend with diffuse_only, specular_only, etc.

6. **FGD color picker untested** → **Added to acceptance criteria:** TrenchBroom rendering verification required.

7. **Translator test coverage gap** → **Added:** Multi-FGD variant tests ensure future ericw-tools support.

**Editor strategy (confirmed viable, see context/reference/light-entities-across-engines.md):**
- FGD + game config folder = complete distribution (no plugins needed).
- Mappers install once, map immediately in TrenchBroom.
- Extensible for future formats (ericw-tools, Doom 3) via additional translators.

### Out of scope

- Probe baking itself. This plan produces light sources; the baker plan consumes them.
- Runtime rendering of lights as dynamic sources. Phase 4 is baked-only. Dynamic light entities are Phase 5.
- Light styles and animation at runtime (flickering neon, pulsing signs). If the FGD carries a style field, it is stored in the parsed representation but not evaluated yet.
- `env_projector` and texture-projecting lights. Out of scope for the validation experiment.
- IES profiles or real-world photometric data. The retro aesthetic doesn't need them.
- Area lights (rectangle, disk). Point / spot / sun cover the decision-gate evaluation.

---

## Finalized Design: Canonical Light Format

All decisions research-backed via comparative analysis of Q2, Q3, Doom 3, UT2004. See `context/reference/light-entities-across-engines.md` for engine details. See REVIEW.md for phase decisions and FGD mapping.

### Canonical Light Representation

The compiler translates all mapper inputs (Quake conventions, future ericw-tools, Doom 3, etc.) to this canonical format. Baker and engine receive normalized data.

```rust
// Proposed design — Phase 4

pub enum FalloffModel {
    Linear,              // brightness = 1 - (distance / falloff_distance)
    InverseDistance,     // brightness = 1 / distance, clamped at falloff_distance
    InverseSquared,      // brightness = 1 / (distance²), clamped at falloff_distance
}

pub enum AnimationStyle {
    None,
    Flicker,        // rapid random (Q2/Q3 style 1)
    SlowPulse,      // gentle pulse (Q2/Q3 style 2)
    Candle,         // candle flicker (Q2/Q3 style 8)
    FastStrobe,     // high-frequency (Q2/Q3 style 4)
    // ... others as needed, matching Quake enum for mapper familiarity
}

pub enum LightType {
    Point,          // omnidirectional
    Spot,           // cone, uses cone_angle_inner/outer + direction
    Sun,            // parallel directional, ignores falloff_distance
}

pub struct CanonicalLight {
    // Spatial
    pub origin: DVec3,                    // position (engine space, meters)
    pub light_type: LightType,            // disambiguates point/spot/sun
    
    // Appearance
    pub intensity: f32,                   // brightness scalar. Phase 4: unitless (0–300 typical). 
                                          // Phase 4.5 baker: intensity unit TBD (lumens? candelas? relative?); 
                                          // refined after probe visuals; may require normalization
    pub color: [f32; 3],                  // linear RGB, 0–1 range (converted from mapper 0–255)
    
    // Falloff (critical for probe baker)
    pub falloff_model: FalloffModel,      // mathematical model: linear, 1/r, 1/r²
    pub falloff_distance: f32,            // explicit distance where light reaches zero (Q3's `_fade` approach).
                                          // **Required:** must be > 0. Baker evaluates light analytically up to this distance.
    
    // Spotlight parameters (point: none, spot: inner+outer+dir, sun: dir only)
    pub cone_angle_inner: Option<f32>,    // inner cone in degrees (full brightness). Spotlights only.
    pub cone_angle_outer: Option<f32>,    // outer cone in degrees (fade edge). Spotlights only.
    pub cone_direction: Option<[f32; 3]>, // spotlight aim direction (normalized vector). Spotlights only.
                                          // Sun lights ignore this; use direction from entity angles if needed.
    
    // Animation / time-varying (baked into probe grid)
    pub animation_style: AnimationStyle,  // enum: None, Flicker, SlowPulse, Candle, etc.
    pub animation_period: f32,            // cycle time in seconds (e.g., 0.4 for candle). Ignored if style == None.
    pub animation_phase: f32,             // phase offset 0–1 within cycle (0 = start, 1 = next cycle). 
                                          // Baker bakes at discrete time samples (t=0, 0.1, 0.2, ..., 1.0) per animation style.
}
```

**Design rationale:**
- **`falloff_model` enum + explicit `falloff_distance`:** Q3's mathematically explicit approach. Probes can evaluate analytically at any point without texture lookups.
- **`color` as linear RGB 0–1:** Normalized, predictable for probe math.
- **`animation_style` enum + `period`/`phase`:** Quake community convention; mappers recognize the style names.
- **`cone_angle_inner/outer`:** Q3/UT2004 pattern. Soft spotlight edges without full projection geometry (D3).
- **`cast_shadows` flag:** Per-light control (D3 approach). Allows neon ambient lights to not occlude.
- **`bake_only` flag:** Phase 4/5 boundary marker. Phase 4 bakes; Phase 5 may re-spawn as dynamic entity if `bake_only=false`.

### Mapper-Facing FGD (Quake Conventions)

Mappers author using familiar Quake syntax. Compiler translates to canonical format via `MapTranslation::light_entity()`.

| FGD Property | Type | Maps to Canonical | Default / Required |
|--------------|------|-------------------|-----|
| `light` | integer | `intensity` | 300 |
| `_color` | color255 (0–255 RGB) | `color` (normalized to 0–1) | 255 255 255 |
| `_fade` | integer (distance in map units) | `falloff_distance` | **Required** (no default; error if missing) |
| `delay` | choices | `falloff_model` (0=Linear, 1=InverseDistance, 2=InverseSquared) | 0 (Linear) |
| `_cone` | integer (degrees) | `cone_angle_inner` | 30 (spotlight only; ignored for point/sun) |
| `_cone2` | integer (degrees) | `cone_angle_outer` | 45 (spotlight only; ignored for point/sun) |
| `style` | integer (0–11) | `animation_style` (enum match) | 0 (None / no animation) |
| `mangle` or `target` | vector or string | `cone_direction` (spotlight only) | **Required for spotlights; error if missing** |

**Translator validation rules (errors block compilation; warnings log but proceed with defaults):**

| Case | Error / Warning | Handling |
|------|-----------------|----------|
| Point/spot light missing `_fade` | **Error** | Compilation fails. Mapper must specify falloff distance. |
| Spotlight missing `mangle` AND missing `target` | **Error** | Compilation fails. Mapper must aim spotlight (pitch/yaw/roll or target entity). |
| Spotlight with `target="nonexistent"` | **Error** | Compilation fails with message "target entity 'X' not found." |
| Light with `light` property = 0 | **Warning** | Intensity is zero; light contributes nothing. Use default 300 or remove entity. |
| Light missing `_color` | **Warning** | Defaults to white (255 255 255). |
| Spotlight missing `_cone` or `_cone2` | **Warning** | Defaults to 30° inner, 45° outer. |
| Light missing `style` | **Warning** | Defaults to 0 (no animation). |
| Spotlight with `_cone` > `_cone2` | **Warning** | Outer cone smaller than inner (should swap). Proceed with inner/outer as specified. |

**Translator notes:**
- `light` is unitless in Phase 4 (typical range 0–300, matching Q3). Phase 4.5 baker may normalize to physical units; see Phase 4 probe baker plan.
- `_fade` **required** for deterministic probe baking. Mapper must decide falloff distance explicitly (no heuristic guessing). Guideline: `_fade = light × 200` (e.g., `light 300` → `_fade 60000`); adjust based on map scale.
- Spotlight direction via `mangle` (pitch yaw roll in degrees, engine space) or `target` (entity name). If both provided, `target` takes precedence. If neither, error.
- `style` 0–11 map to `AnimationStyle` enum variants. Styles 12+ reserved (Phase 5 dynamic light reassignment).
- Animation evaluated per light. Probes bake animation samples across the style cycle (t=0, 0.1, ..., 1.0), allowing flickering surfaces without runtime evaluation.

---

## Acceptance Criteria

1. **FGD file created and verified:** `assets/postretro.fgd` defines `light`, `light_spot`, `light_sun` with Quake-standard properties in TrenchBroom-compatible FGD syntax.
   - FGD loads in TrenchBroom without errors.
   - Entity browser displays all three entity types with correct icons.
   - SmartEdit renders `_color` as color picker, `delay` as dropdown, `_fade` as text field.
   - No editor warnings or crashes.

2. **Translation layer:** `postretro-level-compiler/src/translate.rs` implements `MapTranslation` struct with:
   - `position()` method (coordinate swizzle + unit scale, covers both positions and directions).
   - `light_entity()` method (mapper Quake format → canonical `CanonicalLight`).
   - Explicit validation with error/warning handling per table above.
   - Centralized logic; no scattered coordinate transforms in parse.rs.

3. **Canonical data types:** `CanonicalLight` struct defined per specification above (no `bake_only` field).
   - `MapData` extended with `lights: Vec<CanonicalLight>`.
   - Falloff distance is always specified (not optional); baker can evaluate analytically.

4. **Compiler parser with validation:** `prl-build` recognizes `light`, `light_spot`, `light_sun`, extracts properties, validates, translates to canonical.
   - **Errors (block compilation):**
     - Missing `_fade` on any light.
     - Spotlight missing both `mangle` and `target`.
     - Spotlight with `target="nonexistent"`.
     - Invalid property format (e.g., non-numeric `_fade` or `mangle`).
   - **Warnings (logged, proceed with defaults):**
     - Missing `_color` → white.
     - Missing `_cone` or `_cone2` → 30°/45°.
     - Missing `style` → no animation.
     - Intensity = 0 → light contributes nothing.

5. **Test map coverage:** `assets/maps/test.map` includes:
   - Point light with valid `_fade` (exercise point light parsing).
   - Spotlight with `_cone`, `_cone2`, `mangle`, and `_fade` (exercise spotlight parsing and direction).
   - Sun light with `_fade` and optional `mangle` (exercise sun light parsing).
   - At least one light with non-white `_color` (exercise color parsing).
   - At least one light with `delay` ≠ 0 (exercise falloff model enum).
   - Map compiles without compilation errors.

6. **Documentation:** `context/lib/build_pipeline.md` §Custom FGD table includes rows for `light`, `light_spot`, `light_sun`.
   - Columns: Entity type, Purpose, Mapper-facing properties, Canonical fields, Consumption note.
   - Note: Phase 4 compiles and stores; Phase 4.5 baker consumes (animates probes).

7. **Unit tests for translator:** Test suite covers:
   - Valid point light → canonical conversion.
   - Valid spotlight with target → canonical conversion.
   - Valid spotlight with mangle → canonical conversion.
   - Invalid: light missing `_fade` → error.
   - Invalid: spotlight missing mangle and target → error.
   - Invalid: mangle with non-numeric values → error.
   - Multi-FGD variant: both `light` and `_light` property names accepted (future ericw-tools compat check).

8. **No runtime engine changes:** Engine, game logic, and renderer untouched. Canonical light list available via `MapData::lights` for Phase 4.5 baker. Phase 5 designs separate `light_dynamic` entity type if runtime lights are needed.

---

## Implementation Tasks

Detailed guidance in `REVIEW.md` (mapper→canonical mapping, FGD templates, test coverage). See `context/reference/light-entities-across-engines.md` for design precedent.

1. **Create `assets/postretro.fgd`** with `light`, `light_spot`, `light_sun` entity definitions and Quake FGD syntax.
   - Use `color255` FGD type for `_color` (verify TrenchBroom renders as color picker).
   - Add FGD comments explaining falloff models (Linear, InverseDistance, InverseSquared).
   - Set `_fade` as required (no default value in FGD; error if mapper omits it).

2. **Create translation layer** (`postretro-level-compiler/src/translate.rs`):
   - Centralize all mapper→engine transformations (coordinate swizzle, unit scale, entity properties).
   - Implement `MapTranslation` struct with methods for position, direction, light_entity, etc.
   - Implement `translate_quake_to_canonical()` with explicit validation rules (see validation table above).
   - Document falloff distance guideline: `_fade = light × 200` (e.g., `light 300` → `_fade 60000`).

3. **Extend data types** (`postretro-level-compiler/src/map_data.rs`):
   - Add `CanonicalLight` struct (as specified above, **no `bake_only` field**).
   - Add `lights: Vec<CanonicalLight>` field to `MapData`.

4. **Integrate parser** (`postretro-level-compiler/src/parse.rs`):
   - Extract light entity properties (classname, origin, light, _color, delay, _fade, _cone, _cone2, style, mangle/target) from `.map` file.
   - Call `MapTranslation::light_entity()` with validation (errors block compilation; warnings log but proceed).
   - Populate `MapData::lights`.

5. **Extend test map** (`assets/maps/test.map`):
   - Point light: existing `light` entity with `_fade` specified (verify parsing).
   - Spotlight: `light_spot` with `_cone`, `_cone2`, `mangle` or `target`, and `_fade`.
   - Sun light: `light_sun` with `_fade` and optional `mangle` for direction.
   - Variation: at least one light with non-white `_color` and non-zero `delay` (test falloff/color parsing).
   - Verify map compiles without errors.

6. **Animation baking strategy** (for Phase 4.5 baker plan; document here):
   - For each light with non-None animation style: baker samples at t=0, 0.1, 0.2, ..., 1.0 (11 samples per light cycle).
   - Stores 11 brightness multipliers per animated light in the probe grid (memory cost: ~4 bytes per sample per probe per light).
   - Estimate: 1000 probes × 5 animated lights × 11 samples × 4 bytes = 220 KB overhead (acceptable).
   - Phase 4.5 baker plan refines this strategy (may reduce samples or use compression).

7. **Update documentation** (`context/lib/build_pipeline.md`):
   - Add `light`, `light_spot`, `light_sun` rows to Custom FGD table.
   - Note: FGD entity, mapper-facing properties (Quake conventions), translation to canonical format, Phase 4 consumption (baker in Phase 4.5).

8. **TrenchBroom integration verification:**
   - Copy `assets/postretro.fgd` to TrenchBroom game config folder (e.g., `~/.TrenchBroom/games/postretro/`).
   - Open TrenchBroom, select Postretro game.
   - Verify: entity browser lists `light`, `light_spot`, `light_sun`.
   - Place each entity in editor; verify viewport shows editor icons.
   - Edit properties: color picker appears for `_color`, dropdown for `delay`, text field for `_fade`.
   - No editor crashes, errors, or warnings.

9. **Translator test coverage:**
   - **Multi-FGD variant test:** Write unit test verifying translator accepts both `light` and `_light` property names (prepares for future ericw-tools compatibility).
   - **Validation tests:**
     - Point light missing `_fade` → error message "missing _fade"
     - Spotlight missing `mangle` and `target` → error message "missing mangle or target"
     - Invalid `mangle` (non-numeric) → error message "invalid mangle format"
     - Light with zero intensity → warning "intensity is zero"
