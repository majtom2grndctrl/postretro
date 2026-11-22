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
| `--bake-fidelity <low\|high>` | `low` | How fully movers re-bake their bounced light in an everyday build. `low` is fast and re-bakes one bounce per pose; `high` re-bakes each mover's `light_bounces`, as a shipped map does. Not allowed beside `--release`, which always bakes `high`. See Movers: Lighting. |
| `--mover-payload-max-size <SIZE>` | `256MiB` | The most memory the compiler may hold for the movers' baked indirect lighting blocks, counted as it works. Past it the build stops with an error naming the movers; it never drops or coarsens their lighting. The Build Summary reports how much of it the blocks hold. The memory check before baking charges the full cap, three times over while the lighting is assembled, so raising it can make that check refuse instead. Accepts a byte count or a size such as `512MiB`. See Compilation Errors. |
| `--release` | off | Build the map to ship: every stage bakes fresh, the cache is neither read nor written, and movers bake at `high`. Slow. `bin/postretro-tool dist` passes it for you. |
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
| `checkpoint` | string | empty | The checkpoint this spawn belongs to. Spawns sharing a name are that checkpoint's arrival set. Spawns without it are the level-start set. |

Spawns appear in the map file in the order you placed them. TrenchBroom keeps
that order, but if a co-op spawn must never be the first player's, mark it
`coop_only` rather than relying on its position.

### Checkpoint spawns

