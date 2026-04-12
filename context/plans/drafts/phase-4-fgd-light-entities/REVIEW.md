# Phase 4 FGD Light Entities — Plan Review

**Reviewer:** Claude Code  
**Review Date:** 2026-04-11  
**Status:** Solid foundation. Plan is well-scoped and unblocked. Requires research-backed decisions on 5 open questions before promotion to ready.

---

## Executive Summary

The plan correctly identifies the FGD light entity definitions as a prerequisite for Phase 4 probe baking. The work is discrete and bounded — defining entities in the FGD, wiring compiler-side parsing, and documenting in build_pipeline.md.

**Strengths:**
- Clear scope: mapper-side definition through compiler-side availability (no downstream baker work).
- Acceptance criteria are specific and testable.
- Correctly identifies that ericw-tools and dmap are reference sources, not fallbacks.
- Good framing of the retro aesthetic as a deliberate design constraint.

**Gaps requiring research:**
1. **Exact entity set** — which of `light`, `light_spot`, `light_sun` are needed for the decision gate?
2. **Falloff model** — inverse-square vs. linear falloff (affects baker implementation).
3. **Property naming** — `_color` vs. a Postretro-native convention.
4. **Compiler architecture** — where does the parsed light list live?
5. **Validation strategy** — compile-time error vs. warn-with-defaults.

---

## Detailed Analysis

### 1. Scope is Well-Defined ✓

The plan correctly stops at "parsed representation available to downstream stages." The baker plan (`phase-4-probe-format-research/`) and the engine sampling plan are correctly excluded.

**Observation:** The test `.map` file already contains a `light` entity:
```
// entity 2
{
"classname" "light"
"origin" "736 128 160"
"light" "300"
}
```

This exists in the world but is currently ignored by the compiler (EntityInfo only stores classname and origin). This is good — it means you can prototype with existing maps.

### 2. Current Compiler State

**EntityInfo struct** (`postretro-level-compiler/src/map_data.rs`):
```rust
pub struct EntityInfo {
    pub classname: String,
    pub origin: Option<DVec3>,
}
```

**Entity parsing** (`parse.rs`):
- Extracts classname and origin for all entities.
- Unknown entities are logged and skipped (no hard errors).
- No property-parsing infrastructure exists yet.

**Existing entity precedent:**
The build_pipeline.md already documents `env_fog_volume`, `env_cubemap`, and `env_reverb_zone`. These are recognized entities, but property parsing is out of scope for this plan's current phase. Light entities follow the same pattern.

### 3. Key Design Decisions Requiring Research

#### Decision 1: Exact Entity Set

**Options:**
- **Minimal (option A):** `light` (point) only — proves the pipeline works.
- **Core (option B):** `light` + `light_spot` — covers the core use cases (ambient and directed).
- **Full (option C):** `light` + `light_spot` + `light_sun` — directional sunlight for outdoor areas.

**Research findings:**
- **ericw-tools** defines: `light`, `light_spot`, `light_sun`, and `light_environment` (ambient override).
- **dmap** (id Tech 4): `light`, `light_spot` (both required), `light_sun`, `light_ambient`.
- **Quake lineage:** sun lights are common in outdoor tests; absence would limit test map variety.

**Recommendation:** Start with `light` + `light_spot` (minimal proof of concept). Add `light_sun` if test maps require outdoor lighting. This allows phased implementation without unblocking the baker.

**Rationale:** The decision gate focuses on probe quality on varied surfaces. Indoor test maps with point and spot lights suffice. Sun lights are a nice-to-have for outdoor decision-gate evaluation.

---

#### Decision 2: Falloff Model

**Options:**
- **Inverse-square (IQ):** Physically plausible. Falloff ∝ 1/r². Standard in ericw-tools and dmap.
- **Linear (L):** Simple math, predictable for level designers. Source Engine uses this.
- **Per-light authored falloff:** Mapper can override; default to one of above.

