# Postretro — Level Design Reference

Levels are made in **TrenchBroom** and compiled to `.prl` files that the engine loads. You don't need to know Rust or touch the engine source — just author your map, run `prl-build`, and play.

---

## Getting Started

### Setting Up TrenchBroom

1. Open TrenchBroom and load the Postretro game definition: `sdk/TrenchBroom/postretro.fgd`.
2. Set the texture path to the `textures/` directory inside your content root. Throughout this document `<content-root>` means the content root itself, not its `textures/` subdirectory — `content/<mod>` for the `mod` the bundle's `postretro.toml` names (its generated `README.md` shows the exact path), and for whatever `mod` your `postretro.toml` names in your own project.
3. Author your map in Quake 1/2 `.map` format. Both Standard and Valve 220 UV projections work and can coexist in the same file.

### Compiling Your Map

The level compiler is `bin/prl-build` in an SDK bundle. Run it from your project root:

```bash
bin/prl-build <content-root>/maps/input.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o <content-root>/maps/output.prl
```

**Common options:**

| Flag | Default | What it does |
|------|---------|-------------|
| `-o <PATH>` | same as input, `.prl` extension | Where to write the compiled file |
| `--baked-root <DIR>` | derived from the map's location | The directory that **contains** `materials/` — where the compiled texture sidecars go. Point it at your project's `baked/`. See below. |
| `--cache-dir <DIR>` | `.build-caches/prl-cache` beside the map | Disposable compiler scratch. Name it, or it lands among your `.map` sources. |
| `--lightmap-density <METERS>` | `0.04` | Lightmap pixel size. Higher values = chunkier shadows, faster compile. Try `0.1` for drafts. |
| `--sh-probe-spacing <METERS>` | `1.0` | How dense the indirect lighting probes are. `2.0` is fine for large open areas. |
| `-v`, `--verbose` | off | Prints each compilation step — useful when something goes wrong |

**`--baked-root` is worth getting right.** It names the directory that *contains* `materials/` — `baked`, not `baked/materials` — and the engine takes a flag of the same name meaning the same thing. If the compiler writes its texture sidecars somewhere the engine does not read them, nothing fails: the engine substitutes a placeholder for every world material and logs a warning, so the level loads with every surface flat grey. `bin/postretro-tool run` points the engine at the right directory for you, and `bin/postretro-tool dist` points the compiler at it. The only time you supply it yourself is a direct `prl-build` call like the one above. See [docs/external-projects.md](external-projects.md).

**Unit scale:** 1 map unit = 0.0254 m (one inch). A standard player-height room is roughly 72–80 units tall.

---

## Lights

### `light` — Point Light