A level's script saves a checkpoint with `checkpoint("bridge")` (see
[Characters, runs and checkpoints](scripting-reference.md#characters-runs-and-checkpoints)).
Give the spawns players should use from then on `checkpoint bridge`:

- **At a level start**, players arrive at spawns with no `checkpoint` key.
  Keyed spawns are skipped. If every spawn in the level has a key, the level
  start uses all of them and the engine logs a warning.
- **Once `checkpoint("bridge")` fires**, players who respawn and players who
  join late use the `bridge` spawns until the next level start.
- **Continue on a run saved at `bridge`** arrives at the `bridge` spawns. In
  co-op each player takes a different one of them while one is free, by the
  same rule as joiners.
- **A checkpoint name no spawn carries** falls back to the level-start spawns
  and logs a warning.
- **A level seam's name** is a set name too: after a crossing, and Continue
  from one, players use the spawns keyed with the seam's name. With none keyed,
  the level-start spawns apply with no warning (see [Level Seams](#level-seams)).

Place at least as many spawns in a checkpoint's set as the game's player count.

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

**Bounce colour.** Baked indirect light takes each static-world texture's average diffuse colour, so repainting a texture changes the colour it spills onto its surroundings. A texel "renders as glow" when the texture has an `_e` (emissive) map with non-zero colour there, whatever its glow strength (see Emissive Maps under Textures); glow texels are left out when non-glowing texels remain, and a fully glowing texture uses its full average. A face with no diffuse texture bounces neutral mid-grey. Only static world faces bounce colour: movers, fluids and models do not. Emissive textures still need a separate light entity to light a scene.

**Bounce fill.** `_bounce_fill` on `worldspawn` sets how much light keeps bouncing after its first bounce off a surface, from `0` to `1` (default `1.0`). At `0` the baked indirect light is a single bounce: light that has bounced once off a lit surface still spills through openings, but a room reachable only by light bouncing twice or more stays dark. At `1` it is the full physical fill, so light reflected out of a lit room carries on into the rooms around it; values between blend the two. It is a map key, not a compiler flag, so the look travels with the map. A value outside `0`–`1`, or one that is not a number, warns and uses the default.

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

### `light_area` — Area Light

A glowing surface that lights the room as one soft panel: a ceiling skylight, a light table, a strip light. A grid of point lights leaves hot spots on nearby walls and costs bake time per copy; an area light lights evenly, and its shadows soften the way a large panel's do.

**Building one:**

1. Build the fixture from brushes, for example a concrete frame and a glass pane.
2. Paint the faces that should give off light with a texture that has an `_e` map (see [Emissive Maps](#emissive-maps)). Paint the rest with ordinary textures.
3. Select every brush of the fixture and tie them to `light_area` (**Entity > Tie to Entity**, or `T`).

**How it behaves:**

- **Only the glowing faces emit.** A side emits when its texture has an `_e` map and it is visible. A face half buried in a wall, or covered by another brush, emits from its visible part only. Overlapping glowing brushes emit their shared area once, and a panel built from four brushes lights the room exactly as one brush would.
- **One-sided.** Each glowing face emits from its front only. A surface level with the face or behind it gets no light from it, including the fixture's own faces.
- **The brushes are solid world.** They block movement and cast shadows like worldspawn brushes, so a frame shades the room as a real frame would, and one glowing face can shade another of the same light.
- **At least one glowing face.** A `light_area` with no visible `_e` face stops the compile, naming the entity. Slivers too thin to matter do not count.
- **Brightness is per area.** A bigger panel gives more light. Far from a 1 m² panel (about 40 × 40 units), on its axis, `light 300` matches a `light 300` point light with `delay 2`; close in, the panel is dimmer and softer, which is the point.
- **Reach is worked out for you.** The light fades smoothly to nothing at a distance set by its brightness and size, with no visible edge: a 1 m² panel at `light 300` reaches about 16 m (630 units). Double `light` and it reaches about 1.4 times as far. Set `_falloff_range` to choose the reach yourself, measured from the panel.
- **Always baked.** It lights the lightmap, the indirect lighting, characters and movers, and its light bounces like any static light's (see Bounce fill above). There is no dynamic area light.
- **Diffuse only.** An area light draws no specular highlight and casts no real-time shadow from characters or movers; they are lit by its baked light.

**The glow.** Every glowing face draws its `_e` colour × `_glow` × `_color`. A white pane on a red light glows red. `_glow` is separate from `light`, so a dim glow can pair with strong light, or the reverse. The texture's `materials.toml` strength does not apply to these faces, and the glow shows even on a texture whose strength is `0`. No [`emissive_volume`](#emissive-volumes) reaches them: they glow with their light. When a script animates the light, the faces brighten, dim and change colour with the room, on the same frame.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light` | integer | `300` | Brightness per square metre of glowing face. `0` lights nothing, but the faces still glow and follow the light; a negative value stops the compile. |
| `_color` | RGB | `255 255 255` | Colour of its light and of its faces' glow. |
| `_falloff_range` | integer | worked out | Reach in map units, measured from the panel. Leave blank to have the compiler work it out from brightness and size. |
| `_glow` | float | `4` | How brightly the faces glow, independent of `light`. Above `64` reads `64` and warns; a negative value stops the compile. |
| `brightness_curve`, `color_curve`, `period_ms`, `_curve_phase` | curve | — | Custom animation, as on `light` (see Custom animation curves below). `color_curve` needs `_bake_only`, `_animated` or `_start_inactive`. |
| `_animated` | 0/1 | `0` | `1` reserves the animated bake so a mod-wide script can animate the light. A light the map's own script animates is reserved for you. |
| `_start_inactive` | 0/1 | `0` | `1` = the light starts dark, faces and room together, until a script lights it. On an area light this reserves the animated bake by itself. |
| `_bake_only` | 0/1 | `0` | `1` = baked only; no runtime presence, and scripts cannot find it. With `_start_inactive 1` it stops the compile: nothing could ever light it. |
| `_tags` | string | `""` | Space-delimited tags. Scripts find lights by tag. |

**Keys from other lights.** Changing another light to `light_area` in TrenchBroom keeps its keys. These do nothing on an area light, and each warns once, naming the entity and the key: `delay`, `_light_size`, `_phase`, `_angular_diameter`, `carrier`, `_cast_entity_shadows`, `_cone`, `_cone2`, `angle`, `angles`, `mangle`, `target`.

**Keys that stop the compile.** Each names the entity and the key: `_shadow_type sdf`, `direction_curve`, any `style` other than `0` (use `brightness_curve` instead), a negative `light`, and `_bake_only 1` together with `_start_inactive 1`.

**`light 0`.** The light lights nothing, and the compile warns so. Its faces still glow at `_glow` × `_color` and follow the light: they start dark with `_start_inactive 1`, and brighten, dim and change colour when a script animates it. A fade or colour change that runs once and stops leaves them at its last brightness and colour, as it leaves a lit light's room; clearing the animation afterwards, or starting another one, returns them to `_glow` × `_color` first, so a second fade starts from full glow rather than where the first one stopped. Water reflections show them at their average glow, even while the light is dark.

**Scripts** see an area light as a light of type `"Area"`, at the centre of its glowing faces. `pulse`, `fade`, `flicker` and `colorShift` work; `sweep` has no direction to turn and is refused. See [LightComponent](scripting-reference.md#lightcomponent).

**Examples.** The engine's sample content has two: `content/dev/maps/area-light-panel.map`, a room lit by one ceiling panel, and `content/dev/maps/area-lights-demo.map`, two skylights pulsed by the map's script (`content/dev/scripts/area-lights-demo.ts`). Each panel is a frame brush and a glass brush whose underside alone carries a glowing texture.

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

## Emissive Volumes

An `emissive_volume` brush scales and tints the glow of the faces inside it, and gives scripts a handle to pulse, fade, flicker, dim or recolour that glow. It works on faces whose texture already glows: one with an `_e` map (see [Emissive Maps](#emissive-maps)). Its brushes never render, collide or bake.

### Placing an `emissive_volume`

1. Draw a brush around the glowing faces: a thin slab over a sign's face is enough.
2. Tie it to `emissive_volume` (**Entity > Tie to Entity**, or `T`). An entity may own several brushes.
3. Set its keys. Only the brush's shape matters; its texture does not.

Membership is decided per final face: each piece of world surface the compiler produces joins the volume whose brush holds that piece's centre. A face lying in the plane of a volume brush's side counts as inside, so a slab flush on a wall catches the wall's faces too; keep the slab clear of faces you do not mean.

- **Split faces:** the compiler cuts a brush side where other brushes and room boundaries meet it. A glowing brush side that spans two volumes splits between them, each piece following the volume around it. The later cuts for the lightmap atlas never split a face's membership.
- **Overlaps:** a face inside several volumes belongs to the smallest (by brush volume); two of the same size go to the later one in the map. A volume nested inside another keeps its faces whichever comes first.
- **A volume with no glowing face warns**, naming the entity, including one a smaller volume has emptied. It still compiles, and scripts can still address it.
- **At most 256 volumes per map.** The 257th stops the compile, naming the count.
- **Every world face needs a triangle once a volume holds a face.** A degenerate brush anywhere in the map, one with a face the compiler cannot draw, then stops the compile, naming a volume, the face's texture and its brush. Remove or rebuild that brush; with no volume holding faces, the map compiles as before.
- **Static world faces only.** Movers and fluids glow at their texture strength alone; a volume never reaches them, nor models.
- **Not area-light faces.** A [`light_area`](#light_area--area-light)'s glowing faces glow with their light, so a volume never claims them, even a smaller one nested around them. A volume holding only area-light faces warns as empty.

### Keys

| Key | Type | Default | What it does |
|-----|------|---------|--------------|
| `emissive_min` | float | `0` | The glow multiplier at scripted brightness `0`. |
| `emissive_max` | float | `1` | The glow multiplier at scripted brightness `1`, and with no script. |
| `emissive_color` | color | `255 255 255` | Tints the glow. Each channel is read as linear, `0`–`255`. |
| `_tags` | string | `""` | Space-delimited tags. Scripts find volumes by tag. |

A face's glow is its texture's `_e` colour × its texture strength × (`emissive_min` + brightness × (`emissive_max` − `emissive_min`)) × `emissive_color`. Brightness is a script's value in `0`–`1`; it is `1` until a script writes one, so an unscripted volume glows at `emissive_max` in `emissive_color`. A volume at the defaults changes nothing.

Bad keys never stop the compile; each warns, naming the key. A negative value reads `0`, a value that is not a number reads its default, and a value above `64` reads `64`. Then a minimum above the maximum swaps with it. A malformed `emissive_color` reads white.

### Changing glow from scripts

Scripts own brightness and colour; the map owns the range. `getMapEntities("emissive", { tag })` returns each volume, whose `pulse`, `fade` and `flicker` animate brightness in `0`–`1`, unlike lights and fog, whose methods take absolute values. `setEmissiveBrightness` holds a brightness and stops any running animation; `setEmissiveColor` recolours without touching brightness or its animation. No script can push a face past `emissive_max` or tint it brighter than white. See [Emissive primitives](scripting-reference.md#emissive-primitives).

Reflections capture each volume's unscripted look: its faces at `emissive_max` in `emissive_color`. A script that dims a sign does not dim its reflection.

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

A `kinematic_mover` is a brush entity that slides along a path of `kinematic_waypoint` points, spins, or both: doors, lifts, platforms, carousels. This section covers what a mover does to a player in its way, then to enemies and their corpses, and then to baked lighting.

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
- **`crush_damage_type`** names the damage type a crusher deals, by the name the mod registers in `defineMod({ damageTypes })`: in TypeScript, the name of its `const` binding. Empty deals the mod's `defaultDamageType`. A name the mod does not register warns once per load and crushes as the default.

The rules above also apply to enemies. The next section lists where an enemy or a corpse differs from a player.

### Enemies and corpses

Enemies treat movers as solid ground and solid walls, as a player does. A rider is carried, a `displace` mover pushes an enemy that is not pinned, and a closed door stops one.

| `block_policy` | Enemy in its path | Enemy pinned | Corpse |
|----------------|-------------------|--------------|--------|
| `displace` | Pushes them clear. | Holds. The enemy steps out of the mover's course, then the mover carries on. | Pushes it along; never holds. |
| `stop` | Holds while it touches them. | Holds. The enemy steps out, then the mover carries on. | Never holds. |
| `reverse` | Backs off along its path. | Backs off if its path heads into the enemy and the way back is clear. Otherwise holds. The enemy steps out of its course. | Never backs off. |
| `crush` | Pushes them along, with no damage. | Keeps moving and damages the enemy. | Keeps moving and keeps damaging it while it is pinned or engulfed. |

- **Only `crush` damages.** No other block policy hurts an enemy or leaves one embedded in the mover while there is a clear spot to move it to.
- **`crush` needs a pin.** A `crush` mover damages an enemy only when it presses the enemy against static geometry or another mover, or when it engulfs the enemy. An enemy carried through open air, or pushed along with open floor ahead of it, takes nothing. Damage continues after the enemy dies.
- **A pinned enemy walks clear.** When a non-`crush` mover pins a living enemy, the engine moves the enemy out of the mover's course, whatever the enemy's behavior is doing, and its behavior resumes once it is clear. An enemy that returns to the doorway afterwards holds the door again, as a player standing there does.
- **No clear spot.** If the enemy has nowhere to go, or does not get out in time, the mover passes through it and leaves it unharmed.
- **A rider never stops its own mover.** An enemy standing on a `stop` or `reverse` mover does not hold it.
- **Corpses ride and never block.** A corpse is an enemy at zero health, or one with a despawn already queued. It falls under gravity, rides lifts and is pushed by movers, but it never stops, backs off or holds a mover, and a pin against it does not hold a mover either. `crush` still damages it. What happens to the corpse is your policy: removal, gibbing or revival.
- **A corpse pinned in a closing door** ends up inside the door or pushed out of it, depending on how thick the door is and what is behind it. A door thinner than a corpse can push it out the far side. Do not rely on a corpse staying in the door.
- **Revived inside a mover.** An enemy that gets up inside a mover is pushed out the first tick the mover would push it, and a mover that still pins it holds as for any living enemy.

#### Gibbing a crushed corpse

A crush hit raises an impact on a living enemy or a corpse alike, so a script can gib what a crusher kills. Give the crusher its own damage type: set `crush_damage_type` to the name your mod registers for it, which is the name of its `const` binding in the script. The script side is in [Gibbing a crushed corpse](scripting-reference.md#gibbing-a-crushed-corpse).

#### Making an enemy crush-immune

An enemy is crush-immune when its health and every defense layer resist the crush damage type at `0`. Declare `resist: { crush: 0 }` on its `health`, and on each entry in `layers`. It takes no damage, and the crusher passes through it rather than stopping. An immune enemy left under a crusher is pushed out on the first tick the crusher no longer pins it. If the crusher engulfs it completely, it stays inside until the crusher moves off. See [Attaching layers to health](scripting-reference.md#attaching-layers-to-health).

#### Doors and lifts in enemy routes

Enemies plan around doors and lifts. This needs data that `prl-build` writes into the map, so **rebuild the map** after you add or change a mover. A map built before this existed loads without door or lift routing and logs one warning asking you to rebuild. On such a map a lift still carries an enemy standing on it, and a door still blocks enemies by collision, but enemies do not route through doors or lifts and do not ride lifts on purpose.

- **A docked door closes a route.** Any mover that is not a spinner and rests at a waypoint gates the route through its doorway, whatever its block policy, but only while it is docked: resting at one of its waypoints, whether in a timed wait, after a finished run, after a `goToPathNode` hold, or never started. Enemies detour around it, or hold if there is no other way. The brain fact `targetReachable` reads `false` while a docked door cuts the only route (see [Doors and `targetReachable`](scripting-reference.md#doors-and-targetreachable)).
- **A mover in motion gates nothing.** A mover stopped between waypoints, or passing a waypoint without waiting there (`wait_ms` of `0`), does not change enemy routes. It affects enemies by collision and block policy alone. A sweeper or crusher that rests at a waypoint gates its route there like a door.
- **A hollow doorway stays open.** Only a door that fills the way an enemy would need blocks the route. A mover with a doorway through it does not.
- **Enemies do not call lifts.** They never press a use control, step on a trigger to start a mover, or open a door. A lift that moves only when a player starts it, or one a script has stopped or parked, is never ridden: enemies plan no ride on it. A lift that never started, or that a script has stopped at a dock, can still be walked across while it is docked flush with both floors.
- **Enemies ride lifts that move on their own.** A `ping_pong` lift that is started, and is not held by a script or an `auto_close_ms` countdown, offers a route between its two ends. An enemy walks onto it while it is docked, stays on through the wait, rides it, and steps off at the far end. The same holds for a room that rises into place. A lift must also rest at each stop to be ridden: a `wait_ms` of 34 or more (the threshold is two fixed ticks, about 33.33 ms), or a `Once` mode that ends each leg docked. A started lift with a shorter wait, such as `wait_ms 33`, never docks long enough to be ridden and is plain ground to enemies: they are carried on it but never ride it or plan a route through it. A lift is routed and ridden only if the map bakes a link for it: at a dock, the deck must sit within the agent's step height of the landing floor, and its edge must lie within the agent radius plus about two and a half nav cells of the landing. A lift docked lower or further away than that gets no link, so enemies neither route over it nor ride it. An enemy riding a lift does not walk off it mid-shaft, and a lift that leaves before an enemy steps on does not draw it into the shaft.
- **Spinners get no routes.** A mover with a spin axis set, even one only a script spins, gives enemies no door or lift route, because it has no single pose to plan around. Collision and block policy still apply to it.

A closed door stops an enemy's movement, its line of sight and its shots. It also stops players' shots and blasts, for the host and every connected player alike. An open gap lets all of them through.

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

### Lighting

A mover changes the baked lighting around it as it travels. The compiler bakes what each mover does to the level's baked light with the mover at a series of **poses** along its path: one at each end, its first and last waypoint, and more between them, about one probe spacing apart. As the mover travels, the engine blends between the two poses either side of it. Close a door and the light that spilled through the doorway fades out, the dark room behind it stays dark, and a character behind the door goes dark. Open it again and the light ramps back in with the door's travel. Raising a room out of the floor works the same way: its interior stops taking the arena's light as its walls rise.

What follows the mover:

- Baked light from static and animated lights on characters, props and the mover's own faces.
- Bounced light from static lights, on every surface, on fog and on water.
- How the lighting sees past the mover, so light no longer leaks through a closed door.

What does not, yet:

- Lightmaps on static walls and floors. A character on the floor behind a closed sliding wall goes dark, but the lightmapped floor under them stays lit by the same light.
- Bounced light from animated lights.
- Water reflections, which show the level with every mover absent: a pool behind a closed door still mirrors the lit doorway.
- Exact lighting between poses. Between two poses the lighting blends, so a change the mover makes spreads over about one pose spacing of travel. A mover that stops partway along its path shows the blend of the poses either side, not a bake at that exact spot.
- Spinning. A mover with a `spin_axis` changes no lighting.

Movers in co-op light the same on every machine, with nothing extra sent over the network. A mover that changes no lighting adds nothing to the compiled map. **Rebuild the map** after you change a mover, its path or the level around it.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `light_ramp_start` | float | `0` | Fraction of the path, from `0` (first waypoint) to `1` (last waypoint), where the lighting starts to change. Until the mover passes it, the first waypoint's lighting holds. |
| `light_ramp_end` | float | `1` | Fraction of the path where the lighting finishes changing. Past it, the last waypoint's lighting holds. Must be greater than `light_ramp_start`. |
| `light_bounces` | integer | every bounce | How many bounces of static light the compiler re-bakes with this mover in place. `0` re-bakes no bounce. Leave it off to re-bake every bounce the level bakes; a larger number does the same. `--bake-fidelity low` re-bakes at most `1`. |
| `light_poses` | integer | automatic: one per probe spacing (2 to 16), plus geometry poses where the mover meets each probe | How many evenly spaced poses the compiler bakes along the path, counting both ends. `2` bakes only the two ends. Leave it off and the compiler places poses itself, including geometry poses wherever the mover's brushes reach a probe; a number you set places exactly that many evenly spaced poses and no geometry poses, and may go past 16. A pose that would park a face or seam of the mover right where it crosses a lighting probe moves the least distance needed to clear it. A face that runs along a row of probes, just off them, gets a warning instead (see **A face running along probes** below). |

**How many poses.** Without `light_poses`, a mover bakes one even pose per probe spacing of path length, plus one: ⌈L ÷ s⌉ + 1, where L is the length of its whole path and s is the level's probe spacing (`--sh-probe-spacing`, 1 m by default; 1 m is about 39 map units). It bakes at least 2 and at most 16 even poses. A door that slides 128 units (about 3.25 m) bakes 5 even poses at the default spacing. The poses sit evenly by distance along the path, wherever its middle waypoints fall, and the ends always sit on the first and last waypoint. A path with no length bakes its two ends, both in the same place.

It then adds poses where the lighting changes sharply. The bake treats a probe as enclosed while the mover's brushes fully contain it. Wherever the mover starts or stops enclosing a probe, the compiler adds a pose just outside and one just inside that point of travel, so a surface the mover carries past a probe changes its lighting as the surface passes, not spread over the whole step between even poses. These extra poses where the mover meets probes are its **geometry poses**. A slab that moves straight through a row of probes adds about four geometry poses per row it crosses. Geometry poses are never cut to fit a count, and the Build Summary shows each mover's even and geometry counts. An even pose that lands right beside a geometry pose merges into it, so the Build Summary's even count can be lower than ⌈L ÷ s⌉ + 1. Their cost counts against `--working-set-max-size` like any other pose.

A path that would need more than 16 even poses bakes 16, spread over the whole trip further apart, so its lighting changes blend over more travel. The compiler warns, naming the mover, how far apart its poses ended up and `light_poses`. Set `light_poses` to choose the count yourself: higher for a long lift whose lighting you want to follow closely, or `2` for a mover whose lighting only matters at its ends. A count you set turns off the geometry poses, so it trades lighting accuracy for bake time. More poses cost bake time. Where a pose adds nothing over the blend of the poses either side of it, the compiler leaves most of it out of the compiled map, so poses over steady lighting cost little space.

`light_poses` must be a whole number, `2` or more; `0`, `1`, a negative, fractional or non-numeric value stops the build with an error naming the mover. So does a count past 16777217, the most poses a path can tell apart. If the poses of every mover with `light_poses` set together need more memory than `--working-set-max-size` allows, the build stops too, naming those movers, the largest first. Once the compiler has placed every mover's poses, geometry poses included, it checks them all together the same way. The compiler never bakes fewer poses than you set. To return to the automatic count, remove the key or leave its value empty.

**A face running along probes.** When a face of a mover runs within a few millimetres of a row of lighting probes without passing through them, such as a sliding wall whose front slides just in front of a probe layer, the compiler warns once for that mover. Poses along that stretch bake part of the mover's solid into those probes, whether you set `light_poses` or not, and the compiler leaves them where they are. The warning names the mover, how many probes the face runs along, the nearest one's position and its distance from the face, and the stretch of the path. Move that face off the probe layer, either way, by at least the distance the warning gives (about 3 mm, an eighth of a map unit; one map unit is plenty). The position is in engine metres, with y up: in map units, x is −z ÷ 0.0254, y is −x ÷ 0.0254 and z is y ÷ 0.0254.

**Shaping the ramp.** Most movers need no ramp: the poses already place each change where it happens along the path. The ramp overrides that. Until the mover passes `light_ramp_start`, the first waypoint's lighting holds, whatever the poses there would show. Past `light_ramp_end`, the last waypoint's lighting holds. Between the two, the lighting runs from the first waypoint's through the poses baked inside that stretch to the last waypoint's. A mover whose first-waypoint lighting should hold until late in its travel might use `light_ramp_start` `0.8`. The ramp measures distance along the whole path. Both keys must lie between `0` and `1`, with the start below the end. Anything else, or a value that is not a number, stops the build with an error naming the mover.

**Bake cost.** Re-baking bounced light is the expensive part. At `high` fidelity (below), each end of each mover's path costs about as much as baking the level's whole bounced lighting once, however small the mover. Each pose between the ends re-bakes only the region around the mover's path, so it costs less than an end, but a long path has many. A map with a few movers can take several times longer to compile than the same map without them, and a large map with many movers far longer. After the first build, editing one mover re-bakes only that mover, and changing its `light_poses` or moving one of its middle waypoints keeps the bake of its two ends. Editing a static light or static geometry re-bakes every mover's bounce. You have two levers: bake fidelity for the whole build, and `light_bounces` per mover.

**Bake fidelity.** `--bake-fidelity` sets how fully movers re-bake their bounced light:

- `low`, the default for everyday builds, re-bakes only the first bounce at every pose, as if every mover had `light_bounces 1`, and keeps the later bounces from the level with the mover absent. It is much faster, and it is a preview. Where a mover's lighting depends on light bouncing more than once, such as the dark side of a closed door or the inside of a sealed room, `low` can read noticeably brighter or darker than the shipped map.
- `high` re-bakes each mover's `light_bounces`, exactly as the shipped map will. Build with it before judging a mover's bounce lighting.
- `--release` always bakes `high`. Naming `--bake-fidelity` beside `--release` stops the build with an error, so a shipped map never bakes `low`.

A `low` build reuses a mover's `high` bake from the cache when that mover and the level around it have not changed. After one `high` build, everyday `low` builds show `high` lighting on every mover you have not touched, and preview only the ones you edit. When `prl-build` opens its setup screen, its **Mover bounce fidelity** field sets the same thing, and the **Production** build mode always bakes `high`.

**`light_bounces`.** Pick it per mover:

- Leave it off for a mover whose bounce you can see: a door sealing a lit room from a dark one, or a room that rises in the middle of a lit space.
- `1` keeps the first bounce, light bouncing once off the mover and the surfaces around it, which is most of what you see, for less than half the cost.
- `0` skips bounce for that mover entirely, at either fidelity. Its direct light and how the lighting sees past it still follow the mover, but the bounce around it stays as if the mover were absent, so a closed door can still let a faint glow of bounced light through. Use it for small or out-of-the-way movers.

`light_bounces` must be a whole number, `0` or more; a negative, fractional or non-numeric value stops the build with an error naming the mover, as does one too large to count. To return to the default, remove the key or leave its value empty.

**What the Build Summary shows.** A build that re-bakes mover lighting prints a mover fidelity block: the build's fidelity, how many movers it shows short of `high`, and one line per mover with:

- its pose count, split into even poses and geometry poses: `N pose(s) (E even, G geometry)`;
- `high`, or `low, approximate` with how many of its poses baked fewer bounces than its `light_bounces`. A mover reads `high` whenever it looks as it would in a `high` build: reused from a cached `high` bake, or at `light_bounces` `1` or `0`;
- an estimate of how long its bounced light takes to bake in a `--release` build, `none` at `light_bounces 0`, or `—` while the compiler has no record of how long this level's bounced lighting took to bake;
- for a mover with poses between its ends, how many of the lighting states baked at those poses the compiler left out because they matched the blend of their neighbours.

The Build Summary's `Mover lighting` lines also name each mover that blocks a light only partway along its path, not at either end, with a count of those lights. That light still follows the mover.

---

## Enemies Out of Sight

Enemies no player can perceive run their AI at a lower rate, so a map can hold hundreds of them without slowing the game. An enemy counts as perceived when it stands in a cell a player's cell can see, or within the level's AI wake lead of one. Every player counts, host and clients alike, including dead or downed ones. An unperceived enemy with no target runs its whole AI tick twice a second and checks for targets every 2 s. A perceived or engaged enemy decides 15 times a second and moves every tick, so play in view is unchanged. One that wakes moves up on that tick without catching up movement, so it never jumps.

Things that still reach an unseen enemy every tick: lifts carry it, doors push it, and damage, an opened aggro gate or a target wakes it.

### Keys

| Key | On | Effect |
|-----|-----|--------|
| `ai_wake_lead` | `worldspawn` | Metres past each player's cell in which enemies still count as perceived. Default 16. A value above the level's baked visibility reach is capped to it, with a warning. `0` keeps only the cells players can see. A negative or non-finite value fails the build. |
| `always_active` | an enemy placement | `1` keeps this enemy at full rate even when no player can perceive it; `0` lets it go dormant even if its archetype sets `alwaysActive`. Leave it unset to follow the archetype. Any other value fails the build. Enemies a spawner releases always follow the archetype. |

### Tips

- **Give escorts, racers and scripted walkers `always_active 1`.** An unseen dormant enemy steps in half-second strides, so it can reach a trigger a fraction of a second late. One with `always_active` crosses it on time. It costs full rate for that enemy only.
- **Monster closets need nothing special.** A closet enemy stays dormant until a player can perceive it, then wakes at once.
- **Doors do not hide enemies.** Perception ignores doors, so a closed door never puts an enemy to sleep early.
- **Ghost links count as sight.** Enemies seen through an armed ghost link stay perceived.
- **An enemy in view that moves in half-second strides marks a visibility gap.** The same spot will show lighting or probe problems too, because enemies use the visibility the lighting uses. Fix the level's visibility there; there is no separate AI visibility bake.

## Teleporters

A `trigger_teleport` brush sends a player who walks into it to a
`teleport_destination` point: either exactly there (`classic`), or carried
relative to a pair of points with their offset, view and momentum kept
(`seamless`), for shifts between copies of a room. Scripts turn teleporters on
and off. Keys, room-copy layout, firing rules and scripting are in
[docs/teleporters.md](teleporters.md).

---

## Level Seams

A `level_seam` brush joins two maps: a player who walks into it crosses into the
next map at the same place relative to the seam, facing and moving the same
way, with their gear. Build the crossing in both maps with one shared seam
`name`, and set each seam's `target` to the other map's catalog id. Keys,
landmarks, what crosses, seam checkpoints and scripting are in
[docs/level-seams.md](level-seams.md).

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
and `stop` switch the repeats. One trigger can drive movers, traps, spawners
and attack pools sharing a tag; each takes only the commands it understands
(movers `start`, `stop`, `reverse`, `go_to_path_node`; traps `fire`, `start`,
`stop`; spawners `fire`, one batch each; attack pools `start`, `stop`). A tag
that only reaches things that ignore the command warns at level load. Triggers
act when a player enters; to stop a trap when the player leaves, use a second
trigger or a script. Scripting:
[scripting-reference.md](scripting-reference.md#projectile-trap-members).

---

## Attack Pools

An `ai_attack_pool` point entity gives an arena its own attack budget: while it
runs, the enemies it assigned take turns under a budget profile your mod
declares (`attackBudgets`, see
[scripting-reference.md](scripting-reference.md#attack-tokens-and-budgets)),
instead of the default pool every other enemy shares. Where it sits does not
matter; triggers and scripts reach it by its tag, as they reach a relay.

| Key | Meaning |
|---|---|
| `member_tag` | Required. One tag, no spaces. The tag its members carry: map-placed enemies' `_tags`, or a spawner's `spawned_tags`. |
| `budget_profile` | The profile its members take turns under. Blank means `default`. |
| `_tags` | Tags a trigger or script addresses it by. |

- **Starting** a pool assigns every enemy carrying its `member_tag`. While it
  runs, an enemy a spawner releases with the tag joins as it spawns. Starting a
  pool that is already running assigns its current tag holders again, so one
  trigger returns a group from a "frenzy" pool to "calm".
- **Stopping** it returns its members to the default pool.
- Each enemy is in one pool at a time, and the latest start wins: start a
  "frenzy" pool over a "calm" pool's guards to escalate them without stopping
  "calm". Stopping "frenzy" sends them to the default pool, not back to
  "calm". An enemy spawned with tags matching two running pools joins the one
  started latest. Retagging an enemy later moves no one.
- Pools run side by side, so co-op players in two arenas each fight under their
  own arena's budget. No pool runs when a level starts.
- Only the host runs pools; clients never see them.

**Without a script**, point a `trigger_volume` or `switch` at the pool's tag
with `command` `start` or `stop`. A trigger's start acts before that tick's
attacks are decided. Scripting:
[scripting-reference.md](scripting-reference.md#attack-pool-members).

```
{
"classname" "ai_attack_pool"
"origin" "0 0 0"
"_tags" "arena"
"member_tag" "arena_guard"
"budget_profile" "arena"
}
```

An empty `member_tag`, or two pools sharing a `_tags` tag, fails the build
naming the entities. A `budget_profile` your mod does not declare warns when the
level loads and uses `default`.

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

### `stream_split_volume`

Use this around a doorway or portal where early SH warm-up is useful. The brush
must pass through positive portal area and have interior on both sides of the
portal plane. Portals exist only where the compiler divides the level into
cells: across doorways a door seals, through openings in large walls, past the
free ends of walls, and on its cell-size grid. Small props and pillars never
make portals. A split that crosses none of these fails the build, naming the
volume. The volume never changes cells, portals or what is visible.
It forces a cluster boundary across the matched portals. When
the near-side cluster is visible, the runtime planner prefers warming the far
side even if an opaque door currently blocks render visibility.

This is best-effort lighting preparation, not a door gate: opening a door
before the asynchronous install and SH compose finish still uses the normal
ambient-floor miss fallback.

### `stream_resident_volume`

Use this around lighting that should remain warm, such as a focal room or
transition. Every cluster containing a runtime cell whose AABB overlaps the
brush by positive volume is pinned. Pins are never pressure-suppressed or
selected as eviction victims, so they raise the level's mandatory SH memory
(the probe memory the level must always keep loaded). Build Summary reports it
as "SH mandatory memory" and warns when it is too high; many or large pins raise
it, so pin only what must stay warm. Lightmaps in pinned cells stay loaded too,
so pins also raise what Build Summary reports as "Lightmap pool memory".

### `stream_priority_region`

Set `_stream_priority` to an integer from `0` through `3`; blank and `0` mean
no priority hint. Positive values take the maximum where regions overlap and
rank only optional split warm-up and nearby prefetch work, ahead of distance
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
| `glass` | Glass |
| `wood` | Wood |

If the engine doesn't recognize a prefix, it falls back to a default material and logs a warning.

The prefix never decides how strongly a texture glows: that is its `materials.toml` entry (see Emissive Maps below).

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

### Highlight Sharpness and Tint

Two more optional siblings shape the highlight texel by texel, so one texture can mix matte stone with polished inlay, or white-glinting marble with bronze trim:

- `{diffuse}_p.png` — **smoothness**: how tight each texel's highlight is.
- `{diffuse}_t.png` — **tint**: how much each texel's highlight takes the colour of its diffuse.

Example: `metal_panel.png` → diffuse; `metal_panel_s.png` → highlight brightness; `metal_panel_p.png` → sharpness; `metal_panel_t.png` → tint.

Use either on its own or both together. Neither replaces `_s.png`: `_s` still sets how bright the highlight is, and without one the surface draws no highlight at all.

#### Smoothness: `_p.png`

Black is the broadest highlight, white the sharpest. The compiler stores your byte as is, except that 0 is stored as 1 (it shades the same, since bytes 0–24 all give exponent 1). The engine turns the byte into a highlight size:

r = 1 − p/255, exponent = 2/r⁴ − 2, clamped to 1–8192

| `_p` byte | Highlight exponent | Reads as |
|-----------|--------------------|----------|
| 0–24 | 1 | Broadest; a soft sheen across the whole face |
| 64 | about 4 | Rough stone |
| 128 | about 30 | Satin |
| 160 | about 100 | Polished |
| 192 | about 535 | Glossy |
| 224–255 | 8192 | Mirror-tight pinpoint |

Every byte from 224 up shades at the cap. A highlight that tight is already as small as one texel can show.

Without a `_p.png`, the texture keeps its **prefix's** highlight size, exactly as before. These are the bytes that match each prefix, if you want a `_p` map that starts from the prefix's look:

| Prefix | Exponent without `_p` | Matching `_p` byte |
|--------|-----------------------|--------------------|
| `concrete` | 4 | 61 |
| `grate` | 8 | 84 |
| `wood` | 16 | 108 |
| anything else | 32 | 129 |
| `metal` | 64 | 149 |
| `glass` | 96 | 159 |

#### Tint: `_t.png`

A texture **without** `_t.png` glints the way it always has: the highlight is multiplied by the diffuse colour, so a dark green marble glints dark green.

A texture **with** `_t.png` takes its highlight colour per texel instead:

- **0 (black)**: a white highlight, as bright as `_s` says, on any diffuse.
- **255 (white)**: the diffuse's **hue at full brightness**. A dark bronze and a bright bronze glint identically, so you can darken a metal's diffuse without dimming its glint.
- **In between**: a blend from white to that hue.
- A grey, white or black diffuse texel has no hue, so it glints white at any tint.

Tint changes only the highlight. The diffuse never changes, however you paint `_t`. That is deliberate: a surface must still read correctly where no reflection is drawn.

A texture with `_t.png` and no `_p.png` keeps its prefix's highlight size.

#### Rules for `_p` and `_t`

Each of these **fails the compile**:

- **Linear color space.** No `sRGB` chunk, no `iCCP` chunk, `gAMA` ≈ 1.0, the same rule as `_s.png`, `_n.png` and `_h.png`. A color PNG compiles: the compiler reads the red channel of any color type. A packed ORM (occlusion, roughness, metallic) export has ambient occlusion in its red channel, not roughness, so split the channels first.
- **Same dimensions as the diffuse**, and as `_s.png`, `_h.png` and each other when those exist. They are packed into one texture with the specular and height maps, so they have to line up texel for texel.

A texture with `_p` or `_t` compiles a specular texture four bytes per texel instead of one or two. Only textures that use them pay for it. TrenchBroom's texture browser hides `_p` and `_t` files, like the other sibling maps.

Both the compiler and TrenchBroom recognize sibling maps by the end of the file name, ignoring case. So don't give a diffuse texture a name ending in `_p` or `_t` (or `_s`, `_n`, `_e`, `_h`): TrenchBroom hides it from the browser, and the compiler treats it as a sibling map (a colour-tagged `_p` or `_t` file fails the compile).

They apply to world brushes and `kinematic_mover` brushes. `prop_mesh` models load only their diffuse, and sprites and fluids ignore both.

No tool generates either map: sharpness and tint are artistic choices that no prefix rule can guess.

#### Converting from PBR roughness and metallic

If your texture tool exports PBR maps (Material Maker, Substance and most others do), convert them per texel:

- **`_p` is one minus roughness.** Invert the roughness map: `_p` byte = 255 − roughness byte. Unity's *Smoothness* is already this quantity; copy it as is.
- **`_t` is metallic.** Copy the metallic map unchanged.

The roughness here is the usual glTF "perceptual" roughness, which is what those tools write. Export both as linear grayscale at the diffuse's size. Many editors tag exports as sRGB by default, which fails the compile; turn that off or strip the tag.

There is no metallic darkening of the diffuse. In PBR a fully metallic surface has no diffuse at all. Here it keeps its diffuse, so if a converted metal looks too flat, darken its diffuse by hand: with `_t` its glint keeps its colour. The tint's hue comes from the diffuse's channel ratios (albedo divided by its brightest channel), so a diffuse darkened to a byte or two per channel quantizes the hue, and gold can read yellow. Keep a darkened diffuse comfortably above black.

#### Pixel and smooth highlights

By default, every highlight on world brushes and movers takes its shape from one point at the centre of each texel. Highlights sit still on the texture grid as the camera moves, with the same soft seams between texels that texture reads have. Shadows still cut a highlight on exactly the line they cut the lit surface, even through the middle of a texel. This applies to every texture, including ones without `_p` or `_t`: a glossy prefix such as `metal` shows texel-sized highlight blocks.

A mod can choose `"smooth"` highlights instead, through its `render.specular.style` setting (see [scripting-reference.md](scripting-reference.md), *Render profile*). Smooth evaluates every pixel, so a highlight is a round spot that slides across the surface as the camera moves. With smooth, a very sharp `_p` texel's highlight is a small dot moving inside the texel: author for the style your mod ships with. Players cannot change this; it is the mod's look.

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
- **Same dimensions as `_s.png`, if you have one.** Specular and height are packed into one texture in the compiled output (with `_p` and `_t`, when present), so they have to line up texel for texel.

#### How far it reaches

Depth character comes from the **material prefix** — the same first-token-before-the-underscore rule the Material System table above uses. You control it by naming the texture, not by tuning the PNG:

| Prefix | Depth (each direction) |
|--------|-------|
| `concrete` | Deepest — the cobblestone and pavement case this is built for |
| `grate` | Moderate |
| `wood` | Shallow (same as `metal`) — plank gaps |
| `metal` | Shallow — panel seams and rivets |
| `glass` | **Flat.** Deliberately |
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

### Emissive Maps

A `{diffuse}_e.png` beside a texture makes it glow: the forward pass adds the map's colour, times the texture's glow strength, on top of the lit surface. It must be the same resolution as the diffuse.

- **Format:** RGB colour PNG, authored as ordinary sRGB colour. Black means no glow. Keep alpha opaque; it is reserved.
- **Where:** world brushes, movers and fluids glow. Models do not; they load only their diffuse.
- **Per placement:** an [`emissive_volume`](#emissive-volumes) scales and tints the glow of the world faces inside it, and lets scripts animate it.
- **Effect:** self-illumination only. Glow lights nothing around it — place a light entity for that, or tie the glowing brushes to a [`light_area`](#light_area--area-light), which lights the room from them — but bright glow blooms. A glowing pixel blooms once its linear luminance (`_e` colour × strength) passes `1.0`.

Example: `sign.png` → diffuse; `sign_e.png` → glow colour.

`tools/gen_emissive.py` makes a starting `_e` from a diffuse, keeping only the bright texels. Every texture takes the same default cutoff (`0.75` luminance); pass `--cutoff` to keep more of a sign's pattern:

```bash
uv venv && source .venv/bin/activate && uv pip install Pillow
python3 tools/gen_emissive.py --input <content-root>/textures/signs --cutoff 0.35
```

#### Glow strength: `materials.toml`

A texture's glow strength is content, set in one file per mod: `content/<mod>/materials.toml`, beside the mod's `textures/` and `maps/`. Only the map compiler reads it; recompile the map after editing it.

```toml
# Keys are texture names as TrenchBroom shows them: "collection/name".
["signs/bar_sign"]
emissive = 4.0

# An inline table works too.
"signs/exit" = { emissive = 2 }
```

- **Defaults:** a texture with an `_e` map and no entry glows at `1.0`. A texture with no `_e` map glows at `0`, whatever its entry says.
- **Keys:** name the texture as TrenchBroom shows it, `collection/name`. Case does not matter, `\` reads as `/`, and a leading `textures/` is ignored. A key reaches every face whose texture resolves to it, whether the map names that texture with its collection (`signs/bar_sign`) or by its bare name (`bar_sign`). A key spelled as the map writes the texture (`bar_sign`) also works; if both forms have an entry, the `collection/name` one wins, with a warning when they differ. Two keys that name the same texture warn once, and the later one wins.
- **Values:** `emissive` is a number. A negative value reads `0`; a value above `64` reads `64`; each warns.
- **Mistakes never stop the compile.** Each warns, naming the file and line: a syntax error (every texture takes its default), an unknown key (ignored), a value that is not a number (that texture takes its default), and an entry for a texture with no `_e` map (ignored).

**Migrating `neon_` textures.** Older builds gave every `neon_`-prefixed texture a glow strength of `4.0`; the prefix is gone, and a former `neon_` name now reads as an unknown prefix. `tools/migrate_neon_prefix.py` keeps those looks: it renames each `neon_x` texture and its sibling maps (`neon_x_e`, `_s`, `_n`, `_h`) to `x`, rewrites the references in your maps, and writes `emissive = 4.0` into `materials.toml` for each one with an `_e` map. It lists what it would do and writes nothing unless you pass `--apply`. It refuses, writing nothing, on a name collision or when the new name would start with another material's prefix (`neon_metal_x`), so rename those by hand. It needs Python 3.11 or later and nothing else.

```bash
python3 tools/migrate_neon_prefix.py content/<mod>
python3 tools/migrate_neon_prefix.py content/<mod> --apply
```

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
| `ai_attack_pool` has an empty `member_tag` | An attack pool names no members | Set `member_tag` to the tag its enemies carry |
| … `ai_attack_pool` … share `_tags` … | Two attack pools carry the same tag, so a trigger on it would reach both | Give each pool its own tag |
| `ai_attack_pool` has a `member_tag` with whitespace | A member tag is one tag, so a space-separated list matches nothing | Set `member_tag` to a single tag with no spaces |
| `light_ramp_start` and `light_ramp_end` must satisfy … | A mover's ramp keys are out of order or outside `0`–`1` | Keep both between `0` and `1` with the start below the end, or remove them (see Movers: Lighting) |
| `light_ramp_start` (or `light_ramp_end`) value … is not a valid float, or … is not finite | A mover's ramp key is not a number, or is `nan` or `inf` | Write a number between `0` and `1`, or remove the key |
| `light_bounces` must be an integer of at least 0 | A mover's `light_bounces` is negative, fractional or not a number | Use a whole number, `0` or more, or remove the key or leave it empty |
| `light_bounces` … is too large | A mover's `light_bounces` is past 4294967295 | Use a smaller whole number: any count past the level's bounces already re-bakes them all |
| `light_poses` must be an integer of at least 2 | A mover's `light_poses` is `0`, `1`, negative, fractional or not a number | Use a whole number, `2` or more, or remove the key or leave it empty for the automatic count |
| `light_poses` … is too large | A mover's `light_poses` is past 16777217, the most poses a path can place; past it, neighbouring poses would fall on the same point of travel | Lower `light_poses` |
| … `light_poses` pose … within the band of a face or seam of the mover crossing it … | Two or more of the poses you set fall where one face of the mover passes through a lighting probe, or one falls there right next to an end of the path, so no pose can move clear of it without passing its neighbour | Set a lower `light_poses` so the poses lie further apart, set it to `2` to bake only the ends, or remove it for automatic placement |
| `--release` always bakes high fidelity; remove `--bake-fidelity` or `--release` | The build names both flags | Drop `--bake-fidelity` to ship, or drop `--release` for an everyday build |
| `--bake-fidelity` must be `low` or `high` | Any other value after `--bake-fidelity` | Use `low` or `high`, or leave the flag off for `low` |
| mover poses' working set refused before the SH base gate … | The poses of every mover with `light_poses` set together, or every mover's placed poses with geometry poses included, need more memory than `--working-set-max-size` allows (16 GiB by default), before the rest of the lighting bake is counted. The error names those movers, the largest first, with each one's pose count and bytes | Lower `light_poses` on the named movers (a set count also places no geometry poses), or raise `--working-set-max-size` if the machine has the memory |
| SH base working-set gate refused before patch shading: estimated peak … exceeds budget … | The indirect lighting bake, including every pose of every mover, needs more memory than `--working-set-max-size` allows (16 GiB by default). On a map with movers it charges `--mover-payload-max-size` in full, three times over while the movers' lighting is assembled, and the error says so | Raise `--working-set-max-size` if the machine has the memory. Movers with many poses add to it, so a smaller `light_poses` on a long mover also helps. A `--mover-payload-max-size` raised past what the movers need costs three times the raise here; lower it to the Build Summary's block bytes plus headroom |
| MoverProbeChannel (id 57): the bounce and depth blocks would hold … past --mover-payload-max-size … | The movers' baked indirect lighting needs more memory than `--mover-payload-max-size` allows (256 MiB by default). The error names the mover whose lighting would pass the cap and the movers before it | Use `light_bounces 0` on the largest movers, fewer or smaller movers, or raise `--mover-payload-max-size`. Everyday builds bake one bounce per pose and `--release` bakes every bounce, so a shipped build can need more than an everyday one. Raising the cap raises the memory the SH base working-set gate predicts by three times the raise |
| Probes carry closed-pose depth, past the probe depth tile range | Movers reach into the lighting of more probes than the level can track | Use fewer movers, or move them where they block less of the level's light |
| PNG color-space validation failed | An `_s.png`, `_n.png`, `_h.png`, `_p.png` or `_t.png` carries an `sRGB` or `iCCP` chunk, or a `gAMA` that isn't ≈ 1.0 | Re-export the named files as linear PNG with no color-management metadata. The error lists every offending path |
| `_h.png` dimensions must match diffuse | A height map is a different resolution from its diffuse texture | Re-export the height map at the diffuse's exact dimensions |
| `_h.png` and `_s.png` share one specular slot and must have identical dimensions | A height map and its specular sibling disagree | Match them. The two are packed into one texture and cannot be resized independently |
| `_p.png` dimensions must match diffuse / `_t.png` dimensions must match diffuse | A smoothness or tint map is a different resolution from its diffuse texture | Re-export it at the diffuse's exact dimensions |
| `_p.png` and `_s.png` (or `_h.png`, `_t.png`) share one specular slot and must have identical dimensions | A smoothness or tint map disagrees with another sibling that is packed into the same slot (`_s`, `_h`, or each other) | Match all of them. They are packed into one texture and cannot be resized independently |
