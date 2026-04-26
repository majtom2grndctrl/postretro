# Postretro — Level Design Reference

Levels are made in **TrenchBroom** and compiled to `.prl` files that the engine loads. You don't need to know Rust or touch the engine source — just author your map, run `prl-build`, and play.

---

## Getting Started

### Setting Up TrenchBroom

1. Open TrenchBroom and load the Postretro game definition: `assets/postretro.fgd`.
2. Set the texture path to the `assets/textures/` directory.
3. Author your map in Quake 1/2 `.map` format. Both Standard and Valve 220 UV projections work and can coexist in the same file.

### Compiling Your Map

```bash
cargo run -p postretro-level-compiler -- input.map -o output.prl
```

**Common options:**

| Flag | Default | What it does |
|------|---------|-------------|
| `-o <PATH>` | same as input, `.prl` extension | Where to write the compiled file |
| `--lightmap-density <METERS>` | `0.04` | Lightmap pixel size. Higher values = chunkier shadows, faster compile. Try `0.1` for drafts. |
| `--probe-spacing <METERS>` | `1.0` | How dense the indirect lighting probes are. `2.0` is fine for large open areas. |
| `-v`, `--verbose` | off | Prints each compilation step — useful when something goes wrong |

**Unit scale:** 1 map unit = 0.0254 m (one inch). A standard player-height room is roughly 72–80 units tall.

---

## Lights

### `light` — Point Light

Radiates light in all directions from a single point.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light` | integer | `300` | Brightness |
| `_color` | RGB | `255 255 255` | Light color |
| `_fade` | integer | **required** | How far the light reaches, in map units. The compiler will error if this is missing. |
| `delay` | 0/1/2 | `0` | Falloff shape: `0` = linear, `1` = inverse (1/x), `2` = inverse squared (1/x²) |
| `style` | integer | `0` | Preset flicker/pulse animation — see the style table below |
| `_phase` | float | `0.0` | Shifts the animation cycle (0.0–1.0). Set different values on nearby lights sharing the same style so they don't all pulse together. |
| `brightness_curve` | curve | — | Custom brightness animation — see Custom Animation Curves below |
| `color_curve` | curve | — | Custom color animation (only on `_bake_only 1` or `_dynamic 1` lights) |
| `direction_curve` | curve | — | Custom spotlight aim animation (spotlights only) |
| `period_ms` | float | — | Cycle length in milliseconds. Required when using any `*_curve` key. |
| `_curve_phase` | float | `0.0` | Phase offset for curve-animated lights (0.0–1.0). Works the same as `_phase` but for the curve path. |
| `_start_inactive` | 0/1 | `0` | `1` = the light starts dark at map load. Scripts can toggle it on at runtime. Only meaningful on animated lights. |
| `_dynamic` | 0/1 | `0` | `1` = dynamic light (runs at runtime, not baked). Use for lights that change during gameplay. |
| `_bake_only` | 0/1 | `0` | `1` = contributes to the baked lightmap and SH volume only; no runtime presence. Good for ambient fill that doesn't need to affect gameplay. |

**Static vs. dynamic:** By default, lights are baked into the lightmap and indirect lighting volume at compile time — they're essentially free at runtime and cast hard-pixel shadows, which looks great. Set `_dynamic 1` if you need a light to move, change intensity during play, or be spawned by a script.

---

### `light_spot` — Spotlight

A cone-shaped light that points in a specific direction. Inherits all keys from `light`, plus:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `_cone` | integer | `30` | Inner cone angle in degrees — the full-brightness region |
| `_cone2` | integer | `45` | Outer cone angle in degrees — the edge where brightness fades to zero |
| `mangle` | string | — | Direction as `"pitch yaw roll"` (e.g. `"-90 0 0"` points straight down) |

---

### `light_sun` — Directional Light

A single directional light that hits everything from the same angle, like sunlight or a distant area light. Its position in the map doesn't matter — only direction.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mangle` | string | `"-90 0 0"` | Direction as `"pitch yaw roll"` in degrees |

