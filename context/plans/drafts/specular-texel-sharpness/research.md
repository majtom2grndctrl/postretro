# specular-texel-sharpness — research

Read at e919c0e0. Supports `index.md`; it decides nothing.

## Specular evaluation census

| Site | L from | V from | Exponent | In scope |
|---|---|---|---|---|
| forward `world_static_specular` (owners + SDF selection) | `in.world_position` (true plane) | `in.world_position` | `material.shininess` | yes |
| forward dynamic loop | — | — | none (diffuse only) | no |
| `kinematic_brush.wgsl` selected-static suffix | `depth.world_position` (hit) | `in.world_position` | `material.shininess` | yes |
| `fluid_surface.wgsl` | — | — | field present, unread | no |
| `skinned_mesh.wgsl` | — | — | none (diffuse only) | no |
| `billboard.wgsl` | sprite centre | sprite centre | per-collection `spec_exponent` | no |

## Channel inventory

| Slot | Baked format | Channels read | Free? |
|---|---|---|---|
| diffuse | `Rgba8UnormSrgb` | RGB; A is passed to an opaque target with no effect | A is stored but unread. It is the natural home for future cutouts |
| specular | `R8Unorm` / `Rg8Unorm` | R intensity; G read only by the Surface Depth march | no third channel |
| normal | `Bc5RgUnorm` | RG | none |
| emissive | `Rgba8UnormSrgb` | RGB | A is reserved for a future per-texel emissive channel (`resource_management.md` §4.5) |

## Cost

A 1024×1024 specular slot across 11 mips: R8 is 1,398,101 B, Rg8 2,796,202 B, Rgba8 5,592,404 B. Only materials with authored sharpness pay for the four-channel form.

## Authoring alternatives

- **`_s.png` green channel plus a migration command.** The owner rejected this. The repo holds 55 grayscale and 2 RGB `_s.png` files. texture-tool's `specular()` writes RGBA with R=G=B, so its output and the RGB pair would need migrating. A colour `_s.png` could then never mean tint, and the build would need a permanent check that G equals R.
- **Suffix collisions.** Case-insensitive, across `content/`: `_g` matches 10 files (the Level Eleven pack's `*_G.png` diffuse maps) and `_x` matches 7. `_p`, `_k`, `_gl` and `_gloss` match none.

## Why reconstruction, not a separate snap

`sample_post_retro` moves the UV to the texel centre when magnified and leaves it near the true UV once a texel is smaller than a pixel. Mapping that same reconstructed UV to world has two effects. When magnified, the highlight is flat per texel with the same antialiased seam that intensity and the normal have. When minified, the position error is sub-pixel. A hard snap, by contrast, would step the position at the seam while N and intensity still blend across it, which would draw a hard edge in the highlight.