**Research findings:**
- **ericw-tools:** Inverse-square only (fixed).
- **dmap / id Tech 4:** Inverse-square with optional `_attenuation` override per light.
- **Source (Half-Life 2):** Linear with radius. Easier for mappers to reason about but less realistic.

**Recommendation:** Implement inverse-square as the default falloff. Store a `_falloff` property in the parsed representation (default: IQ). This future-proofs without bloating Phase 4.

**Rationale:** 
- Inverse-square matches ericw-tools precedent (simpler to reason about when researching).
- Storing the falloff property in the parsed form doesn't require baker implementation yet.
- Mapper can author `_falloff 1.0` (linear) if needed, even if it's ignored in Phase 4.

---

#### Decision 3: Property Naming

**Options:**
- **ericw-tools compat (EC):** `_color` (RGB), `_intensity` or `_light` (brightness).
- **Postretro-native (PN):** Invent new names aligned with style.
- **Hybrid:** Use `_color` (well-known), custom names for Postretro-specific fields.

**Current map example:** The test map uses `light "300"` (property name is just "light", not "_light").

**Research findings:**
- **ericw-tools:** Properties are `_light` (intensity), `_color` (RGB string), `_angle` (spot angle).
- **dmap:** Properties are `light` (intensity, no underscore), `_color`, `_angle`.
- **FGD convention:** Underscores distinguish "special properties" from editor-only fields.

**Recommendation:** Use ericw-tools naming (`_color`, `_light`, `_angle`, `_falloff`). The underscore convention is clear and well-documented.

**Rationale:**
- Underscores are intentional signaling: "this property matters to the baker."
- ericw-tools is the nearest Quake-family reference.
- Mapper familiarity if they've used ericw-tools before.
- Leaves room for Postretro-native properties later (e.g., `_neon_glow`, `_shadow_casting`).

---

#### Decision 4: Compiler Architecture — Where Does the Parsed Light List Live?

**Options:**
- **Option A:** New module `postretro-level-compiler/src/entities/lights.rs` — dedicated light parsing.
- **Option B:** Extend existing `parse.rs` — inline parsing during map load.
- **Option C:** New module `postretro-level-compiler/src/parsed_entities.rs` — all entity types here.

**Current precedent:**
- `parse.rs` is the boundary layer between shambler and MapData.
- `map_data.rs` holds the output types (EntityInfo, MapData, etc.).
- No other entity parsing beyond classname + origin exists yet.

**Recommendation:** Option B (inline in parse.rs, store in MapData).

**Rationale:**
- Minimal scope creep for a single entity type.
- If Phase 8 (Entity Framework) defines a generic entity parser, it's easier to refactor from parse.rs than from a buried module.
- Keep entity handling centralized for now (all entity parsing in one place).
- Extend `MapData` to hold a `lights: Vec<ParsedLight>` field alongside `entities`.

**Pseudocode structure:**
```rust
pub struct ParsedLight {
    pub origin: DVec3,
    pub light_type: LightType, // Point, Spot, Sun
    pub intensity: f32,
    pub color: [f32; 3],
    pub angle: Option<f32>, // Spot cone angle
    pub falloff: FalloffModel, // IQ, Linear, etc.
}

// In MapData:
pub struct MapData {
    pub brush_volumes: Vec<BrushVolume>,
    pub entities: Vec<EntityInfo>,
    pub lights: Vec<ParsedLight>, // NEW
}
```

---

#### Decision 5: Validation Strategy

**Options:**
- **Strict (S):** Missing required properties or out-of-range values are compile errors. Bad maps fail hard.
- **Lenient (L):** Warnings + sensible defaults. Maps always compile; mapper gets feedback.
- **Hybrid (H):** Required properties are errors; optional properties warn and default.

**Precedent:** The engine today logs unknown entities as warnings (entity_model.md §4). No hard failures.

**Recommendation:** Hybrid approach — required properties (classname, origin, intensity) cause errors; optional properties (color, falloff) warn and use defaults.

