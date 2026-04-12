# Light Entities Across Game Engines

> **Purpose:** Document light entity design patterns across id Tech and Unreal engines to inform Postretro's canonical light format.
> **Scope:** Quake 2, Quake 3, Doom 3/idTech4, Unreal Tournament 2004. Covers spatial definition, falloff, animation, and baking philosophy.
> **Use this when:** designing light entity contracts, evaluating translation requirements, or researching falloff/animation strategies.

---

## Universal Properties (Across All Four Engines)

Every engine provides these primitives. Postretro canonical format should guarantee these:

| Property | Purpose | Range / Type |
|----------|---------|--------------|
| **Position (`origin`)** | Light location in 3D space | Vector3, engine coordinates |
| **Intensity** | Primary brightness control | 0–255+ (integer) or 0–1 (normalized float) |
| **Color** | RGB light color | Implicit white (Q2) or explicit RGB (Q3, D3, UT2004) |
| **Animation / Style** | Time-varying behavior (flicker, strobe, etc.) | Predefined enum or material-driven |
| **Switching / Targeting** | Enable/disable or aim at entity | Via `target` name or scripting |

Contract: Mapper specifies position, intensity, color, animation. Engine/compiler derives falloff, shadows, illumination.

---

## Key Divergence: Baking Philosophy

### **Quake 2 & 3: Offline Lightmap Baking**

- Compiler evaluates lights at **compile time**
- Generates precomputed **lightmaps** baked into BSP
- Lights become static (animation runs on already-baked data)
- Falloff is **mathematically fixed** (inverse-square law)
- Mapper controls intensity only; falloff derived from intensity value
- No runtime light updates possible

**Trade-off:** Fast runtime (lookup textures), slow iteration (recompile lightmaps).

### **Doom 3: Dynamic Per-Pixel Lighting**

- **No lightmaps.** Every surface evaluated per frame using light volumes
- Lights define 3D **cubic bounding boxes** (not spheres)
- Falloff is **texture-based LUT** (arbitrary curve, artist-driven)
- Lights are fully dynamic (move, spawn, delete, modify at runtime)
- Per-light toggles for shadows, specularity, diffuse
- Material system applies shaders to light behavior (flicker, color shift, projection)

**Trade-off:** Flexible per-pixel quality (any falloff curve), expensive runtime (evaluates every light per pixel).

### **Unreal Tournament 2004: Hybrid (Precomputed + Dynamic)**

- Supports both static **lightmaps** (precomputed) and **dynamic lights** (per-frame)
- Dynamic lights are separate class with full runtime recalculation
- Falloff is proprietary **black-box function** (scaled by `LightRadius` multiplier, not explicit)
- Animation is **tick-based** with phase control (frame-by-frame timing)
- Lighting channels allow selective illumination (light affects only chosen channels)
- Supports raytraced lighting for offline baking

**Trade-off:** Static geometry gets baked quality, moving objects get dynamic updates, can opt-in to expensive techniques.

---

## Falloff Models

### **Q2: Implicit Inverse-Square**

Light value (0–255) implicitly determines both brightness and falloff radius. Compiler derives distance via heuristic (typically 300 units = `light 300`). No mapper control.

### **Q3: Configurable, Mathematically Explicit**

| Property | Semantics |
|----------|-----------|
| `_light` | Wattage-like intensity (0–300+) |
| `_fade` | Explicit distance where light reaches zero brightness |
| `_wait` | Fade distance scale multiplier (>1 = fade faster) |
| `delay` | Falloff enum: 0=linear, 1=inverse-distance (1/r), 2=inverse-squared (1/r²) |
| `_deviance` | Sphere radius for soft shadow sampling |

**Advantage:** Mapper has explicit control. Compiler can evaluate analytically at any distance.

### **Doom 3: Texture-Based LUT**

Falloff is encoded in light **material texture** (2D curve: x-axis = distance, y-axis = brightness). Artist-designed, fully customizable, but opaque to baking algorithms (no analytical evaluation).

### **UT2004: Proprietary Function + Scale**

`LightRadius` is a multiplier for undisclosed falloff function. No explicit distance or formula documented. Opacity limits baking flexibility.

---

## Animation / Time-Varying Behavior

### **Q2 & Q3: Predefined Styles (Enum)**

```
0: Normal (no animation)
1: Flicker (rapid random flicker)
2: Slow Pulse (gentle pulse in/out)
3: Candle (candle-like flicker)
4: Fast Strobe (high-frequency strobe)
5: Gentle Pulse
6–8: Candle variations
9: Slow Strobe
10: Fluorescent Flicker
11: Slow Pulse (no fade to black)
12–31: Reserved
32–62: Compiler-assigned (switchable lights)
63: Test pattern
```

Limited artistic control. Mapper picks one of ~12 predefined behaviors.

### **Doom 3: Material Shader-Driven**

Animation is programmed via material/shader graph. Arbitrary flicker patterns, color shifts, intensity pulses. Fully customizable, but requires material authoring (not accessible to level designers in traditional sense).

### **UT2004: Rich Enum + Tick-Based**

```
LT_Pulse, LT_Strobe, LT_BackdropLight, LT_Steady, 
LT_FlickerA, LT_FlickerB, LT_Modulate, LT_FireFlicker, LT_None
```

Plus `LightPeriod` (cycle time in seconds) and `LightPhase` (0–1 offset within cycle). More control than Q2/Q3; still enum-based (not fully customizable).