All shared `light` keys (`light`, `_color`, `_fade`, `delay`, `style`, `_phase`, `brightness_curve`, `color_curve`, `direction_curve`, `period_ms`, `_curve_phase`, `_start_inactive`, `_dynamic`, `_bake_only`) apply.

---

### Animated Lights

#### Preset styles

The `style` key picks a named flicker or pulse animation from a built-in table. These come straight from classic Quake and feel great for torches, strobes, and fluorescent lights.

| `style` | Name | Cycle length |
|---------|------|-------------|
| 0 | Constant (no animation) | — |
| 1 | Flicker | 2.3 s |
| 2 | Slow strong pulse | 5.0 s |
| 3 | Candle (first variety) | 3.3 s |
| 4 | Fast strobe | 1.2 s |
| 5 | Gentle pulse | 3.3 s |
| 6 | Flicker (second variety) | 1.7 s |
| 7 | Candle (second variety) | 2.7 s |
| 8 | Candle (third variety) | 4.1 s |
| 9 | Slow strobe | 1.6 s |
| 10 | Fluorescent flicker | 2.4 s |
| 11 | Slow pulse, no black | 3.7 s |

Use `_phase` to desync lights that share the same style. For example, two `style 1` flicker lights with `_phase 0.0` and `_phase 0.5` will be out of sync by half a cycle.

#### Custom animation curves

If the presets aren't enough, you can author your own timing using keyframes. Each keyframe is `time_ms:value` separated by spaces. The compiler resamples them into a smooth curve using Catmull-Rom interpolation.

**Example — a 1-second fade in/out:**
```
brightness_curve "0:0.1 500:1.0 1000:0.1"
period_ms "1000"
```

**Example — color shift (bake_only or dynamic lights only):**
```
color_curve "0:255 0 0  500:255 255 255  1000:255 0 0"
period_ms "1000"
```

Use `_curve_phase` to offset curve-animated lights from each other, same as `_phase` works for style presets.

If you set both `brightness_curve` and `style`, the curve wins and `style` is ignored (you'll see a warning in the compiler output).

---

## Textures

Textures are PNG files under `assets/textures/<collection>/<name>.png`. TrenchBroom requires this one-level subdirectory structure.

### Material System

The engine reads the first part of a texture name (up to the first `_`) to decide what material it is. This controls footstep sounds, bullet impacts, and decals.

| Prefix | Material |
|--------|----------|
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |
| `neon` | Neon (emissive) |
| `glass` | Glass |
| `wood` | Wood |

If the engine doesn't recognize a prefix, it falls back to a default material and logs a warning.

### Specular Maps

You can add a specular intensity map alongside any diffuse texture by naming it `{diffuse}_s.png`. It must be the same resolution as the diffuse.

- **Format:** grayscale PNG, R8Unorm, linear color space
- **Effect:** controls the brightness of light highlights on the surface

Example: `wall.png` → diffuse; `wall_s.png` → specular intensity.

You can auto-generate `_s` maps using the included tool, which applies sensible defaults by material prefix (`metal_` = shiny, `concrete_` = matte, `wood_` = moderate):

```bash
uv venv && source .venv/bin/activate && uv pip install Pillow
python3 tools/gen_specular.py --input assets/textures/ --recursive
```

---

## Map Sealing

Your map must be a sealed, airtight box of brushes. The compiler flood-fills outward from outside the map and marks any reachable leaf as exterior — those leaves produce no geometry.

A **leak** is a gap in your brush hull. If you have a leak, interior rooms can vanish or the compile can behave unexpectedly. Fix it before compiling; the compiler reports which leaf it reached from outside.

---

## Compilation Errors

| Error | Cause | Fix |
|-------|-------|-----|
| Missing `_fade` on light | Every light entity requires `_fade` | Add `_fade` to every `light`, `light_spot`, and `light_sun` |
| `period_ms` missing | A `*_curve` key is present but no cycle length | Add `period_ms` to the same entity |
| Lightmap atlas overflow | Too many surfaces at the current texel density | Increase `--lightmap-density` (the compiler retries automatically and logs a warning) |
| Exterior leak | Gap in the brush hull | Seal the map and check the compiler output for the breach location |
