# Postretro — Level Design Reference

Levels are authored in **TrenchBroom** using the Postretro game config and custom FGD, then compiled to `.prl` with `prl-build`.

---

## Toolchain Setup

1. Load the Postretro game definition (`assets/postretro.fgd`) in TrenchBroom.
2. Point TrenchBroom's texture path at the `assets/textures/` directory.
3. Author your map in idTech2 `.map` format (Quake 1/2 dialect). Both Standard and Valve 220 UV projections work and can coexist in the same file.

### Compiling a Map

```bash
cargo run -p postretro-level-compiler -- input.map -o output.prl
```

| Flag | Default | Description |
|------|---------|-------------|
| `-o <PATH>` | input path with `.prl` extension | Output file path |
| `--pvs` | off | Precomputed PVS instead of default portal graph |
| `--probe-spacing <METERS>` | `1.0` | SH irradiance probe density (meters between probes) |
| `--lightmap-density <METERS>` | `0.04` | Starting lightmap texel size. Higher = chunkier, more retro. The compiler automatically retries at coarser densities if the atlas overflows. |
| `-v`, `--verbose` | off | Per-stage compilation logging |

**Unit scale:** 1 map unit = 0.0254 m (one inch). A player-height room is roughly 72–80 units tall.

---

## Entities

### `light` — Point Light

Omnidirectional light source.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light` | integer | `300` | Intensity |
| `_color` | RGB | `255 255 255` | Light color |
| `_fade` | integer | **required** | Falloff distance in map units. Compilation fails if omitted. |
| `delay` | choices | `0` | Falloff model: `0` = Linear, `1` = Inverse distance (1/x), `2` = Inverse squared (1/x²) |
| `style` | integer | `0` | Light animation style index (flickering, pulsing, etc.) |
| `_phase` | float string | `"0.0"` | Animation cycle phase offset (0.0–1.0) — staggers lights so they don't all pulse together |
| `_dynamic` | choices | `0` | `0` = Static (baked into SH volume + lightmap), `1` = Dynamic (runtime direct lighting, not baked) |
| `_bake_only` | choices | `0` | `0` = Normal (contributes to bake and runtime), `1` = Bake only (contributes to SH/lightmap bake, absent from runtime lighting pool) |

**Static vs. dynamic:** static lights (`_dynamic 0`) are baked into the SH irradiance volume and lightmaps at compile time. They look great and cost nothing at runtime. Dynamic lights (`_dynamic 1`) run through the per-frame direct lighting loop — use these for lights that move, flicker at runtime, or are spawned during gameplay.

---

### `light_spot` — Spotlight

Inherits all keys from `light`, plus:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `_cone` | integer | `30` | Inner cone angle in degrees (full-intensity region) |
| `_cone2` | integer | `45` | Outer cone angle in degrees (falloff region edge) |
| `mangle` | string | — | Direction as `"pitch yaw roll"` in degrees (e.g. `"-90 0 0"` points straight down) |

---

### `light_sun` — Directional Light

A single directional light simulating sunlight or a distant source. Origin is ignored; direction is all that matters.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mangle` | string | `"-90 0 0"` | Direction as `"pitch yaw roll"` in degrees |

All shared `light` keys (`light`, `_color`, `_fade`, `delay`, `style`, `_phase`, `_dynamic`, `_bake_only`) apply.

---

## Textures

Textures are PNG files under `assets/textures/<collection>/<name>.png`. TrenchBroom requires the one-level subdirectory structure.

### Material System

The engine derives a material type from the first `_`-delimited token in the texture name. Known prefixes:

| Prefix | Material |
|--------|----------|
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |
| `neon` | Neon (emissive) |
| `glass` | Glass |
| `wood` | Wood |

Unknown prefixes fall back to a default material with a warning at load time.

### Specular Maps

An optional sibling PNG provides per-texel specular intensity. Name it `{diffuse}_s.png` alongside the diffuse texture; dimensions must match.

- **Format:** R8Unorm, linear color space.
- **Effect:** Modulates the direct lighting highlight. Absent = zero specular.

Example: `wall.png` → diffuse, `wall_s.png` → specular intensity.

**Generating specular maps:** `tools/gen_specular.py` generates `_s` maps from diffuse textures using material-prefix heuristics (`metal_` = high intensity, `concrete_` = low, `wood_` = moderate).

```bash
uv venv && source .venv/bin/activate && uv pip install Pillow
python3 tools/gen_specular.py --input assets/textures/ --recursive
```

---

## Map Sealing

The compiler flood-fills outward from outside the map boundary. Any leaf reachable from outside is considered exterior and produces no geometry. A **leak** — a gap in the brush hull — causes interior leaves to be incorrectly classified as exterior and disappear. Fix leaks before compiling; the compiler reports the first exterior-reachable leaf.

---

## Compilation Errors

| Error | Cause | Fix |
|-------|-------|-----|
| Missing `_fade` on light | `_fade` is required on all light entities | Add `_fade` to every `light`, `light_spot`, and `light_sun` |
| Lightmap atlas overflow | Too many surfaces at the requested density | Increase `--lightmap-density` (the compiler retries automatically and warns in the log) |
| Exterior leak | Gap in brush hull | Seal the map; check compiler output for the breach location |
