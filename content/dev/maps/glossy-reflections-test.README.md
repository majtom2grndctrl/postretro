# glossy-reflections-test: a glossy floor and wall around a freestanding monument

One sealed room for the `glossy-world-reflections` feature: a glossy floor
with a non-glossy pillar and a small freestanding monument standing on it,
and a glossy wall. Glossy placement runs only when `prl-build` is given
`--glossy-reflections`; without it the map bakes as if no material reflected.

## Build and run

From the workspace root:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/glossy-reflections-test.map \
  --glossy-reflections -o content/dev/maps/glossy-reflections-test.prl
cargo run -p postretro-tool -- run --install-root . maps/glossy-reflections-test.prl
```

`postretro-tool run` passes `--mod`, so its map path is relative to the mod
folder (`content/dev`). The `.prl` is a local build artifact; do not commit it.
Under `-v` the build logs the glossy census line, the floors that take probes
and each probe's faces.

## Layout

TrenchBroom units, Z up (1 unit = 0.0254 m), Valve 220 texture axes. The
interior spans x 0–512, y 0–512, z 0–256 (13 × 13 × 6.5 m), with walls 32
thick. You spawn at (64, 64) in the south-west, facing the monument.

| Surface | Where | Texture | Reflects |
|---|---|---|---|
| Floor | z 0 | `test/metal_smoothness_ramp`, scale 8 | yes: a `_p` ramp and `_s` 255 |
| North wall | y 512 | `test/concrete_carved_tinted_gloss`, scale 8 | yes, with `_t`; binds no probe until walls do |
| Other walls, ceiling | x 0, x 512, y 0, z 256 | `50-free-textures/concrete_stone_021`, scale 1 | no: `_s` and no `_p` |
| Pillar | x 384–416, y 352–384, z 0–256 | `concrete_stone_021` | no |
| Monument | plinth x 224–288, y 224–288, z 0–16; column x 240–272, z 16–96; cap x 232–280, z 96–104 | `50-free-textures/concrete_pavement_036`, scale 1 | no |

Two static `static_light_map` point lights, white, `_light_size 0`: one at
(256, 128, 200), `light` 300, range 768; one at (160, 400, 180), `light` 250,
range 640.

## What the build places

The fixture materials are `tools/gen_specular_texel_fixtures.py`'s; see
`specular-texel-sharpness.README.md` for their texel values. The pillar and
the monument cut the floor into several coplanar faces. Placement groups them
by plane and fitted box, so the floor takes two probes: one for most of the
room, and one for the strip north of the pillar, whose box fit lands on the
pillar's north face.

The main floor probe captures from the floor's area centroid, which falls
on the monument's plinth and moves just off its south edge. Glossy probes
exclude no brush yet, so the monument hides most of the floor north of it
from that point, and the build warns that the probe sees under half of its
floor.