Radiates light in all directions from a single point.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light` | integer | `300` | Brightness |
| `_color` | RGB | `255 255 255` | Light color |
| `_falloff_range` | integer | **required** | How far the light reaches, in map units. The compiler will error if this is missing. |
| `_light_size` | float | `~9.84` (≈0.25 m) | Emitter radius in map units (inches), like `_falloff_range`, driving bake-time soft shadows (wider = softer penumbra). Leave blank for the soft default (~0.25 m); set `0` for a hard 1-texel shadow. Point/spot only — ignored on `light_sun`. Bake-only (not stored at runtime). |
| `delay` | 0/1/2 | `0` | Falloff shape: `0` = linear, `1` = inverse (1/x), `2` = inverse squared (1/x²) |
| `style` | integer | `0` | Preset flicker/pulse animation — see the style table below |
| `_phase` | float | `0.0` | Shifts the animation cycle (0.0–1.0). Set different values on nearby lights sharing the same style so they don't all pulse together. |
| `brightness_curve` | curve | — | Custom brightness animation — see Custom Animation Curves below |
| `color_curve` | curve | — | Custom color animation (only on `_bake_only 1` or `_animated 1` lights) |
| `direction_curve` | curve | — | Custom spotlight aim animation (spotlights only) |
| `period_ms` | float | — | Cycle length in milliseconds. Required when using any `*_curve` key. |
| `_curve_phase` | float | `0.0` | Phase offset for curve-animated lights (0.0–1.0). Works the same as `_phase` but for the curve path. |
| `_start_inactive` | 0/1 | `0` | `1` = the light starts dark at map load. Scripts can toggle it on at runtime. Only meaningful on animated lights. |
| `_dynamic` | 0/1 | `0` | `1` = dynamic light (runs at runtime, not baked). Use for lights that change during gameplay. |
| `_bake_only` | 0/1 | `0` | `1` = contributes to the baked lightmap and SH volume only; no runtime presence. Good for ambient fill that doesn't need to affect gameplay. |

**Static vs. dynamic:** By default, lights are baked into the lightmap and indirect lighting volume at compile time — they're essentially free at runtime and cast soft baked shadows (penumbra width controlled by `_light_size`/`_angular_diameter`; set to `0` for the classic hard-pixel look). Set `_dynamic 1` if you need a light to move, change intensity during play, or be spawned by a script.

---

### `light_spot` — Spotlight

A cone-shaped light that points in a specific direction. Inherits all keys from `light`, plus:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `_cone` | integer | `30` | Inner cone angle in degrees — the full-brightness region |
| `_cone2` | integer | `45` | Outer cone angle in degrees — the edge where brightness fades to zero |
| `angles` | angles | `"0 0 0"` | Direction as `"pitch yaw roll"` (e.g. `"-90 0 0"` points straight down) |

---

### `light_sun` — Directional Light

A single directional light that hits everything from the same angle, like sunlight or a distant area light. Its position in the map doesn't matter — only direction.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `angles` | angles | `"-90 0 0"` | Direction as `"pitch yaw roll"` in degrees |
| `_angular_diameter` | float | `0.5` | Angular size of the sun disc in degrees, driving bake-time soft shadows (wider = softer penumbra). Leave blank for the soft default; set `0` for a hard shadow. `_light_size` does not apply to directional lights. Bake-only. |

All shared `light` keys (`light`, `_color`, `_falloff_range`, `delay`, `style`, `_phase`, `brightness_curve`, `color_curve`, `direction_curve`, `period_ms`, `_curve_phase`, `_start_inactive`, `_dynamic`, `_bake_only`, `_animated`) apply. (`_falloff_range` and `_light_size` aside, `light_sun` ignores distance falloff and emitter radius.)

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

**Example — color shift (bake_only or animated static lights only):**
```
color_curve "0:255 0 0  500:255 255 255  1000:255 0 0"
period_ms "1000"
```

Use `_curve_phase` to offset curve-animated lights from each other, same as `_phase` works for style presets.

If you set both `brightness_curve` and `style`, the curve wins and `style` is ignored (you'll see a warning in the compiler output).

---

## Fog Volumes

A fog volume is a brush entity that marks a region of the map for volumetric fog rendering.

### Creating a Fog Volume

1. Draw a hollow brush (or any convex brush) covering the area you want to fog.
2. Select the brush and use **Entity > Tie to Entity** (or press `T`) to bind it to `env_fog_volume`.
3. Set your desired properties in the Entity Inspector. The volume's AABB is derived from the brush geometry — the texture on the faces doesn't matter.

Up to 16 `env_fog_volume` entities are allowed per map.

### `env_fog_volume` Properties

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `color` | RGB | `255 255 255` | Fog tint color as `"R G B"` values 0–255 |
| `density` | float | `0.5` | How opaque the fog is. Higher values thicken the fog faster; values above 1.0 are very heavy. |
| `falloff` | float | `1.0` | How sharply the fog fades at the volume boundary. `0` = hard cutoff, `1` = smooth linear ramp. |
| `scatter` | float | `0.6` | How much light scatters toward the camera. Higher values make dynamic spotlights produce more visible beams and halos. |
| `height_gradient` | float | `0.0` | Density bias by height. `0` = uniform density throughout the volume; `1` = denser at the bottom, thinner at the top. Good for ground-hugging smoke or water surface haze. |
| `radial_falloff` | float | `0.0` | Density falloff toward the outer edges of the volume. `0` = no falloff (flat-sided box); `1` = sphere-shaped cloud, thin at the edges and dense at the center. |
| `_tags` | string | `""` | Space-delimited tags for script queries (e.g. `"smoke ambient"`). Not visible in-game. |

### Fog Resolution — `fog_pixel_scale`

The fog pass renders at a reduced resolution for performance and to preserve the chunky pixelated look. The scale is controlled by a worldspawn property:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `fog_pixel_scale` | integer | `4` | Downscale factor for the fog render target. `1` = full resolution, `4` = quarter resolution (default), `8` = coarsest. |

Set `fog_pixel_scale` on the `worldspawn` entity, not on individual volumes. It applies to all fog volumes in the map.

### Tips

- **Keep volumes inside sealed rooms.** A fog volume that crosses exterior geometry will still render but the AABB extends to the brush bounds, which may clip unexpectedly at room boundaries.
- **Overlapping volumes stack additively.** Two volumes occupying the same space add their densities together. Use this intentionally for layered effects (ground mist plus a higher haze layer), but avoid accidental overlap.
- **Match color to your lighting.** Fog lit by a blue neon overhead looks better with a slightly blue `color` than pure white. The color is multiplied by scattered light, so very bright values can wash out.
- **`radial_falloff` hides box edges.** If a rectangular volume looks obviously box-shaped, increase `radial_falloff` toward `0.5`–`1.0` to soften the corners into a cloud shape.

---

## SH Probe Protection Volumes

Adaptive SH probe density is enabled by default. The compiler stores each
4×4×4 base-irradiance brick at L0 (all valid probes), L1 (its eight corners),
or L2 (one mean) according to the composed-lighting error classifier. Direct
SH-delta id 41 also uses adaptive 4×4×4 brick classification; animated/scripted
delta sections ids 27 and 45 remain uniform L0.

To keep a specific map on the uniform L0 grid, set `_sh_coarsen` to `0` on
`worldspawn`. The compiler skips both base-density and direct-delta
classification for that map. `_sh_density_fidelity` on `worldspawn` scales the
base-density classifier's error gates (default `1.0`): smaller positive values
retain more L0 data, while larger values permit more L1/L2 storage. Invalid
values fall back to the default. The `--sh-density-fidelity` compiler flag
overrides the worldspawn value for a bake.

`sh_protect_volume` is an invisible brush entity that forces every intersecting
4×4×4 probe brick to L0 (full valid-probe density) in both the stored base
irradiance and coarsened direct-delta data. Use it where coarsened lighting
changes would be noticeable, such as around a small hero prop, a sharp
lighting transition, or a gameplay focal point.

Create brushwork around the area, then tie it to
`sh_protect_volume`. The compiler converts the brush hull to a world-space
AABB. The brush does not render, collide, become a trigger, or enter the
static world geometry.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `dilation` | float | `0` | Expands the resolved AABB on all six faces, in world/engine meters. Negative values are a compile error. |

The base-density and direct-delta classifiers force every intersecting 4×4×4
probe brick to L0 before they smooth density seams. A brush authored flush to
a brick edge can miss because bricks intersect against their probe-span bounds
rather than an outer cell boundary. When a volume hugs an edge, set `dilation`
to roughly half a probe cell or more.

---

## SH Streaming Hints

These invisible compiler-only brush entities influence how baked SH lighting is
clustered and kept warm. They do **not** create geometry, collision, triggers,
runtime entities, door behavior, or portal visibility changes. Each entity
must own exactly one finite, positive-volume convex brush; invalid brushes or
regions that match no portal/cell are compiler errors.

### `streaming_seam_volume`

Use this around a doorway or portal where early SH warm-up is useful. The brush
must pass through positive portal area and have interior on both sides of the
portal plane. It forces a cluster boundary across the matched portals. When
the near-side cluster is visible, the runtime planner prefers warming the far
side even if an opaque door currently blocks render visibility.

This is best-effort lighting preparation, not a door gate: opening a door
before the asynchronous install and SH compose finish still uses the normal
ambient-floor miss fallback.

### `stream_resident_volume`

Use this around lighting that should remain warm, such as a focal room or
transition. Every cluster containing a runtime cell whose AABB overlaps the
brush by positive volume is pinned, along with its baked owner closure. Pins
are never pressure-suppressed or selected as eviction victims.

### `stream_priority_region`

Set `_stream_priority` to an integer from `0` through `3`; blank and `0` mean
no priority hint. Positive values take the maximum where regions overlap and
rank only optional seam warm-up and two-hop prefetch work under pressure.
They never outrank visible or pinned demand. Use priority to retain the more
important of several otherwise-safe optional regions, not to increase the GPU
pool budget or guarantee a pop-free cold doorway.

---

## Textures

Textures are PNG files under `content/<mod>/textures/<collection>/<name>.png`. TrenchBroom requires this one-level subdirectory structure.

### Material System

The engine reads the first part of a texture name (up to the first `_`) to decide what material it is. This controls footstep sounds, bullet impacts, and decals.

| Prefix | Material |
|--------|----------|
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |
| `neon` | Neon |
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
python3 tools/gen_specular.py --input <content-root>/textures --recursive
```

