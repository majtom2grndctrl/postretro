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
| `--lightmap-density <METERS>` | `0.025` | Lightmap pixel size. Higher values = chunkier shadows, faster compile. Try `0.1` for drafts. The worldspawn key `_lightmap_density` (meters per texel, smaller = finer) sets the same thing per map; the flag overrides it. |
| `--sh-probe-spacing <METERS>` | `1.0` | How dense the indirect lighting probes are. `2.0` is fine for large open areas. |
| `-v`, `--verbose` | off | Prints each compilation step — useful when something goes wrong |

**`--baked-root` is worth getting right.** It names the directory that *contains* `materials/` — `baked`, not `baked/materials` — and the engine takes a flag of the same name meaning the same thing. If the compiler writes its texture sidecars somewhere the engine does not read them, nothing fails: the engine substitutes a placeholder for every world material and logs a warning, so the level loads with every surface flat grey. `bin/postretro-tool run` points the engine at the right directory for you, and `bin/postretro-tool dist` points the compiler at it. The only time you supply it yourself is a direct `prl-build` call like the one above. See [docs/external-projects.md](external-projects.md).

**Unit scale:** 1 map unit = 0.0254 m (one inch). A standard player-height room is roughly 72–80 units tall.

### Entity facing

An entity faces the direction of its TrenchBroom arrow: the yaw of its `angle`
key, or the middle value of `angles` (`"pitch yaw roll"`). This holds for
everything that has a front: a `player_spawn` looks along the arrow, and a
`prop_mesh`, an enemy, a pickup, or the enemies an `entity_spawner` releases
turn their model's front along it, and a `projectile_trap` fires along it. An entity with no angle key faces yaw `0`.
When an `entity_spawner` releases several copies, they line up side by side
to the left of its arrow, starting at the spawner.

Set only the yaw for characters and pickups. Pitch and roll tilt a `prop_mesh`,
but an enemy keeps only its yaw once it starts turning, and a player start
ignores roll. Lights use `angles` as a direction instead; see their keys below.

---

## Player Spawns

A `player_spawn` is a place a player can start, not a player by itself. However
many a map has, one player gets one body.

- **Where the player stands.** Rest the spawn's box on the floor, as
  TrenchBroom does when you drop it there. The bottom of the box is where the
  player's feet stand, whatever size your mod makes the player. A box floating
  above the floor starts the player in the air, and they drop. A box sunk into
  the floor is an error in the map, not something the engine fixes.
- **Who starts where.** In single player, and for the host of a co-op game, the
  player starts at the first `player_spawn` in the map that is not marked
  `coop_only` and whose `entity_class` is registered. Each player who joins a
  co-op game takes the first spawn in map order that nobody is using. A spawn
  whose `entity_class` is not registered is skipped for everyone. When every
  spawn is taken, joiners share them and the engine logs a warning, so give a
  co-op map one spawn per player.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `angle` / `angles` | yaw, or pitch yaw roll | `0` | The direction the player looks (see Entity facing). |
| `coop_only` | 0/1 | `0` | `1` keeps this spawn for co-op joiners: the first player never starts here. If every spawn is marked, the first player takes the first one and the engine logs a warning. |
| `entity_class` | string | `player` | The registered entity the player is spawned as. |

Spawns appear in the map file in the order you placed them. TrenchBroom keeps
that order, but if a co-op spawn must never be the first player's, mark it
`coop_only` rather than relying on its position.

---

## Lights

### `light` — Point Light

