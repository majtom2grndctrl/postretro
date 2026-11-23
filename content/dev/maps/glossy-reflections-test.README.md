# glossy-reflections-test: glossy faces of every facing around a freestanding monument

A sealed map for the `glossy-world-reflections` feature. Its main room holds
a glossy floor with a non-glossy pillar and a small freestanding monument
standing on it, a glossy wall, a glossy down-facing ceiling over an animated
`light_area` panel on the floor, and a 45° glossy ramp. A side wing south of
it holds four small rooms that art-direct reflection through materials
alone. Glossy placement runs only when `prl-build` is given
`--glossy-reflections`; without it the map bakes as if no material reflected.

Reflectivity is art-directed per texel through `_p` and `_s`: a texel
reflects only where its `_p` sharpness passes the reflection floor (exponent
above 8, fading in to full weight at 16) and its `_s` is above zero, so an
author picks which surfaces reflect by their materials, and a player can turn
the feature off with the reflection quality option (Off).

## Build and run

From the workspace root:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/glossy-reflections-test.map \
  --glossy-reflections -o content/dev/maps/glossy-reflections-test.prl
cargo run -p postretro-tool -- run --install-root . maps/glossy-reflections-test.prl
```

`postretro-tool run` passes `--mod`, so its map path is relative to the mod
folder (`content/dev`). The `.prl` is a local build artifact; do not commit it.
Under `-v` the build logs the glossy census line, the glossy faces that take
probes by facing, and each probe's box, capture point and faces.

## Layout

TrenchBroom units, Z up (1 unit = 0.0254 m), Valve 220 texture axes. The
main room's interior spans x 0–512, y 0–512, z 0–256 (13 × 13 × 6.5 m), with
walls 32 thick. You spawn at (64, 64) in the south-west, facing the monument.
A 32-unit doorway in the south wall at x 384–416, in line with the pillar,
leads to the side wing.

| Surface | Where | Texture | Reflects |
|---|---|---|---|
| Floor | z 0 | `test/metal_smoothness_ramp`, scale 8 | yes: a `_p` ramp and `_s` 255 |
| North wall | y 512 | `test/concrete_carved_tinted_gloss`, scale 8 | yes, with `_t` |
| Ceiling underside | z 256 | `test/concrete_carved_gloss`, scale 8 | yes, facing down |
| Ramp slope | x 0–96, y 352–448, from z 0 at x 96 up to z 96 at the west wall, facing east and up at 45° | `test/glass_gloss_tiles`, scale 8 | yes |
| Ramp's other sides, other walls | x 0, x 512, y 0 (y 0 with the doorway at x 384–416) | `50-free-textures/concrete_stone_021`, scale 1 | no: `_s` and no `_p` |
| Pillar | x 384–416, y 352–384, z 0–256 | `concrete_stone_021` | no |
| Monument | plinth x 224–288, y 224–288, z 0–16; column x 240–272, z 16–96; cap x 232–280, z 96–104 | `50-free-textures/concrete_pavement_036`, scale 1 | no |
| Floor panel | `light_area`, x 96–160, y 160–224, z 0–4; its top `neon/glow_panel` | sides `concrete_pavement_036` | no |

Two static `static_light_map` point lights, white, `_light_size 0`: one at
(256, 128, 200), `light` 300, range 768; one at (160, 400, 180), `light` 250,
range 640. The floor panel is an animated `light_area`, `light` 150, pulsing
1.0 → 0.25 → 1.0 over 1600 ms.

### Side wing

Four rooms of the same shell, each 256 units square (6.5 m) and 256 tall,
z 0–256, open off a hall that runs x −608–1120, y −96–−32, z 0–256. The
hall, its floor and its ceiling are `concrete_stone_021`. Each room's door is
a 64-unit gap at the middle of its north wall. Each room has three static
lights: a `static_light_map` point light (white, `light` 200,
`_falloff_range` 512, `_light_size` 0) at (room centre x, −320, 160), toward
its south wall; a ceiling `light_area` panel (`light` 80), 64 units square
at the centre, z 248–256, `neon/glow_panel` facing down; and a dim
`light_area` glow band (`light` 10) across the foot of the south wall,
y −384–−376, z 0–48, `neon/glow_panel` facing north, whose reflection reads
on a glossy floor at grazing angles.

| Room | Interior | Floor | Walls, ceiling | Intent |
|---|---|---|---|---|
| Fade-band | x −608–−352, y −384–−128 | `test/metal_fade_band`, scale 8 | `test/concrete_satin`, scale 8 | the floor reflects at about half weight and blurrier than the floor-only room's; satin walls highlight but never reflect |
| Floor-only | x −320–−64, y −384–−128 | `test/metal_polished`, scale 8 | `test/concrete_satin`, scale 8 | only the floor reflects; satin walls and ceiling highlight but never reflect |
| Highlights-only | x 576–832, y −384–−128 | `test/metal_tint_inlay`, scale 8 | north wall and ceiling `test/concrete_satin`, scale 8; east and west `test/concrete_grimey`, south `test/concrete_dark_speckled`, scale 1 | every surface highlights, none reflects: no probe and no overlay draw |
| Tile-and-grout | x 864–1120, y −384–−128 | `test/concrete_tile_grout`, scale 8 | `test/concrete_satin`, scale 8 | the tiles reflect; the grout, painted through `_s` 0 alone, stays matte |

The wing's new materials are `tools/gen_specular_texel_fixtures.py`'s
(texel values in its docstring). Exponents are n = 2 / r⁴ − 2 at
r = 1 − p / 255:

| Material | `_s` | `_p` | `_t` | Reflects |
|---|---|---|---|---|
| `test/concrete_satin` | 255 | 64 (n ≈ 4.4) | — | no: under the floor |
| `test/metal_polished` | 255 | 200 (n ≈ 922) | 0 (white) | yes, full weight at the minimum blur |
| `test/metal_fade_band` | 255 | 97 (n ≈ 11.6) | 0 (white) | yes, about half weight (the log₂ fade) at about mip 3.8 |
| `test/concrete_tile_grout` | tiles 255, grout 0 | 200 everywhere | 0 (white) | tiles yes, grout no |
| `test/metal_tint_inlay` | 255 | — | inlay | no: without `_p` it highlights at `metal`'s 64 only |

The doorway reshaped three main-room brushes. The floor and ceiling start
at y 0 instead of under the south wall, so neither glossy surface shows in
the doorway. The south wall splits at the doorway; its east piece runs on to
x 960, so it stays too long for the floor probe's capture exclusion. The hall
floor and ceiling run under the doorway. Every wing plane that crosses the
main room's span is the pillar's x 384 or x 416, so the main room keeps its
seven probes, their boxes and their capture points.

## What the build places

The fixture materials are `tools/gen_specular_texel_fixtures.py`'s; see
`specular-texel-sharpness.README.md` for their texel values. The pillar, the
monument, the panel and the ramp cut the floor, and the pillar cuts the
ceiling and the north wall, into several coplanar faces. Placement groups
them by facing, plane and fitted box, so the build places seven main-room
probes, each 2 mm off its faces along their normal:

- the floor: one for most of the room, and one for the strip north of the
  pillar, whose box fit lands on the pillar's north face;
- the ceiling: the same two, captured 2 mm under it;
- the north wall: one for most of it, and one for the stretch behind the
  pillar (x 384–512), whose box stops at the pillar's north face;
- the ramp's slope: one, its box grown to hold the slope.

The ceiling's probe stores the panel's animated light share on its −Y lobe and
none on +Y; the floor's the reverse.

The main floor probe captures from the floor's area centroid, which falls
on the monument's plinth and moves just off its south edge. Up-facing
probes keep water's capture exclusion: the monument's brushes and the other
brushes small against the floor leave the main floor probe's capture (`-v`
logs 6 excluded brushes) and reflect in the floor only through its
screen-space layer. The floor behind the monument
keeps its probe term, and no "sees under half" warning appears. Wall,
ceiling and slope probes capture every brush, so the north wall's term is
withheld behind the pillar.

The doorway's jamb planes, x 384 and x 416, cut the main room's faces into
more pieces than before, each bound to the same probe, so the main room binds
35 faces: floor 11 and 1, ceiling 10 and 1, north wall 9 and 2, slope 1.

The side wing adds three probes, one per reflecting room floor, each binding
that floor's five faces: ten probes and 50 bindings in all under `-v`. No
wall or ceiling in the wing takes a probe, and the highlights-only room takes
none. The fade-band and tile-and-grout floor probes each exclude one brush,
the 128-unit stretch of hall wall beside their door; the floor-only probe
excludes none. The ceiling panels stay in each floor probe's capture, since
their glowing undersides lie over the floor.