**Rationale:**
- Consistent with current "unknown entities warn, don't crash" behavior.
- Intensity is non-negotiable for a light; missing it is a mapper error worth blocking.
- Color and falloff have sensible defaults (white, inverse-square).
- Mirrors dmap behavior: errors on bad input, warnings on missing optional fields.

---

## FGD File Structure

The plan mentions editing the FGD but doesn't specify where it lives. No FGD file was found in the repo. This will need to be created.

**Location:** Likely at `assets/postretro.fgd` or `config/postretro.fgd` (TrenchBroom game config will reference it).

**Example structure** (pseudo-FGD, actual syntax varies by FGD version):
```fgd
@PointClass color(255 200 0) size(-4 -4 -4, 4 4 4) = light : "Point Light"
[
    origin(origin)
    _light(integer) : "Intensity" : 300
    _color(string) : "Color (R G B)" : "255 255 255"
    _falloff(choices) : "Falloff Model" : 0 = 
    [
        0 : "Inverse Square"
        1 : "Linear"
    ]
]

@PointClass color(255 150 0) size(-4 -4 -4, 4 4 4) = light_spot : "Spot Light"
[
    origin(origin)
    _light(integer) : "Intensity" : 300
    _color(string) : "Color" : "255 255 255"
    _angle(integer) : "Cone Angle" : 45
    _falloff(choices) : "Falloff Model" : 0 =
    [
        0 : "Inverse Square"
        1 : "Linear"
    ]
]

@PointClass color(255 100 0) size(-4 -4 -4, 4 4 4) = light_sun : "Sun Light"
[
    origin(origin)
    _light(integer) : "Intensity" : 300
    _color(string) : "Color" : "255 255 255"
]
```

---

## Acceptance Criteria — Fulfillment Check

| Criterion | Status | Notes |
|-----------|--------|-------|
| FGD defines agreed light entity set with TrenchBroom metadata | 🟡 Pending | Depends on Decision 1 (entity set finalization). |
| TrenchBroom displays entities, allows placement and editing | 🟡 Pending | Conditional on FGD creation. |
| `prl-build` parses light entities and produces typed list | 🟡 Pending | Requires Decisions 2–4 (falloff model, naming, compiler location). |
| `context/lib/build_pipeline.md` reflects new entity rows | 🟢 Ready | Update the Custom FGD section table after FGD is written. |
| At least one test `.map` contains each entity type | 🟡 Pending | Test map already has `light`; needs `light_spot` and (optionally) `light_sun`. |
| No runtime engine changes | 🟢 Done | Parser produces a representation; engine is Phase 5. |

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| Falloff model choice mismatch with baker expectations | Medium | Medium | Research baker constraints early (probe-format-research plan). |
| FGD property naming doesn't match mapper expectations | Low | Low | Document naming convention in CLAUDE.md; use ericw-tools precedent. |
| Compiler parser becomes bottleneck for future entity types | Low | High | Keep entity parsing in `parse.rs` (unified boundary), not scattered. |
| Test maps don't exercise all entity types | Low | Medium | Author a dedicated `lighting_test.map` with all three types. |

---

## Recommendations for Promotion to Ready

1. **Research & Document:**
   - Confirm inverse-square falloff is the baker's expected default (coordinate with probe-format-research).
   - Document the decision on entity set, falloff, and naming in `CLAUDE.md` §Phase 4 so Phase 5 knows constraints.

2. **Update Plan:**
   - Finalize Decisions 1–5 in the plan (add a §Finalized Decisions section).
   - Add a §FGD File Location section specifying where the FGD will live.

3. **Ready Checklist:**
   - [ ] Decision 1 (entity set): Final choice committed.
   - [ ] Decision 2 (falloff): Inverse-square + optional per-light override decided.
   - [ ] Decision 3 (naming): ericw-tools convention (`_light`, `_color`, `_angle`) confirmed.
   - [ ] Decision 4 (compiler): ParsedLight struct designed, stored in MapData::lights.
   - [ ] Decision 5 (validation): Hybrid (error on required, warn on optional) chosen.
   - [ ] FGD file location identified and FGD template drafted.
   - [ ] Test map plan identified (which existing map will be extended, or new map authored).