### Surface Depth (Height Maps)

Add a height map next to any diffuse texture and its surface gets real depth: cobblestones stand proud of their mortar, panel seams sink in, plank gaps read as gaps. The effect is strongest at shallow viewing angles, which is exactly where a flat texture normally gives itself away.

Name it `{diffuse}_h.png`. That's the whole workflow — the compiler finds it by suffix. There is no material file to edit and no map key to set, and you don't need an `_s.png` alongside it; a height map on its own is fine. (A helper that generates one is below, but nothing requires you to use it.)

Example: `cobble.png` → diffuse; `cobble_h.png` → height.

**Author an ordinary height map: white = raised, black = recessed.** This is the familiar convention and it is the one the engine wants. Do not pre-invert it. `prl-build` converts height to depth when it bakes, so the inverted form only ever exists inside the compiled `.prm` — you never see it or think about it.

Requirements — each of these **fails the compile**. None of them is a warning you can ignore:

- **Linear color space.** No `sRGB` chunk, no `iCCP` chunk, `gAMA` ≈ 1.0. Same rule as `_s.png` and `_n.png`. A gamma curve on a height map warps the depth. Re-export as linear PNG if your editor tags files by default.
- **Same dimensions as the diffuse.** No scaling, no half-res height maps.
- **Same dimensions as `_s.png`, if you have one.** Specular and height are packed into one two-channel texture in the compiled output, so they have to line up texel for texel.