---

## Spotlight / Directional Light Design

### **Q2: None**

No native spotlight support in entity definitions.

### **Q3: Target-Based Aiming + Cone Angle**

```
_cone (degrees)      // Inner (full-bright) cone angle
_cone2 (degrees)     // Outer (fade) cone angle
target (entity name) // Entity to aim at (alt: mangle for direction)
```

Spotlight is defined by source position, target position, and cone angles. Direction computed from target or explicit mangle vector.

### **Doom 3: Full Projection Geometry**

```
light_target (vector)  // Aim direction
light_up (vector)      // Up direction for projection
light_right (vector)   // Right direction for projection
light_start (vector)   // Near plane of projection
light_end (vector)     // Far plane of projection
```

Defines a complete frustum projection volume. Enables texture projection (spotlight projected texture).

### **UT2004: Simple Cone**

```
ConeAngle (float)      // Cone angle in degrees
Direction (vector)     // Spotlight aim direction (normalized)
```

Simpler than D3; similar to Q3 but with explicit normalized direction instead of target name.

---

## Per-Light Control Toggles

### **Q2: None**

Lights are simple; no per-light flags.

### **Q3: Soft Shadow Control**

`_deviance` (float): Sphere radius for stochastic point-light sampling. Creates soft shadow penumbra.

### **Doom 3: Extensive Per-Light Toggles**

```
noshadows (bool)      // Don't cast shadows
nodiffuse (bool)      // Skip diffuse contribution
nospecular (bool)     // Skip specular contribution
noSelfShadow (bool)   // Object doesn't shadow itself from this light
```

Fine-grained control over which light components contribute.

### **UT2004: Channel-Based Selective Illumination**

`LightingChannels` (bitmask): Light affects only surfaces/actors in matching channels. Enables selective illumination without per-light toggles.

---

## Comparison Matrix

| Aspect | Q2 | Q3 | D3 | UT2004 |
|--------|----|----|----|----|
| **Baking Model** | Compile-time lightmaps | Compile-time lightmaps | Runtime per-pixel | Hybrid (static + dynamic) |
| **Falloff Definition** | Implicit (heuristic) | Explicit `_fade` distance | Texture LUT (opaque) | Proprietary function + scale |
| **Falloff Control** | None | `_wait`, `_fade`, `delay` | Arbitrary (material) | `LightRadius` multiplier |
| **Color** | Implicit white | Explicit `_color` RGB | Explicit `_color` RGB | Explicit RGB (`LightColor`) |
| **Animation Styles** | 12 predefined | 12 predefined | Material-driven (custom) | 9 predefined + period/phase |
| **Spotlight** | None | Target + `_cone` | Projection geometry | Simple cone angle |
| **Per-Light Shadow Control** | No | `_deviance` (soft) | `noshadows`, `nodiffuse`, `nospecular` | Lighting channels |
| **Runtime Mutability** | Static (baked) | Static (baked) | Fully dynamic | Static (baked) + dynamic class |
| **Volume Shape** | Spherical (implicit) | Spherical (implicit) | Cubic (explicit bounds) | Spherical (implicit) |

---

## Design Patterns Postretro Inherits

### **1. Mathematical Explicitness Over Opacity**

Q3's approach (`_fade`, `delay` enum) is more bakeable than D3's texture LUT. Analytical evaluation at probe points requires knowing exactly how distance → brightness.

**For Postretro probes:** Use explicit falloff distance and enum, not texture-based LUTs.

### **2. Animation as Enum + Timing Parameters**

All engines use predefined animation styles. Q3/Q2 style enums are simpler and more portable than D3's material system. UT2004 adds `period` and `phase` for frame-level control.

**For Postretro:** Precompute animation phases and bake into probe grid (probe at t=0, t=0.1, t=0.2, ..., t=1.0 for full animation cycle). No runtime animation in Phase 4.

### **3. Per-Light Shadow Toggles**

D3's approach (per-light `noshadows`, `nodiffuse`, `nospecular`) is cleaner than channel-based (UT2004) for fine control. Enables neon fill lights, ambient lights, etc.

**For Postretro:** Support `cast_shadows` bool per light so non-shadowing lights (neon, ambient) don't pollute occlusion.

### **4. Cone Angle for Spotlights (Not Projection Geometry)**

Q3's cone approach scales better than D3's full projection geometry (which is power-user territory). Simpler for mappers.

**For Postretro:** Inner/outer cone angles (Q3 style) suffice for Phase 4.

### **5. Compile-Time Baking > Runtime Dynamics (Phase 4)**

Q2/Q3 show that offline baking is fast iteration for static levels. D3's runtime per-pixel is expensive. UT2004's hybrid gives both.

**For Postretro Phase 4:** Bake into probes at compile time. Phase 5 adds dynamic lights as separate entity type.

---

## Non-Goals (What We Don't Adopt)

- **Texture projection** (D3): Out of scope for Phase 4 validation experiment.
- **Lighting channels** (UT2004): Over-complex for Phase 4; simple per-light flags suffice.
- **Material-based falloff** (D3): Opaque for baking; explicit distance is cleaner.
- **Implicit falloff heuristics** (Q2): Explicit distance better for deterministic probes.
- **Compile-time style reassignment** (Q2 switchable lights): Use absolute style indices or tags instead.
