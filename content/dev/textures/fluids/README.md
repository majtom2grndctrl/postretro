# Fluid surface textures

Procedural, seamlessly tiling liquid textures for dev fixture maps. They exist to
judge fluid-surface motion (whole-texel shimmer, ripple rings, current drift)
against real fluid art. Each one uses a small palette (12–13 colours) with
chunky texel-scale features, so a one-texel offset is visible under nearest
sampling.

| Map-facing name | Size | Look |
|---|---|---|
| `fluids/water_64` | 64×64 | Deep blue-teal pool with a bright cyan caustic web. |
| `fluids/water_128` | 128×128 | Water at twice the extent from its own seed: caustic cells keep the same texel size, while the swell and warp scale with the texture. |
| `fluids/water_neon_64` | 64×64 | Cyberpunk canal water: violet body, magenta-to-cyan caustic web. |
| `fluids/sludge_64` | 64×64 | Murky olive-brown ooze with oily contour sheen and pale bubble rims. |
| `fluids/lava_64` | 64×64 | Dark cooled crust plates split by glowing orange-yellow cracks, a few hot bubbles. Has an `_e` emissive sibling. |
| `fluids/slime_64` | 64×64 | Toxic green gel with glowing lumps and pale bubble rings. |

All are 8-bit RGBA PNGs with opaque alpha, with no `_n` or `_s` siblings and no
registration step. A `.map` face references them by the names above.

`lava_64_e.png` is lava's emissive sibling: the molten cracks and bubbles at
their diffuse colors, ramping down into the crust, black elsewhere. It shares
the diffuse's texel grid, and the fluid shader samples both at the same moved
UV, so the glow moves with the surface. Emissive strength is chosen by texture
prefix (`Material::emissive_strength`), and only `neon` is nonzero, so the
`lava` prefix bakes this map but shows no glow until it gets a strength.

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