#### How deep it carves

Depth character comes from the **material prefix** — the same first-token-before-the-underscore rule the Material System table above uses. You control it by naming the texture, not by tuning the PNG:

| Prefix | Carve |
|--------|-------|
| `concrete` | Deepest — the cobblestone and pavement case this is built for |
| `grate` | Moderate |
| `wood` | Shallow — plank gaps |
| `metal` | Shallowest — panel seams and rivets |
| `glass`, `neon` | **Flat.** Deliberately |
| anything else | A conservative carve, so a `_h.png` on an unrecognized prefix still shows up |

The compiler does not know about prefixes — it bakes any `_h.png` it finds. So a `glass_*_h.png` compiles cleanly, makes that material's compiled surface texture twice the size it needed to be, and is then ignored at render time. Don't ship one.

A height map's own black-to-white range shapes *where* the surface is high and low; the prefix decides *how far* in meters, and how many flat terraces the depth snaps to. Depths are a couple of centimeters at most.

#### Depth never changes where you can walk

The carve goes **inward only**. Nothing is ever pushed out past the real brush face, so the surface you see is never higher than the surface the map actually has. The player walks on the stone tops — that *is* the true plane — and collision, clipping and mover geometry are untouched. You cannot break a jump or a ledge by adding a height map.

The effect applies to static world brushes and to `kinematic_mover` brushes. It does not apply to `prop_mesh` glTF models.

