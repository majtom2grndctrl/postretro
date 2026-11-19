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

## Pins

| ID | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Smooth style committed, then the player switches Surface Depth | style commit → Surface Depth Off → On | Static faces and movers draw smooth on every frame. The march turns off, then is restored |
| P2 | Surface Depth Off, then the style changes | Surface Depth Off → style smooth → style pixel | The march stays off. Each style reaches the next frame's specular |
| P3 | A four-channel material whose Surface Depth uniform resolves flat | built, then Surface Depth Off → On. Cases: `_p` without `_h`; a prefix with no Surface Depth tuning; `_p`+`_h` | Per-texel sharpness on every frame in every case. The march runs only for `_p`+`_h` at On |
| P4 | Style committed before a level's materials exist | style smooth → then one of: full init; first level install; unload → next install; surface loss → full-renderer rebuild → install; suspend → resume | The first frame of each installed level draws smooth on static faces and movers |
| P5 | A staged mod init commits while a level load is in flight | load dispatches at pixel → staged init commits smooth → load publishes | The published level draws smooth on its first frame. No further commit is needed |
| P6 | Staged mod-init outcomes after smooth is active | smooth → failed → rejected → stale → committed with no start script | Smooth through failed, rejected and stale. Pixel after the committed no-start result |
| P7 | Plane byte without `_h` | bake | Stored G = 127 (255 − 128). A stored 128 reads as authored 127, a trough of −1/128. Symbols: `surface_height_fraction_from_byte`, `bake_materials_into` |

## Uniform constraints

- **Material uniform.** `set_surface_depth_quality` rewrites all 32 bytes of the material uniform from `MaterialUniformPlan`. `SurfaceDepthUniform::resolve` returns `FLAT`, which zeroes the march word and its spare bits, in four cases: Off, a flat band, an untuned prefix, and a non-surface slot. Only `shininess` and `emissive_strength` keep their values through Off.
- **Forward group-0 `Uniforms`.** Exactly 128 bytes, shared with billboard and wireframe (`pipeline_layout.rs`). Spare bits are in `spec_shadowmask_flags`, which billboard declares inert.
- **Mover per-frame `KinematicLightParams`.** Has `_pad2`, written every frame.
- **Planar mirror.** It copies the main group-0 bytes, so a style written outside `build_uniform_data` would miss the mirror.

## Rivals

- **Split sharpness from the snap.** Ship sharpness alone, either capped at today's 96 or evaluated per pixel. Bundling wins on two counts:
  - without the snap, sharpness's headline use crawls;
  - both changes edit the same two shader sites, so the Mac `Off` and register-cliff measurement runs once.