Radiates light in all directions from a single point.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light` | integer | `300` | Brightness |
| `_color` | RGB | `255 255 255` | Light color |
| `_falloff_range` | integer | **required** | How far the light reaches, in map units. The compiler will error if this is missing. |
| `_light_size` | float | `~9.84` (≈0.25 m) | Emitter radius in map units (inches), like `_falloff_range`, driving bake-time soft shadows (wider = softer penumbra). Leave blank for the soft default (~0.25 m). `0` gives the crispest edge the lightmap allows: a baked (lightmap) shadow edge ramps over about one lightmap texel, so it never shows texel stair-steps. For crisper contact shadows, use a finer lightmap density (worldspawn `_lightmap_density`, or `--lightmap-density`). Keep the emitter clear of walls: when more than a sliver of a light's emitter reaches into a surface (the light sits noticeably closer to it than its `_light_size`), the compiler warns (naming the entity and the share of the emitter inside geometry), the light comes out dimmer by about that share, and its lightmap takes longer to bake. Move the light off the surface or shrink `_light_size`. Point/spot only — ignored on `light_sun`. Bake-only (not stored at runtime). |
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

**Static vs. dynamic:** By default, lights are baked into the lightmap and indirect lighting volume at compile time — they're essentially free at runtime and cast soft baked shadows (penumbra width controlled by `_light_size`/`_angular_diameter`; `0` gives the crispest edge the lightmap allows, about one texel wide, and a finer lightmap density (worldspawn `_lightmap_density`, or `--lightmap-density`) makes it crisper still). Set `_dynamic 1` if you need a light to move, change intensity during play, or be spawned by a script.

**Bounce colour.** Baked indirect light takes each static-world texture's average diffuse colour, so repainting a texture changes the colour it spills onto its surroundings. A texel "renders as glow" when its `_e` (emissive) map has non-zero colour there and the texture's name prefix gives it emissive strength, as `neon_` does (see Material System under Textures); glow texels are left out when non-glowing texels remain, and a fully glowing texture uses its full average. A face with no diffuse texture bounces neutral mid-grey. Only static world faces bounce colour: movers, fluids and models do not. Emissive textures still need a separate light entity to light a scene.

**Keep static lights out of solid.** A static `light` or `light_spot` whose origin sits inside a brush is dropped at compile time, and the compiler warns, naming the entity. It lights nothing, has no runtime presence, and won't match `getMapEntities("light", …)`. A light flush against a brush face is kept, not dropped, but a default-size emitter then sits half inside the wall and the compiler warns about it (see `_light_size` above): keep it clear of surfaces or shrink `_light_size`. If you see the dropped-light warning, move the light into open space. Dynamic lights and `light_sun` are never dropped this way.

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
| `_angular_diameter` | float | `0.5` | Angular size of the sun disc in degrees, driving bake-time soft shadows (wider = softer penumbra). Leave blank for the soft default. `0` gives parallel rays and the crispest edge the lightmap allows: a baked (lightmap) shadow edge ramps over about one lightmap texel, so it never shows texel stair-steps. For crisper contact shadows, use a finer lightmap density (worldspawn `_lightmap_density`, or `--lightmap-density`). `_light_size` does not apply to directional lights. Bake-only. |

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

Fog is volumetric: the engine marches a ray through each fog region and adds in-scattered light along the way. Three entities place fog, and they share most of their keys.

| Entity | Kind | Shape | Use it for |
|--------|------|-------|-----------|
| `fog_volume` | brush | Ellipsoid (axis-aligned brush) or the brush's own convex hull (any other brush) | Rooms, corridors, large or oddly shaped regions |
| `fog_lamp` | point | Sphere | A halo around a light, a puff of smoke |
| `fog_tube` | point | Capsule, with a tilt | Strip lights, beams, steam from a pipe |

**Sixteen fog entities per map, all three types combined.** The compiler skips any beyond the sixteenth and logs a warning naming the entity type. Entities are counted in the order they appear in the `.map` file.

Fog has no color key. Its color comes from the level's baked lighting. To shift the hue, use `tint` (see the Appearance table below).

### Placing a `fog_volume`

1. Draw a convex brush covering the area you want to fog.
2. Select it and use **Entity > Tie to Entity** (or press `T`) to bind it to `fog_volume`.
3. Set properties in the Entity Inspector. Only the brush's shape matters; the texture on its faces does not.

The compiler picks the fog's shape from the brush's faces:

| Brush | Fog shape | Keys that apply |
|-------|-----------|-----------------|
| **Axis-aligned.** Every face normal points along ±X, ±Y, or ±Z (a box, within about 1°). | An ellipsoid inscribed in the brush's bounding box. Dense in the middle, fading toward the surface. | `falloff`. `edge_softness` is ignored. |
| **Anything else.** Any face is tilted. | The brush's own convex volume, with a hard or softened boundary at each face. | `edge_softness`. `falloff` is ignored. |

An axis-aligned `fog_volume` may own several brushes; their bounding boxes are merged into one. A tilted one must own exactly one brush and at most 16 faces, or the compiler stops with an error. To fog a space with a compound shape, use several `fog_volume` entities.

For a plain sphere or capsule, `fog_lamp` and `fog_tube` are easier to place and come with tuned defaults.

### Placing a `fog_lamp` or `fog_tube`

Place the point entity where you want the fog centered. `radius` (and `height` for a tube) size it, in map units. TrenchBroom shows a unit-sized model scaled to match.

A `fog_tube` is a capsule along its local up axis. `yaw` turns it around the vertical axis first; `pitch` then tilts it around the resulting horizontal axis. Both are in degrees. With both at `0` the tube stands upright.

The engine bounds each point-entity volume by an axis-aligned box that encloses the shape, and the fade is measured inside that box. A `fog_lamp` with `radial_falloff` above `0` fades to nothing at the sphere's surface. A `fog_tube` is an approximation: its fade is measured to the corner of its bounding box, so density at the box faces is not zero and a tilted tube can look boxy. Raise `radial_falloff` to hide the box.

### Keys

Keys you do not set take the defaults below. A value that cannot be read as a number is silently replaced by the default, so check the compiler log if a key seems to have no effect.

#### Density and shape

| Key | Entities | Type | Default | Range | What it does |
|-----|----------|------|---------|-------|--------------|
| `density` | all | float | `0.5` (`fog_tube`: `0.3`) | `0` and up | How opaque the fog is. `0` is invisible; values above `1` are very heavy. |
| `falloff` | `fog_volume` (axis-aligned) | float | `2.0` | `0` and up | Fade exponent from the center to the surface. `0` = uniform density with a hard edge; higher = denser core, softer edge. |
| `radial_falloff` | `fog_lamp`, `fog_tube` | float | `2.0` (`fog_tube`: `1.5`) | `0` and up | Same exponent as `falloff`, for the point entities. `0` fills the whole bounding box evenly. |
| `edge_softness` | `fog_volume` (tilted) | float | `1.0` | `0` and up | Width of the fade band just inside each face, in **meters** (1 m ≈ 39 map units). `0` = hard cutoff at the face. |
| `radius` | `fog_lamp`, `fog_tube` | float | `64` (`fog_tube`: `32`) | above `0` | Sphere or capsule radius, in map units. Zero or negative is a compile error. |
| `height` | `fog_tube` | float | `128` | above `0` | Capsule length tip to tip, in map units. Zero or negative is a compile error. |
| `pitch` | `fog_tube` | float | `0` | any | Tilt, in degrees, around the horizontal axis after yaw. |
| `yaw` | `fog_tube` | float | `0` | any | Turn, in degrees, around the vertical axis. |

#### Light and scatter

| Key | Entities | Type | Default | Range | What it does |
|-----|----------|------|---------|-------|--------------|
| `glow` | all | float | `0.6` | `0`–`1` | How much the fog lights up near light sources. `0` = the fog still blocks the view but takes no light, so it stays dark even under bright lights; `1` = it picks up the full light color. Raise for misty glow, lower for thick opaque smoke. |
| `light_range` | all | float | `1.0` | above `0` | Scales how far dynamic lights reach inside this fog. `1.0` = same reach as open air, `2.0` = double, `0.5` = half. Zero or negative is raised to `0.001` with a warning. |
| `scatter_bias` | all | float | `0` | `0`–`100` | Makes the fog brighten when you look toward baked light. `0` = flat haze, `100` = strongest directional glow. Out-of-range values clamp, with a warning. |
| `ambient_scatter` | all | float | `1.0` | `0`–`1` | How much baked ambient light shows in the fog. `0` = only dynamic lights show, `1` = full ambient. Out-of-range values clamp, with a warning. |
| `min_brightness` | all | float | `0.0` | `0` and up | A floor on the fog's scattered light, so it stays at least this bright in unlit areas. `0` = no floor. The floor is applied before `tint`, so it takes the fog's color. |

#### Appearance

| Key | Type | Default | Range | What it does |
|-----|------|---------|-------|--------------|
| `tint` | color | `255 255 255` | `0`–`255` per channel | Multiplies the scattered light color. White = no change. A value that is not three whole numbers in range falls back to white. |
| `saturation` | float | `1.0` | `0` and up | `0` = greyscale, `1` = natural, above `1` = boosted. Applied before `tint`. |

#### Scripting

| Key | Entities | Type | Default | What it does |
|-----|----------|------|---------|--------------|
| `_tags` | all | string | `""` | Space-delimited tags. Scripts find fog by tag. Not visible in-game. |

### Changing fog from scripts

Scripts reach fog by tag and can change `density`, `glow`, `edge_softness`, `falloff` (the value of `falloff` or `radial_falloff`, whichever the entity uses), `tint`, `saturation`, `min_brightness`, and `light_range` while the level runs. The shape, `scatter_bias`, and `ambient_scatter` are baked at compile time and cannot change. Scripts name these fields in camelCase (`edgeSoftness`, `minBrightness`, `lightRange`). Ranges and clamping are in the [`FogVolumeComponent` reference](scripting-reference.md#fogvolumecomponent).

Changing `edge_softness` on an axis-aligned `fog_volume` or on a point entity does nothing visible, and neither does changing `falloff` on a tilted `fog_volume`; the shape ignores that value.

### How overlapping fog combines

Fog entities may overlap freely.

- **Densities add.** Two volumes in the same space are denser together than either alone. Use this for layered effects, such as ground mist under a higher haze.
- **Glow takes the highest value** among the overlapping volumes.
- **Everything else is a density-weighted average** at each point: `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, and `ambient_scatter`. A thin volume overlapping a thick one barely changes the thick one's color.

