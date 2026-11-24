# glossy-reflections-showcase: one room built so glossy floor reflections read

A sealed map for the `glossy-world-reflections` feature, built to judge
reflection detail and blur by eye. `glossy-reflections-test` covers every
facing and the art-direction cases. Its walls are flat beige and its floors
untinted, so they reflect at 4% head-on and show little. Here the floor is
tinted, so it reflects strongly head-on. Bright, high-contrast subjects stand
on it at several distances, and twin subjects stand on a sharp and a rough
zone of the same floor.

Strength comes from the tint: a texel's head-on reflectance is
0.02 × (2 + 48 × t), so `_t` 0 reflects at 0.04 and `_t` 224 at about 0.88.
Reflectivity is still art-directed per texel through `_p` and `_s`.
Placement runs only when `prl-build` is given `--glossy-reflections`.

## Build and run

From the workspace root:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/glossy-reflections-showcase.map \
  --glossy-reflections -o content/dev/maps/glossy-reflections-showcase.prl
cargo run -p postretro-tool -- run --install-root . maps/glossy-reflections-showcase.prl
```

The `.prl` is a local build artifact; do not commit it.

## Layout

TrenchBroom units, Z up (1 unit = 0.0254 m), Valve 220 texture axes. The
room's interior spans x 0–480, y 0–480, z 0–192 (12.2 × 12.2 × 4.9 m), with
walls 32 thick. You spawn at (240, 48) facing north. A `reflection_volume`
over the interior makes one probe for the whole floor and one for the north
wall.

| Surface | Where | Texture | Reflects |
|---|---|---|---|
| Sharp floor | z 0, x 0–320 | `test/metal_mirror_tinted`, scale 2 | yes: full weight at the minimum blur, reflectance 0.88 head-on |
| Rough floor | z 0, x 320–480 | `test/metal_satin_tinted`, scale 2 | yes: full weight, blurrier (about mip 3.4), same tint |
| North wall | y 480 | `test/concrete_carved_tinted_gloss`, scale 8 | yes: cyan tiles, copper mortar |
| East wall | x 480 | Level Eleven `ConcreteWall-Hazard-Full-01_64`, scale 2 | no |
| West, south walls, ceiling | | Level Eleven `Metal-Panel_Base-003`, scale 2 | no |

Both floor zones have 5 cm texels (scale 2), close to shipped content, so
pixel style quantizes the reflection at a realistic density.

Subjects, all standing on the floor:

| Subject | Where | Texture |
|---|---|---|
| Monument | plinth x 148–212, y 196–260, z 0–16; column x 164–196, z 16–112; cap x 156–204, z 112–120 | plinth and cap `concrete_pavement_036`; column Level Eleven hazard stripes, scale 1 |
| Stripe columns (twins) | x 208–240 (sharp) and x 400–432 (rough), y 304–336, z 0–160 | `test/concrete_signal_stripes`, scale 8 |
| Glowing pillars (twins) | x 264–280 (sharp) and x 360–376 (rough), y 392–408, z 0–144 | `light_area`, `neon/status_lamp` on all four sides, `light` 20, `_glow` 1.5 |
| Kinematic block | x 16–80, y 280–344, z 0–64, parked at its first waypoint (48, 312, 32) | Level Eleven `ConcretePanel-Hazard-Full-01_64` |

The twins stand 96 (columns) and 48 (pillars) units either side of the zone
boundary x 320, at the same depth, so sharp and rough compare in one frame.
The monument stands about 5.4 m from the south wall, the columns 7.7 m and
the pillars 9.8 m, so blur growth with distance shows.

Lights: a white `light_area` band across the north wall (x 96–448, z
112–136, `light` 40, `_glow` 2) and a vertical one on the east wall (y
176–200, z 16–176, `light` 30, `_glow` 1.5), both `neon/status_lamp` on
their glowing face. Static `static_light_map` point lights: magenta
(255 60 180) at (64, 320, 150), `light` 220, range 448; cyan (60 200 255) at
(416, 208, 150), `light` 220, range 448; a white fill at (240, 128, 176),
`light` 120, range 640. The point lights put highlights on the floor, so a
capture shows whether highlights stay out of reflections.

## Materials

New materials from `tools/gen_specular_texel_fixtures.py` (texel values in its
docstring). Exponents are n = 2 / r⁴ − 2 at r = 1 − p / 255:

| Material | Diffuse | `_s` | `_p` | `_t` | Reflects |
|---|---|---|---|---|---|
| `test/metal_mirror_tinted` | neutral grey checker by 4-texel cell, sRGB 70 and 96 | 255 | 200 (n ≈ 922) | 224 | yes, full weight at the minimum blur; the neutral albedo keeps the reflection white |
| `test/metal_satin_tinted` | as `metal_mirror_tinted` | 255 | 115 (n ≈ 20) | 224 | yes, full weight, about mip 3.4 |
| `test/concrete_signal_stripes` | a 2-texel white and black checker band, then orange, cyan and white bands, each closed by a black row | — | — | — | no |

## What the build places

The census finds 3 of 8 materials reflecting and 29 glossy faces: 25 floor
faces and 4 north-wall faces. The `reflection_volume` claims them all, so the
build places two probes and 29 bindings under `-v`: one up-facing probe
binding both floor zones (25 faces) and one for the north wall (4 faces).
The floor probe excludes 7 brushes from its capture, by water's rule: the
freestanding subjects, small against the floor (the monument's three, the two
stripe columns and the two glowing pillars). The two wall light brushes stay,
since their undersides face the floor. The subjects reflect in the floor only
through its screen-space layer, at Medium and above. At Low they are absent
from the floor's reflection, as designed.

Besides the usual lightmap texel-step notes on faces near the area lights,
the build warns that portal generation found no portals (one room, so every
cell sees every other) and that one SH cluster cell is an indivisible
singleton over the primitive limit. Neither front-side, hidden-floor nor
probe-cap warnings appear.

## Capture poses

Engine metres, 90° field of view, used for the review captures:

| View | Position | Yaw | Pitch |
|---|---|---|---|
| Hero, across the floor toward the subjects | (−0.4064, 1.8288, −6.096) | 90 | −12 |
| Grazing, low along the floor toward the columns | (−1.6256, 0.3556, −6.9088) | 95 | −3 |
| Down, onto the sharp column's and pillars' contact points | (−4.4704, 3.81, −7.112) | 90 | −42 |
| Sharp vs rough, centred on the zone boundary | (−2.4384, 1.016, −8.128) | 90 | −8 |
