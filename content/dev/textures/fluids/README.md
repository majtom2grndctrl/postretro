# Fluid surface textures

Procedural, seamlessly tiling liquid textures for dev fixture maps. They exist to
judge fluid-surface motion (whole-texel shimmer, ripple rings, current drift)
against real fluid art. Each one uses a small palette (12–13 colours) with
chunky texel-scale features, so a one-texel offset is visible under nearest
sampling.

| Map-facing name | Size | Look |
|---|---|---|
| `fluids/water_64` | 64×64 | Deep blue-teal pool with a bright cyan caustic web. |
| `fluids/water_128` | 128×128 | Same water at twice the extent; features stay the same texel size. |
| `fluids/water_neon_64` | 64×64 | Cyberpunk canal water: violet body, magenta-to-cyan caustic web. |
| `fluids/sludge_64` | 64×64 | Murky olive-brown ooze with oily contour sheen and pale bubble rims. |
| `fluids/lava_64` | 64×64 | Dark cooled crust plates split by glowing orange-yellow cracks, a few hot bubbles. |
| `fluids/slime_64` | 64×64 | Toxic green gel with glowing lumps and pale bubble rings. |

All are 8-bit RGBA PNGs with opaque alpha. They are diffuse-only: no `_n`, `_s`
or `_e` siblings, and no registration step. A `.map` face references them by the
names above.

## Regenerating

The textures are output of `tools/gen_fluid_textures.py`. It uses only the
Python standard library and fixed seeds, so it reproduces the same pixels (PNG bytes can differ between zlib builds).
From the repository root:

```sh
python3 tools/gen_fluid_textures.py --out content/dev/textures/fluids
```

Add `--preview <dir>` to also write a 2×2-tiled, 4×-upscaled copy of each
texture for checking seams by eye.

Tiling holds because every input wraps: value noise and Worley cells sit on
periodic lattices, domain warps sample that wrapped noise, bubbles use toroidal
offsets, and the 4×4 ordered dither divides every texture size.
