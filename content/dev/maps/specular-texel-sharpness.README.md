# specular-texel-sharpness: per-texel sharpness, tint and texel-centre highlights

One sealed room for the manual rows M1–M8 of the
`specular-texel-sharpness` brief (`context/plans/in-progress/specular-texel-sharpness/`).
Its fixture materials have known texel values, and each one appears on a
static face and on a parked mover under the same two static lights. Three more
things sit in the room: a carved door set into a wall of the same material, a
shelf whose spot shadow crosses the glossy ramp, and a strip of three shipped
textures for the before/after captures.

## Build and run

From an SDK bundle of this workspace, at the bundle root:

```bash
bin/prl-build content/dev/maps/specular-texel-sharpness.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o content/dev/maps/specular-texel-sharpness.prl
bin/postretro-tool run maps/specular-texel-sharpness.prl
```

From the workspace root:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/specular-texel-sharpness.map -o content/dev/maps/specular-texel-sharpness.prl
cargo run -p postretro-tool -- run --install-root . maps/specular-texel-sharpness.prl
```

`postretro-tool run` passes `--mod`, so its map path is relative to the mod
folder (`content/dev`). The `.prl` is a local build artifact; do not commit it.
The build takes a few seconds and should end with `Warnings: 0` and
`EntityShadowLights: selected 2 static light candidate(s)` under `-v`.

## Fixture materials

`tools/gen_specular_texel_fixtures.py` writes all ten PNGs into
`content/dev/textures/test/`. Regenerate them with it and never edit them by
hand; its test checks every texel:

```bash
python3 tools/gen_specular_texel_fixtures.py
python3 -m unittest discover -s tools/tests -p test_gen_specular_texel_fixtures.py
```

Each is 16 × 16. Siblings are linear greyscale with no colour-space chunk.

| Material | Diffuse | `_s` | Other siblings | Shades at |
|---|---|---|---|---|
| `test/metal_smoothness_ramp` | grey checker per texel, sRGB 118 and 138 | 255 | `_p`: column x stores 17·x (0, 17, …, 255) | per column: bytes 0 and 17 at exponent 1, 238 and 255 at the 8192 cap; never the `metal` fallback of 64 |
| `test/metal_tint_inlay` | dark green field (28, 64, 40), bronze inlay (150, 96, 44): a one-texel square ring on texels 2–13 and a 4 × 4 block on texels 6–9 | 255 | `_t`: field 0, ring 255 (baked as 254), block 128 | `metal`'s 64 (no `_p`) |
| `test/concrete_carved_gloss` | 4 × 4-texel cells, each a 3 × 3 grey tile (sRGB 150) with a one-texel dark mortar line (sRGB 56) where x or y is a multiple of 4 | 220 | `_h`: tile 200, mortar 56. `_p`: tile 235, mortar 30 | tiles at the 8192 cap, mortar near exponent 1 |

All three bake a four-channel `Rgba8Unorm` specular slot. Every fixture face
uses scale 8, so one texel spans 8 units (0.2 m) and one 16-texel tile spans
128 units. Every fixture face's edges sit on multiples of 8, so no face cuts a
texel. Surface Depth caps its full depth at 0.2 m, about one texel at this
scale: carved tiles rise, and mortar sinks, by a fraction of that.

## Layout

TrenchBroom units, Z up (1 unit = 0.0254 m). The interior spans x 0–768,
y 0–512, z 0–256 (19.5 × 13 × 6.5 m), with walls 32 thick. You spawn at
(128, 128) in the south-west, facing the two fixture movers.

| Surface | Where | Texture |
|---|---|---|
| Floor | z 0 | `test/concrete_carved_gloss`, scale 8 |
| North wall | y 512 | `test/metal_smoothness_ramp`, scale 8 |
| East wall | x 768 | `test/metal_tint_inlay`, scale 8 |
| South wall | y 0, with a door recess at x 272–400, z 0–144, 16 deep | `test/concrete_carved_gloss`, scale 8 |
| West wall, ceiling | x 0, z 256 | `50-free-textures/concrete_stone_021`, scale 1 |
| Shelf | north wall, x 192–448, y 416–512, z 180–188 | `50-free-textures/concrete_stone_021`, scale 1 |
| Comparison strip | west wall, 8 proud (x 0–8), z 32–160: y 32–160, 160–288, 288–416 | `50-free-textures/default_dirt_011` (Default, 32), `wood_bark_048` (wood, 16), `concrete_pavement_036` (concrete, 4), scale 1 |

### Lights

Both are static, unanimated `static_light_map` lights with `_light_size 0`,
white, linear falloff. The compiler selects both for promotion, so each owns
the static faces it reaches and promotes onto the movers.

| Light | Origin | `light` | `_falloff_range` | Aim | Tags |
|---|---|---|---|---|---|
| `light` | (272, 20, 72) | 300 | 896 | — | `point_light` |
| `light_spot` | (384, 160, 216) | 500 | 768 | `angles` −25 63 0, `_cone` 40, `_cone2` 55 | `spot_light` |

The point light hangs 20 units (0.51 m) in front of the door's west seam at
x 272, at the door's mid height. Its range covers the whole room. The spot
aims north-east, at about (528, 448, 64). Its cone holds both fixture movers,
the north wall from about x 300 east and most of the east wall. It does not
reach the door or the comparison strip, which the point light lights alone.
The tags let a capture scene's `force_promotion` hold either light's mover
crossfade.

### Movers

All three are parked: `start_on_spawn 0`, with a path of two waypoints 8 units
apart, so each stays at its first waypoint. Each brush is one box with the same
texture on every face.

| Mover | Brush | Rest waypoint | Second waypoint | Faces |
|---|---|---|---|---|
| `ramp_mover` | x 512–640, y 416–448, z 0–128 | `ramp_mover_rest` (576, 432, 64) | (576, 424, 64) | ramp; front face at y 416, 64 in front of the north wall |
| `inlay_mover` | x 672–704, y 256–384, z 0–128 | `inlay_mover_rest` (688, 320, 64) | (680, 320, 64) | inlay; front face at x 672, 64 in front of the east wall |
| `carved_door` | x 272–400, y −16–0, z 0–144 | `carved_door_rest` (336, −8, 72) | (336, 0, 72) | carved gloss; front face flush with the south wall at y 0 |

Each fixture mover's front face spans exactly one 128-unit tile, aligned with
the wall's tiles behind it. Texture projection follows world axes, so a
column or texel of the mover sits at the same x (or y) as the matching one on
the wall.

### Door beside wall

The door fills its recess, so its front face is coplanar with the wall's and
the carved lattice runs straight across. Its seams at x 272, x 400 and z 144
fall at texel 2 of a 4-texel cell, so each seam cuts through a tile: one tile
lies part on the static wall and part on the door. Under the point light, the
forward pass places that tile's specular point on the wall plane and the mover
pass at the texel's carved top height (plan.md, Corrections, Mover pass). Any
glint difference across the seam is that gap.

## Capture scenes

Two scenes for M1, identical but for `specular_style`. Both view the
comparison strip from (224, 240, 96), looking west, under the point light.
The point light's mirror highlight falls near the dirt and wood panels'
shared edge. Save each scene beside the capture output, for example in
`target/captures/`:

`specular-texel-sharpness-pixel.json`:

```json
{
  "map": "content/dev/maps/specular-texel-sharpness.prl",
  "camera": {
    "position": [-6.096, 2.4384, -5.6896],
    "yaw_deg": 180.0,
    "pitch_deg": 0.0,
    "fov_deg": 90.0
  },
  "resolution": [1280, 720],
  "output": "target/captures/specular-texel-sharpness-pixel.png",
  "specular_style": "pixel"
}
```

`specular-texel-sharpness-smooth.json`: the same, with
`"output": "target/captures/specular-texel-sharpness-smooth.png"` and
`"specular_style": "smooth"`.

Camera positions are engine metres, Y up: engine (x, y, z) is
(−y, z, −x) × 0.0254 of the map's (x, y, z). Yaw follows the map's: 0 faces
+x, 180 faces −x. Run each from the workspace root, after compiling the map
beside its source as above:

```bash
mkdir -p target/captures
POSTRETRO_SH_STREAMING=sync-proof cargo run -p postretro --features capture -- \
  --capture target/captures/specular-texel-sharpness-pixel.json