### Fog Resolution — `fog_pixel_scale`

The fog pass renders at reduced resolution for performance and to keep the chunky pixelated look. A `worldspawn` key sets the scale for every fog volume in the map:

| Key | Type | Default | Range | What it does |
|-----|------|---------|-------|--------------|
| `fog_pixel_scale` | integer | `4` | `1`–`8` | Downscale factor for the fog render target. `1` = full resolution, `4` = quarter resolution, `8` = coarsest. Coarser is faster but blockier near edges. Values above `8` clamp to `8`; `0`, negative, or unset use `4`. |

Set it on the `worldspawn` entity, not on individual fog entities.

### Tips

- **Let the lighting color the fog.** Fog takes its color from the baked lighting around it. Leave `tint` white and nudge it only to shift the hue.
- **Soften the box.** A rectangular `fog_volume` is an ellipsoid already, so raise `falloff` to make the core denser and the edge fainter. For a `fog_lamp` or `fog_tube`, raise `radial_falloff` toward `2`–`3`.
- **Thick smoke versus misty glow.** Thick opaque smoke: high `density`, low `glow`. Misty glow around lights: moderate `density`, high `glow`.
- **Fog in the dark.** Set `min_brightness` to give fog a faint visible body in unlit rooms.
- **Keep volumes inside the space you mean them for.** Walls do not stop fog; it fills its whole volume. A volume that crosses a wall also fogs the space on the other side.
- **Mind the sixteen-entity cap.** A few well-placed large volumes beat many small ones.

---

## Environment Volumes: Fluids, Gravity and Push

Three non-solid brush entities change what a region does to the player:

| Entity | Key | Use it for |
|--------|-----|-----------|
| `fluid_volume` | `fluid` | Water, sludge or any other fluid to swim in. A pool, a channel, a block of water floating in mid-air. |
| `gravity_volume` | `gravity` | A region with its own gravity: a low-gravity room, a weightless chamber. |
| `push_volume` | `push` | A steady force in any direction: an updraft, a wind, a current. |