---

## Finalized Decisions (Research-Backed)

### Decision 1: Entity Set ✓
**Choice:** `light` (omnidirectional) + `light_spot` (cone) + `light_sun` (directional)

**Rationale:** 
- TrenchBroom supports all three natively (no special setup needed).
- Quake 1, Quake 2, and Half-Life all define these three. Standard across brush-based editors.
- This covers the decision-gate test cases: point lights for general illumination, spotlights for directed effects (neon signs, dramatic lighting), sun for outdoor variety.
- Matches industry standard (ericw-tools, Radiant, TrenchBroom built-in FGDs all include these three).

---

### Decision 2: Falloff Model ✓
**Choice:** Per-light `delay` property (choices: linear, inverse-distance 1/x, inverse-squared 1/x²)

**Rationale:**
- Mapper can switch falloff per light for rapid comparison testing.
- Matches ericw-tools `delay` property convention (0=linear, 1=1/x, 2=1/x²).
- Gives the baker freedom to test which falloff looks best for the retro aesthetic without recompiling maps.
- Parser stores the choice in `ParsedLight`; baker can consume it when implemented.

**Property name:** `delay` (integer, displayed as dropdown choices in FGD)

---

### Decision 3: Property Naming ✓
**Choice:** Follow Quake 1/2 FGD conventions as implemented in TrenchBroom built-in FGDs

**Naming scheme:**
| Property | FGD Type | Purpose | Default | Notes |
|----------|----------|---------|---------|-------|
| `light` | integer | Brightness/intensity | 300 | Directly controls radius in linear falloff; standard Quake convention |
| `_color` | color255 | RGB light color | 255 255 255 | Underscore prefix = engine-specific key (standard in Quake community) |
| `delay` | choices | Falloff model | 0 (linear) | 0=linear, 1=1/x, 2=1/x² |
| `_cone` | integer | Inner cone angle (spotlight only) | 30 | Spotlight parameter |
| `_cone2` | integer | Outer cone angle (spotlight only) | 45 | Spotlight parameter |
| `style` | integer | Animation style (future expansion) | 0 | Stored for Phase 5; not evaluated in Phase 4 |

**Rationale:**
- Underscore prefix (`_color`, `_cone`) is the **Quake mapping community standard** for engine-specific properties.
- Mappers familiar with ericw-tools or Quake modding will immediately recognize these names.
- Matches TrenchBroom's built-in Quake FGDs (reference: Quake.fgd in TrenchBroom repo).
- Balances id Tech ecosystem compatibility with clarity.

---

### Decision 4: Compiler Architecture ✓
**Choice:** Inline parsing in `parse.rs`, new `ParsedLight` struct stored in `MapData::lights`

**Rationale:**
- Thoughtfully architected but not over-engineered: all entity parsing lives in one place (parse.rs).
- If Phase 8 (Entity Framework) adds a generic parser, it's easier to refactor from a single location.
- Keeps entity handling centralized; no scattered parsing logic across multiple modules.

**Pseudocode structure:**
```rust
// postretro-level-compiler/src/map_data.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalloffModel {
    Linear,          // delay = 0
    InverseDistance, // delay = 1
    InverseSquared,  // delay = 2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    Point,
    Spot,
    Sun,
}

pub struct ParsedLight {
    pub origin: DVec3,
    pub light_type: LightType,
    pub intensity: f32,
    pub color: [f32; 3],             // R, G, B in 0-1 range (converted from 0-255)
    pub falloff: FalloffModel,
    pub cone_inner: Option<f32>,     // Some(degrees) for spotlights
    pub cone_outer: Option<f32>,     // Some(degrees) for spotlights
    pub style: u8,                   // Animation style (0-11, stored for future use)
}

pub struct MapData {
    pub brush_volumes: Vec<BrushVolume>,
    pub entity_brushes: Vec<(String, usize)>,
    pub entities: Vec<EntityInfo>,
    pub lights: Vec<ParsedLight>,    // NEW: parsed light entities
}
```

