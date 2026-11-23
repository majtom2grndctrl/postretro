# glossy-reflections-test: glossy faces of every facing around a freestanding monument

One sealed room for the `glossy-world-reflections` feature: a glossy floor
with a non-glossy pillar and a small freestanding monument standing on it, a
glossy wall, a glossy down-facing ceiling over an animated `light_area` panel
on the floor, and a 45° glossy ramp. Glossy placement runs only when
`prl-build` is given `--glossy-reflections`; without it the map bakes as if no
material reflected.

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
interior spans x 0–512, y 0–512, z 0–256 (13 × 13 × 6.5 m), with walls 32
thick. You spawn at (64, 64) in the south-west, facing the monument.

| Surface | Where | Texture | Reflects |
|---|---|---|---|
| Floor | z 0 | `test/metal_smoothness_ramp`, scale 8 | yes: a `_p` ramp and `_s` 255 |
| North wall | y 512 | `test/concrete_carved_tinted_gloss`, scale 8 | yes, with `_t` |
| Ceiling underside | z 256 | `test/concrete_carved_gloss`, scale 8 | yes, facing down |
| Ramp slope | x 0–96, y 352–448, from z 0 at x 96 up to z 96 at the west wall, facing east and up at 45° | `test/glass_gloss_tiles`, scale 8 | yes |
| Ramp's other sides, other walls | x 0, x 512, y 0 | `50-free-textures/concrete_stone_021`, scale 1 | no: `_s` and no `_p` |
| Pillar | x 384–416, y 352–384, z 0–256 | `concrete_stone_021` | no |
| Monument | plinth x 224–288, y 224–288, z 0–16; column x 240–272, z 16–96; cap x 232–280, z 96–104 | `50-free-textures/concrete_pavement_036`, scale 1 | no |
| Floor panel | `light_area`, x 96–160, y 160–224, z 0–4; its top `neon/glow_panel` | sides `concrete_pavement_036` | no |

Two static `static_light_map` point lights, white, `_light_size 0`: one at
(256, 128, 200), `light` 300, range 768; one at (160, 400, 180), `light` 250,
range 640. The floor panel is an animated `light_area`, `light` 150, pulsing
1.0 → 0.25 → 1.0 over 1600 ms.

## What the build places

The fixture materials are `tools/gen_specular_texel_fixtures.py`'s; see
`specular-texel-sharpness.README.md` for their texel values. The pillar, the
monument, the panel and the ramp cut the floor, and the pillar cuts the
ceiling and the north wall, into several coplanar faces. Placement groups
them by facing, plane and fitted box, so the build places seven probes, each
2 mm off its faces along their normal:

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