Each also takes `priority` (see How overlapping volumes combine).

### Placing a volume

1. Draw a convex brush covering the region.
2. Select it and use **Entity > Tie to Entity** (or press `T`) to bind it to the volume entity.
3. Set its key in the Entity Inspector.

Each brush is one convex piece of the volume. One entity may own several brushes, and together they make one volume. Volumes may be any convex shape, at any height: a fluid block can float in mid-air with nothing under it.

**Volumes are never solid.** They draw nothing except a fluid's surface, never block movement or shots, never seal the map, and change nothing about lighting, visibility or navigation. A volume that crosses a wall works on both sides of it, so keep each volume inside the space you mean it for.

**Volumes cannot move.** They are baked into the level when it compiles. A volume cannot be part of a `kinematic_mover`, and scripts cannot move, switch off or retune one.

### Keys

| Key | Entity | Type | Default | What it does |
|-----|--------|------|---------|--------------|
| `fluid` | `fluid_volume` | string | **required** | The name of a fluid your mod declares with `defineFluid`, such as `water`. The mod's declaration sets how thick the fluid is, the screen tint and sound muffling while your view is inside, the surface opacity, how strongly the surface reflects (`surfaceReflectivity`), and the splash sounds (see [Fluids and swimming](scripting-reference.md#fluids-and-swimming)). A name the mod does not declare loads as dry, with a warning naming the entity and the fluid. |
| `gravity` | `gravity_volume` | float | **required** | Gravity inside the volume, in **m/s²**, signed like worldspawn `initialGravity`: negative pulls down. `-20.32` is the level default, `-6` is low gravity, `0` is weightless, and a positive value pulls up. It replaces the level's gravity inside the volume. A script's `setGravity` changes the level's gravity everywhere else, never inside a gravity volume, and `getGravity` returns the level's gravity even inside one. |
| `push` | `push_volume` | three floats | **required** | An acceleration in **m/s²**, in map axes (X, Y, Z, with Z up, as TrenchBroom shows them). `0 0 25` is an updraft, `8 0 0` a wind toward +X, `0 6 0` a current toward +Y. A push never changes which way is down. |
| `force` | `push_volume` | float | `1` | How hard the push shoves the player, who feels `push` × `force`. `push` keeps its direction and look: a current's surface drifts, and particles flow, the same at any force. `0` makes a decorative current that pushes nothing; above `1` shoves harder than it looks. Any finite number from `0` up. |
| `priority` | all three | integer | `0` | Which volume wins where volumes that set the same key overlap. Higher wins. |

`gravity` and `push` are in metres per second squared, not map units. The level default is −20.32 m/s², about twice Earth's 9.81: shooter jumps use exaggerated gravity so they land quickly.

A blank `fluid`, `gravity` or `push`, a `gravity` with more than one number, a `push` without exactly three numbers, a negative, infinite or non-numeric `force`, or a volume entity with no brush fails the build, and the error names the entity.

### How overlapping volumes combine

Volumes may overlap freely. Each key resolves on its own, among the volumes that set it:

1. The highest `priority` wins.
2. On a tie, the smallest volume wins. Size is the total volume of the entity's brushes. A pool inside a room needs no priority to win over the room.
3. On an exact tie, the entity that comes first in the `.map` file wins.

Where no volume sets a key, the level's own value applies: worldspawn `initialGravity` (or whatever a script last set with `setGravity`), no push, and no fluid.

Because each key resolves separately, volumes that set different keys combine. A water pool inside a low-gravity room is wet, and it has low gravity. Gravity and push apply at the player's position. A fluid applies wherever it covers part of the player's body from feet to head, and how deep the player is counts only the winning fluid's brushes.

Push is one winner, never a sum, and `force` goes with its winning push: where volumes overlap, the player feels the winning volume's `push` × its own `force`, never a mix of two volumes. That gives you two patterns:

- **A calm pocket.** Put a `push_volume` with `push` `0 0 0` inside a wind, at a higher `priority`. Nothing pushes inside the pocket.
- **Lift and drift together.** A shaft that should both lift and drift sideways is one `push_volume` carrying the combined vector, such as `4 0 15`.

A brush that needs several keys is several entities on the same brush shape. A current is a `fluid_volume` with a matching `push_volume` on top of it; a swimmer in it drifts with the push, and the fluid's surface drifts with it, at the speed the push alone carries a swimmer at `force` 1. A bending river is one `fluid_volume` under several `push_volume` entities, one per stretch: a push is one vector per entity, so each stretch needs its own, and the surface drifts along each, exactly where its push is. The surface drifts only where the push holds the water 2 mm below the surface: a current must reach within 2 mm of the waterline, and a push from above must dip at least 2 mm into the water. A current sunk wholly beneath the waterline carries swimmers but leaves the surface still, and a push whose bottom rests on the waterline, such as a wind over the water, moves no water. A vertical push (an updraft or a down-current) has nothing along a level surface, so the surface looks still from above.

### What the player feels