#### Generating one

`tools/texture-tool` writes `{stem}_h.png` alongside the diffuse, specular and normal maps in the same run. It derives height from diffuse luminance and terraces it so the plateaus line up with the diffuse's own quantization:

`texture-tool` is the one exception to the bundle's "no toolchain needed"
rule. Unlike the prebuilt helpers in `bin/`, it ships as Rust *source* under
`tools/`, so building it needs the Rust toolchain (rustup/cargo), which you
install yourself. If you have no toolchain you cannot run `texture-tool`:

```bash
cargo run --release --manifest-path tools/texture-tool/Cargo.toml -- \
  process --src cobble-source.png --stem concrete_cobble_01 \
  --out-dir <content-root>/textures/street \
  --tileable --spec-profile polished-stone \
  --height-strength 1.6 --height-quantize-levels 6
```

`--height-strength` scales the relief (above `1.0` exaggerates it, below flattens it; each spec profile has its own default). `--height-quantize-levels` sets how many terraces — **lower means fewer, flatter, chunkier plateaus**, which is the retro read the effect is tuned for. The tool writes untagged linear PNGs at the diffuse's exact dimensions, so its output satisfies the rules above by construction. See `tools/texture-tool/README.md` for the full flag list and the per-profile defaults.

#### The player's on/off setting

Players get a **SURFACE DEPTH** setting in the graphics options: **Off** or **On**, defaulting to **On**. `On` is the full effect. `Off` renders exactly as the engine did before the feature existed, and costs nothing — it is there for machines that can't afford the per-pixel march. There is no middle setting. Author for `On`, but don't build a room whose readability depends on it — someone will be playing with it off.

### Model Texture Sidecars

Map compilation bakes texture sidecars for any `prop_mesh` glTF models placed in the map. If you want to prepare a model's textures without compiling a map, run:

```bash
bin/postretro-tool bake-model-textures <scene.gltf>
```

The helper writes `.prm` sidecars under your project's `baked/materials/`. Those files are runtime-required output, but they are regenerable and safe to delete whenever the source model or texture changes.

---

## Map Sealing

Your map must be a sealed, airtight box of brushes. The compiler flood-fills outward from outside the map and marks any reachable leaf as exterior — those leaves produce no geometry.

A **leak** is a gap in your brush hull. If you have a leak, interior rooms can vanish or the compile can behave unexpectedly. Fix it before compiling; the compiler reports which leaf it reached from outside.

---

## Compilation Errors

| Error | Cause | Fix |
|-------|-------|-----|
| Missing `_falloff_range` on light | Every point/spot light requires `_falloff_range` (renamed from `_fade`; no alias) | Add `_falloff_range` to every `light` and `light_spot` |
| `period_ms` missing | A `*_curve` key is present but no cycle length | Add `period_ms` to the same entity |
| Lightmap atlas overflow | Too many surfaces at the current texel density | Increase `--lightmap-density` (the compiler retries automatically and logs a warning) |
| Exterior leak | Gap in the brush hull | Seal the map and check the compiler output for the breach location |
| PNG color-space validation failed | An `_s.png`, `_n.png`, or `_h.png` carries an `sRGB` or `iCCP` chunk, or a `gAMA` that isn't ≈ 1.0 | Re-export the named files as linear PNG with no color-management metadata. The error lists every offending path |
| `_h.png` dimensions must match diffuse | A height map is a different resolution from its diffuse texture | Re-export the height map at the diffuse's exact dimensions |
| `_h.png` and `_s.png` … must have identical dimensions | A height map and its specular sibling disagree | Match them. The two are packed into one two-channel texture and cannot be resized independently |