```

`specular_style` parses only once task 3 of the brief lands: until then the
capture parser rejects it as an unknown key. A 1c7fc9e3a binary has no such
key either. For the before capture, drop the key and use another output path.
That build draws today's per-pixel highlight, which the smooth scene must
match.

## Manual walk

Content/dev's mod leaves `render.specular.style` at its default, pixel. Rows
that compare styles use the capture scenes above.

| Row | Where | What to check |
|---|---|---|
| M1 | Comparison strip, the two scenes plus a 1c7fc9e3a capture | Pixel against the previous build: the dirt and wood highlights turn into texel blocks with antialiased seams, and concrete looks the same. Smooth matches the previous build. |
| M2 | North wall ramp and east wall inlay inside the spot | Strafe and dolly slowly a few metres from each wall. No highlight moves inside any texel; a highlight only steps from texel to texel. |
| M3 | Ramp wall and `ramp_mover`, inside the spot | Facing north, each tile's columns sharpen from left to right: column 0 (west) shades at exponent 1, and the two right columns at the 8192 cap. The mover's columns match the wall's at the same x. Hold position a moment so the light promotes onto the mover. |
| M4 | East wall and `inlay_mover`, inside the spot | The green field glints white. The ring glints in the bronze hue, and the block lies between white and that hue. The mover matches the wall. |
| M5 | Carved floor near the point light; the door and its frame | Each tile's top and each exposed side face holds one highlight value per texel face. Then compare the tiles cut by the door's seams, half wall and half door. This is an observation, not a gate: a visible glint difference goes to a follow-up brief. |
| M6 | Ramp, under the shelf, between about x 300 and 470 | The spot's shadow edge from the shelf crosses the ramp horizontally at about z 166, partway up the texel row at z 160–168. Find the spot's highlight on the ramp and line it up on the edge: the edge cuts the highlight on the same line as the diffuse, inside the texel. |
| M7 | From the south-west corner, looking north-east | Look at the ramp's right columns and the carved floor tiles, up to about 20 m away. Report whether high-sharpness texels sparkle as the camera moves. Not a gate. |
| M8 | Task 9 of the brief, on the Mac | Smooth's and pixel's costs at Surface Depth Off against a 1c7fc9e3a binary, and the mover shader's register count in both styles. This map is small enough to be a pilot for that run. |

## Still to verify after task 6

- Load the compiled map once in the engine: the three movers draw, and the
  fixtures shade with their per-texel sharpness and tint.
- The four-channel bake already holds: a build with the task-4 compiler wrote
  each fixture's slot 1 as `Rgba8Unorm` with the expected bytes.
- Run both capture scenes once task 3 lands.