- **Fluids.** A player whose descriptor has a `movement.swim` block swims once deep enough, and can jump out from the surface. A player without one walks along the bottom.
- **Gravity.** The player falls, jumps and swims under the volume's gravity. Leaving the volume brings the level's gravity back at once.
- **Push.** The player feels `push` × `force`: `force` `0` pushes nothing, and the lift-off and the steady drift scale with it. The terminal speed below is the player's own and does not change with `force`. A sideways push makes a standing player drift at a steady speed, and walls stop the drift. Airborne, the drift carries on after leaving the volume until you land. An upward push stronger than gravity lifts a standing player off the ground, and the rise levels off at the player's terminal fall speed. A weaker upward push only makes jumps floatier.

Enemies take only a volume's gravity, and only when it pulls down: in a volume whose gravity pulls up, an enemy behaves as if weightless. Enemies ignore push, do not swim, and walk along the bottom of a fluid. Projectiles are not affected by any volume.

### Fluid surfaces

The compiler draws a fluid's surface from its brush faces, using each face's texture. The surface is translucent and visible from both above and below. It moves in whole texels on its texture's own grid, so a fluid has the grain of a wall at the same texture scale: still water shimmers and rings, and a current drifts the way it pushes. Scale the texture down for finer water.

- A face flush against solid world draws nothing, so a pool sunk into the floor shows only its top. A face partly against solid world draws only its open part.
- A face shared by two brushes of the same `fluid_volume` draws nothing.
- **Build one pool as one entity.** A pool made from two `fluid_volume` entities draws a surface where they meet. Give one entity several brushes instead.

### Fluid reflections

Every up-facing fluid surface reflects the room around it, with no authoring: after a rebuild the compiler derives a reflection probe for each pool, fitted to the room the pool sees. A pool sunk below the floor reflects the room, not the pit. How strongly a fluid reflects is its declaration's `surfaceReflectivity` (see [Fluids and swimming](scripting-reference.md#fluids-and-swimming)). Sloped and side faces do not reflect. Seen from below the surface, nothing reflects.

When the fitted box is wrong, draw a `reflection_volume` brush around the room. It is an invisible, compiler-only brush entity: it does not render, collide or change the lighting.

| Key | Type | Default | What it does |
|-----|------|---------|--------------|
| `_capture_offset` | two floats | none | Moves the capture point, the spot the room is photographed from, in **map units** on the map's horizontal axes: `32 -16`. It starts in the middle of the lowest water inside the box, 2 mm above the surface. Use it to step clear of a pillar standing in the water there. Its height cannot be moved. |

- **The box is the reflection's room and its reach.** The reflection projects onto the box's walls, and the box claims the up-facing fluid surfaces inside it. A surface binds the smallest box that contains the centre of its area. On a tie, the entity that comes first in the `.map` file wins. A surface no box claims keeps its derived probe.
- **Offset errors.** An offset with three numbers (the third would be vertical), one that is not two numbers, or one that moves the capture point outside the box fails the build, and the error names the entity.
- **Too many boxes.** A level holds at most 256 reflection probes. `reflection_volume` boxes past that cap refuse the build; derived probes past it are dropped with a warning instead, the ones over the least water first.
- **Warnings.** A box with no water inside, or whose water smaller boxes all claim, draws nothing and warns. A capture point inside solid, inside a fluid or outside the level drops its probe with a warning naming the entity. A capture point within 1 cm of a wall moves 1 cm clear of it; it drops with a warning if that move would leave its box or its water, or if walls stand within 1 cm on both sides of it. Derived probes drop the same way, and their water then reflects nothing: draw a `reflection_volume` there. A pillar standing in the water where the capture point would land does not drop it: the point moves onto the water beside the pillar, which then blocks much of the view, so use an offset to step clear. A box holding water at more than one height warns, naming the heights, and captures above the lowest. A capture point that sees less than half of its probe's water warns, naming the probe; the water it cannot see reflects nothing. Move the point with `_capture_offset`, or give the hidden water its own box.

**Small decor near water.** The probe photographs the room from one point, so small brushes near the water, such as pillars, posts, trim, crates and steps, would show there blurred or at the wrong angle. The compiler leaves them out of the probe, and they reflect through screen-space reflections instead, which show them with true parallax. What counts as small scales with the water's width, so a pillar that counts as small beside a lake may not beside a fountain. Walls, floors and ceilings stay in the probe, and so does anything hanging over the water, like a dock or a footbridge, whose underside the reflection shows. At reflection quality **Low** there are no screen-space reflections, so left-out decor does not reflect at all.

To force brushes in or out, select them and use **Create Brush Entity → `func_group`**, then set `_reflection_capture` on it. `switch` takes the same key.

| Key | Values | Default | What it does |
|-----|--------|---------|--------------|
| `_reflection_capture` | `auto`, `always`, `never` | `auto` | `auto` follows the rule above. `always` keeps every brush of the entity in the probe; use it when a left-out or `never` brush, such as a short wall between two openings, lets the reflection show what lies behind it, or when the probe's box reaches past a left-out or `never` wall and the reflection shows what lies beyond. `never` leaves the brush out of the probe except where the probe would otherwise see through the level: an outer wall with only the exterior behind it, or a brush buried in solid, still shows its own surface. Any other value warns, naming the entity, and acts as `auto`. |

Worldspawn brushes always follow the rule. A TrenchBroom group or layer cannot carry the key: TrenchBroom drops it when it saves, so the compiler ignores it there with a warning. Use a `func_group` brush entity instead.

### Tips