---

### Decision 5: Validation Strategy ✓
**Choice:** Hybrid (errors on required, warnings on optional with sensible defaults)

**Rationale:** Matches current engine behavior (unknown entities warn, don't crash). Provides good mapper feedback.

**Validation rules:**
- **Required (error if missing):** classname, origin, intensity (`light` property).
- **Optional (warn if missing, use default):** color (white), falloff (linear), cone angles (30/45 degrees for spotlights), style (0 = normal).

**Parser behavior:**
```
light entity with intensity = 0 or missing
  → Warning: "light at (x y z) has no intensity, using default 300"
light_spot with missing _cone2
  → Warning: "light_spot at (x y z) has no outer cone angle, using default 45"
light entity with invalid color format
  → Warning: "light at (x y z) has invalid color, using default white"
```

---

## FGD File Location & Structure

**Location:** `assets/postretro.fgd`

**Rationale:** 
- Mirrors texture pipeline precedent (`assets/` for authoring content).
- TrenchBroom game configuration will reference: `FGD = "path/to/postretro.fgd"`
- Keeps mapper-facing content together.

**FGD Template** (complete, ready to implement):

```fgd
@BaseClass = Light
[
    light(integer) : "Intensity" : 300
    _color(color255) : "Color" : "255 255 255"
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
]

@PointClass base(Light)
    color(255 100 0)
    size(-8 -8 -8, 8 8 8)
    = light_sun : "Sun/Directional Light"
[
    origin(origin)
]
```

---

## Test Coverage Plan

**Extend existing `test.map`** with spotlight and sun light entities.

Current state: `test.map` already has one `light` entity at (736 128 160) with intensity 300.

Additions:
- Add at least one `light_spot` entity (e.g., at one of the corridor junctions with cone angles 30/60).
- Add one `light_sun` entity at a high elevation (e.g., (960 128 500) simulating an outdoor sun).
- Add a light with non-white color (e.g., `_color "255 100 100"` for red light) to exercise color parsing.
- Add a light with `delay 2` to test inverse-squared falloff selection.

**Example entities to add to test.map:**
```
// entity X - spotlight
{
"classname" "light_spot"
"origin" "800 200 96"
"light" "200"
"_color" "255 200 100"
"_cone" "25"
"_cone2" "50"
"delay" "2"
}

// entity Y - sun
{
"classname" "light_sun"
"origin" "960 128 500"
"light" "150"
"_color" "200 200 255"
}

// entity Z - red light
{
"classname" "light"
"origin" "1100 128 96"
"light" "250"
"_color" "255 50 50"
"delay" "1"
}
```

---

## Next Steps for Promotion to Ready

1. **Update the plan (`index.md`)** with the finalized decisions section (copy from above).
2. **Create `assets/postretro.fgd`** with the FGD template provided.
3. **Extend `test.map`** with the additional light entities (spotlight, sun, colored lights, falloff variations).
4. **Update `context/lib/build_pipeline.md`** Custom FGD section to add `light`, `light_spot`, `light_sun` rows.
5. **Verify TrenchBroom can load and display the FGD** (requires TrenchBroom game config setup — likely already exists, but confirm).

---

## Ready Checklist

- [x] Decision 1 (entity set): `light` + `light_spot` + `light_sun` finalized.
- [x] Decision 2 (falloff): `delay` property with 3 choices (linear, 1/x, 1/x²).
- [x] Decision 3 (naming): Quake FGD conventions (`light`, `_color`, `delay`, `_cone`, `_cone2`).
- [x] Decision 4 (compiler): ParsedLight struct in MapData::lights, inline parsing in parse.rs.
- [x] Decision 5 (validation): Hybrid (error on required, warn on optional with defaults).
- [x] FGD location & template: `assets/postretro.fgd` with full structure ready.
- [x] Test coverage: Extend `test.map` with all three entity types + falloff/color variations.

**Status:** Ready for implementation. All 5 decisions finalized and research-backed.