- **Sink pools into the floor.** Cut a pit into the world brushwork and fill it with the fluid brush. Leave the surface a little below the rim so swimmers can climb out with a jump, or by climbing over the edge when the player descriptor has a `movement.vault` block.
- **Mind the depth.** A player starts swimming only once the fluid covers the share of their body that the player descriptor's `enterDepth` names; at `0.5`, about waist deep. Shallower is wading.
- **Push is acceleration.** `8` is a firm wind; `25` up is enough to lift a player against the default gravity. Start small and tune by feel.
- **Keep volumes clear of walls you don't mean.** Like fog, a volume does not stop at a wall.

---

## Movers

A `kinematic_mover` is a brush entity that slides along a path of `kinematic_waypoint` points, spins, or both: doors, lifts, platforms, carousels. This section covers what a mover does to a player in its way.

### Pinned players

A mover never presses a player into the world or into another mover, unless its `block_policy` is `crush`. A player is **pinned** when the mover's next move would do that:

- **A rider**, standing on the mover or vaulting onto it, whom the mover would carry into a ceiling, a wall or another mover.
- **Any other player** the mover pushes, with a wall, a door jamb or another mover behind them.

### Block policy

`block_policy` picks what the mover does to a player in its way:

| `block_policy` | Player in its path | Player pinned |
|----------------|--------------------|---------------|
| `displace` (default) | Pushes them along. | Holds, then carries on the tick the player is clear. |
| `stop` | Holds while it touches them, then carries on. | Holds, then carries on. |
| `reverse` | Backs off along its path. | Backs off if its path heads into the player and the way back is clear. Otherwise holds. |
| `crush` | Pushes them along. | Keeps moving and deals `crush_damage` every `crush_interval_ms`. Never yields. |

- **Holding** stops the mover's travel and its spin together.
- **A rider on a `stop` or `reverse` mover** (or just after jumping off it) is not an obstruction to it. A rider the mover would carry into a ceiling, wall or another mover, in any direction, is pinned. A rider may stand right up to a ceiling; a move that would carry them into it pins.
- **`reverse` holds instead of backing off** when the mover has no path to back along, when its spin causes the pin, when the pin is off its heading (such as a player pressed sideways to the path), when it is still at its start, or when backing off would pin someone else.
- **A hold keeps its plan.** A `once` door finishes its run once the player is clear. A mover sent to a path node by a script still goes there. An expiring `auto_close_ms` still sends a held door closed, and the closing door holds for the player the same way.
- **A `reverse` back-off is a new course.** It drops a scripted `goToPathNode` target and heads for the other end.
- **A `once` mover that backs off to its start** ends unstarted: its trigger's `start` runs it again, and a `reverse` command on it at its start sends it out along its path (opens it). An `auto_close_ms` countdown that expires on such a parked mover does nothing.
- **Dead players' bodies count.** A body left under a default `once` lift holds it until the body is gone.
- **`crush` never yields**, even at `crush_damage` `0`: then it moves through the player and leaves them inside it, unharmed. It also crushes a rider carried sideways into a wall and a player squeezed between it and another mover.

Enemies are never carried or pushed, so none of this applies to them: a `displace` mover ignores them, and a `stop` or `reverse` mover reacts to an enemy on its heading (and fires the blocked event) as it does to a player.

### Blocked event and sound

`blocked_event` names a reaction to fire, and `blocked_sound` a sound to play, when the mover first reacts: a `stop` or `reverse` mover meeting a player or enemy on its heading, or any non-`crush` mover first holding or backing off for a pinned player. A hold fires once for as long as it lasts, even if its cause changes from a contact to a pin. Once the hold ends (a script stops the mover, it finishes its run, or the player moves clear), the next one fires again.

- A `reverse` mover fires on every back-off, since a back-off is not a hold. A ping-pong `reverse` mover that keeps meeting a player fires each time.
- A script command that restarts a `stop` mover held by an unpinned contact (a `blocked_event` reaction calling `start()`) fires the event again on the next tick. A mover held for a pinned player does not.

### Choosing a response

Outside `crush`, the engine guarantees only that a mover never leaves a player stuck inside it or the world. How the mover responds is yours to author:

- **Wait for the player.** The default: `displace` holds, then carries on.
- **Back off.** Use `reverse`.
- **Anything a script can do.** Name a reaction in `blocked_event`. It can run the door's `reverse()` to back it off the player, play a sound or show a message. A command never moves a held mover into the player: a course back into them holds again.
- **Close no matter what.** Use `crush`.

**Co-op.** Any player can hold a default door open by standing in it. A door that must close, such as one sealing an arena, needs `crush`. On a remote player's screen a held mover may move for a moment, then settle back where the host holds it.

**Headroom.** A lift whose rider is too tall for the gap above its stop just waits, with the rider under the ceiling. A rider who crouches or steps off lets it carry on. The first time a mover holds or backs off for a pinned rider (a rider carried sideways into a wall included), the engine logs one `[Mover]` warning giving the mover's numeric id and its current position in TrenchBroom coordinates; it does not repeat for that mover until the level loads again. Raise the ceiling, lower the stop, or use `crush`.

---

## Teleporters

A `trigger_teleport` brush sends a player who walks into it to a
`teleport_destination` point: either exactly there (`classic`), or carried
relative to a pair of points with their offset, view and momentum kept
(`seamless`), for shifts between copies of a room. Scripts turn teleporters on
and off. Keys, room-copy layout, firing rules and scripting are in
[docs/teleporters.md](teleporters.md).

---

## Ghost Links

A `ghost_link` brush over one copy of a room makes what is inside it — players,
monsters, projectiles and tagged movers that are allowed to ghost — also draw
as translucent ghosts at the matching spot in another copy, with their sounds
echoing there if its style says so. It reuses `teleport_destination` points for
the mapping. Keys, styles, ghost audio and scripting are in
[docs/ghost-links.md](ghost-links.md).

---

## Projectile Traps

A `projectile_trap` point entity fires a projectile weapon from its origin along
its arrow. It has no model, no collision and no health: place it where the
shot should leave, such as just inside a wall slot, facing the room.

| Key | Meaning |
|---|---|
| `weapon` | Canonical name of a projectile weapon descriptor. |
| `angle` / `angles` / `mangle` | Facing. `angle` `-1` fires straight up, `-2` straight down. |
| `interval_ms` | Time between repeats once started. Default `1000`. Rounded up to whole ticks, at least one. |
| `phase_ms` | Delay from a start to the first shot. Default `0`, which fires on the start. |
| `start_on_spawn` | `1` starts it repeating when the level loads. |
| `_tags` | Tags a trigger or script addresses it by. |

The weapon fires exactly as it does for anyone else: its shot pattern, its
recovery and its sounds. A trap never runs out of ammo, overheats or reloads.
A weapon that is not a projectile weapon, a weapon whose primary action
charges, an unknown name, a bad timing value, or a `start_on_spawn` other than
`0`/`1` (or `false`/`true`) leaves the trap inert, and the level load warns
once.

A trap fires once per `fire`. A `fire` that arrives while the weapon is still
firing or recovering is ignored, not queued. `start` repeats on a fixed rhythm
that never drifts; a repeat that lands while the weapon is still recovering is
skipped and the next one stays on time. `stop` ends the repeats and lets a
shot pattern already under way finish.

Trap shots belong to no one: an enemy they hit takes the damage without
turning toward the trap, and no player gets credit. In co-op every player
sees and hears them, and only the host fires them.

**Without a script**, point a `trigger_volume` or `switch` at the trap's tag
and set its `command`: `fire` shoots once each time a player enters, `start`
and `stop` switch the repeats. One trigger can drive movers, traps and
spawners sharing a tag; each takes only the commands it understands (movers
`start`, `stop`, `reverse`, `go_to_path_node`; traps `fire`, `start`, `stop`;
spawners `fire`, one batch each). A tag that only reaches things that ignore
the command warns at level load. Triggers act when a player enters; to stop a
trap when the player leaves, use a second trigger or a script. Scripting:
[scripting-reference.md](scripting-reference.md#projectile-trap-members).

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

`_bounce_fill` on `worldspawn` sets how much light keeps bouncing after its
first bounce off a surface, from `0` to `1` (default `1.0`). At `0` the baked
indirect light is a single bounce: a room no light reaches stays dark. At `1`
it is the full physical fill, so light reflected out of a lit room carries on
into the rooms around it; values between blend the two. It is a map key, not a
compiler flag, so the look travels with the map. A value outside `0`–`1`, or
one that is not a number, warns and uses the default.

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
portal plane. Portals exist only where the compiler divides the level into
cells: across doorways a door seals, through openings in large walls, past the
free ends of walls, and on its cell-size grid. Small props and pillars never
make portals. A seam that crosses none of these fails the build, naming the
seam. The volume never changes cells, portals or what is visible.
It forces a cluster boundary across the matched portals. When
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
rank only optional seam warm-up and nearby prefetch work, ahead of distance
from the camera, including which of it yields under pressure.
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

Add a height map next to any diffuse texture and its surface gets real relief: cobblestones stand proud of their mortar, panel seams sink in, rivets and studs rise, plank gaps read as gaps. The effect is strongest at shallow viewing angles, which is exactly where a flat texture normally gives itself away.

Name it `{diffuse}_h.png`. That's the whole workflow — the compiler finds it by suffix. There is no material file to edit and no map key to set, and you don't need an `_s.png` alongside it; a height map on its own is fine. (A helper that generates one is below, but nothing requires you to use it.)

Example: `cobble.png` → diffuse; `cobble_h.png` → height.

**Mid-gray is the brush face. Darker sinks into it, lighter rises out of it.**

| Shade | Value | Where the surface appears |
|-------|-------|---------------------------|
| Black | `#000000` (0) | Sunk the material's full depth below the face |
| Dark gray | `#404040` (64) | Sunk about half the depth (exactly half for even terrace counts) |
| **Mid-gray** | **`#808080` (128)** | **Exactly on the brush face** |
| Light gray | `#C0C0C0` (192) | Raised about half the depth (exactly half for even terrace counts) |
| White | `#FFFFFF` (255) | Raised the full depth above the face, once terraced |

The scale is linear between rows. White sits at 127/128 of the depth before terracing, and every prefix's terracing carries it up to the full depth. The half-depth rows land exactly on a terrace only when the prefix has an even terrace count. With an odd count, such as `grate` and the default prefix (3 terraces per direction), an exact half step snaps up: `#404040` sinks one third and `#C0C0C0` rises two thirds.

Paint flat areas exactly `#808080` — Photoshop's and GIMP's 50% gray. The engine snaps 127 to flat too. A map with nothing lighter than mid-gray only carves, and pays nothing for raise.

Requirements — each of these **fails the compile**. None of them is a warning you can ignore:

- **Linear color space.** No `sRGB` chunk, no `iCCP` chunk, `gAMA` ≈ 1.0. Same rule as `_s.png` and `_n.png`. A gamma curve on a height map warps the depth. Re-export as linear PNG if your editor tags files by default.
- **Same dimensions as the diffuse.** No scaling, no half-res height maps.
- **Same dimensions as `_s.png`, if you have one.** Specular and height are packed into one two-channel texture in the compiled output, so they have to line up texel for texel.

#### How far it reaches

Depth character comes from the **material prefix** — the same first-token-before-the-underscore rule the Material System table above uses. You control it by naming the texture, not by tuning the PNG:

| Prefix | Depth (each direction) |
|--------|-------|
| `concrete` | Deepest — the cobblestone and pavement case this is built for |
| `grate` | Moderate |
| `wood` | Shallow (same as `metal`) — plank gaps |
| `metal` | Shallow — panel seams and rivets |
| `glass`, `neon` | **Flat.** Deliberately |
| anything else | Moderate, so a `_h.png` on an unrecognized prefix still shows up |

The compiler does not know about prefixes — it bakes any `_h.png` it finds. So a `glass_*_h.png` compiles cleanly, makes that material's compiled surface texture twice the size it needed to be, and is then ignored at render time. Don't ship one.

The map's shades decide *where* the surface sits high or low. The prefix decides *how far*: how deep black sinks, how high white rises, and how many flat terraces each direction snaps to. Depth applies in each direction, so a full-range `concrete` map spans twice its depth from deepest mortar to highest stone. Depth is counted in the texture's own texels — six each way at most, for `concrete` — so it follows the texture's pixel grid, not a fixed distance.

#### Raised texels and the true surface

Height maps never change collision. The player walks on the brush face — the mid-gray level — whatever the map shows. Sunk texels sit below that face, where feet never reach. Raised texels draw *above* it, but the effect cannot change the brush's real shape. Raising therefore costs three things sinking never does:

- **Edges stay straight.** Raised texels never poke past their face's outline. At a ledge lip or outside corner, raised stones look sliced flat along the edge.
- **Walls cut raised floors.** Where floor meets wall, the wall hides any part of a raised floor stone that should stand in front of it.
- **Objects sink into raised texels.** Feet, props, pickups and projectiles draw against the true face. On a strongly raised floor they read as sitting slightly *into* the stones, not on top.

None of this changes how a room plays, only how it reads. Keep strong raises off floors that props and players stand on, and away from ledge lips and trim edges. Sink seams instead. Save raising for studs, rivets, wall brick, and surfaces the player looks at rather than stands on.

The effect applies to static world brushes and to `kinematic_mover` brushes. It does not apply to `prop_mesh` glTF models.

#### Generating one

`tools/texture-tool` writes `{stem}_h.png` alongside the diffuse, specular and normal maps in the same run. It derives height from diffuse luminance and places the diffuse's average brightness on mid-gray, so the result both rises and sinks. Its terrace boundaries follow the diffuse's own edges:

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

`--height-strength` scales the relief (above `1.0` exaggerates it, below flattens it; each spec profile has its own default). `--height-quantize-levels` sets how many terraces, split evenly above and below mid-gray — **lower means fewer, flatter, chunkier plateaus**, which is the retro read the effect is tuned for. Left out, it follows the stem's prefix: `concrete` 12, `metal` and `wood` 4, `grate` and anything else 6. That is twice the engine's own terraces per direction, so every terrace lands exactly on one of the engine's plateaus. A lower count still lands on plateaus when half of it divides the prefix's count: `concrete` (6 each way) also suits 4 or 6. Other counts can put a terrace on a half step, where raised sides render one terrace taller than sunk sides. The tool writes untagged linear PNGs at the diffuse's exact dimensions, so its output satisfies the rules above by construction. See `tools/texture-tool/README.md` for the full flag list and the per-profile defaults.

#### The player's on/off setting

Players get a **SURFACE DEPTH** setting in the graphics options: **Off** or **On**, defaulting to **On**. `On` is the full effect. `Off` renders exactly as the engine did before the feature existed and skips the per-pixel march — it is there for machines that can't afford it. There is no middle setting. Author for `On`, but don't build a room whose readability depends on it — someone will be playing with it off.

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
| Lightmap atlas layer overflow | Too many surfaces at the current texel density, or a `_lightmap_scale` region too fine | Raise `--lightmap-density` (coarser texels), lower the region's `_lightmap_scale`, or split the map |
| Animated block count exceeds the block-table cap | More faces lie within animated lights' reach than the animated lightmap can hold | Shrink animated lights' `_falloff_range`, make some of them static, or split the map |
| Exterior leak | Gap in the brush hull | Seal the map and check the compiler output for the breach location |
| PNG color-space validation failed | An `_s.png`, `_n.png`, or `_h.png` carries an `sRGB` or `iCCP` chunk, or a `gAMA` that isn't ≈ 1.0 | Re-export the named files as linear PNG with no color-management metadata. The error lists every offending path |
| `_h.png` dimensions must match diffuse | A height map is a different resolution from its diffuse texture | Re-export the height map at the diffuse's exact dimensions |
| `_h.png` and `_s.png` … must have identical dimensions | A height map and its specular sibling disagree | Match them. The two are packed into one two-channel texture and cannot be resized independently |
