# specular-texel-sharpness — research

Read at e919c0e0; §Tint read at e7438238. Supports `index.md`; it decides nothing.

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
| P8 | Legacy specular slot under tint | A one-channel slot, a two-channel slot or the 1×1 placeholder is sampled by the forward or mover fragment | Whatever its stored width, the slot glints in raw albedo, byte-identical to the previous build. No narrow read is taken as a tinted or sharp texel |
| P9 | Pixel-style point under an active march | Surface Depth On, carved material: march shifts the shading UV → texel centre reconstructed → static specular | The point anchors on the fragment's own UV and plane position, never on the marched UV. Every fragment that lands on one texel's top, or on one of its side faces, takes the same point and draws one highlight value |
| P10 | Mover gates ahead of its specular | Per selected-static light: influence early-out and diffuse N·L gate, both per pixel → pixel-style specular | Neither gate cuts a texel's highlight. A texel crossed by the light's influence radius, range, cone edge or diffuse terminator draws one highlight value, scaled only by each fragment's own shadow. Diffuse keeps its per-pixel gates |
| P11 | Per-texel inputs when materials build at Off | Surface Depth Off committed → level dispatches and installs at Off → On → Off. Cases: `_p`, `_t`, `_p`+`_t`+`_h`, and a legacy one-channel slot | `_p` shades per texel and `_t` tints on the first frame and every later one. The march runs only for the `_h` case at On. The legacy material keeps the raw-albedo multiply throughout |
| P12 | Smoothness byte 255 | bake → sample → decode | Shades at exponent 8192 on every backend. r = 0 makes 2/r⁴ a division by zero, and WGSL clamp() does not reliably sanitize Inf, so r⁴ is floored at the cap's value before the divide, as `sample_post_retro` floors its divisor. Symbols: `sample_post_retro`, `blinn_phong` |
| P13 | Marker bytes through the mip filter | authored map with a hard 0-to-255 edge → Mitchell-Netravali mips (negative lobes) → clamp to [0, 1] → marker clamp | B of a `_p` material stays at or above 1 and A of a `_t` material at or below 254 at every mip; no texel beside the edge reads as unauthored. Symbols: `texture_mips.rs` filter and clamp |
| P14 | `_n` or `_s` at a resolution other than the diffuse (unenforced) | build → pixel style | The position snaps to the albedo lattice while N or intensity steps on its own, so one albedo texel can show more than one highlight value. Accepted, not a build error; shipped content has no such pair. Symbols: `sample_post_retro`, `bake_materials_into` |

## Uniform constraints

- **Material uniform.** `set_surface_depth_quality` rewrites all 32 bytes of the material uniform from `MaterialUniformPlan`. `SurfaceDepthUniform::resolve` returns `FLAT`, which zeroes the march word and its spare bits, in four cases: Off, a flat band, an untuned prefix, and a non-surface slot. Only `shininess` and `emissive_strength` keep their values through Off.
- **Forward group-0 `Uniforms`.** Exactly 128 bytes, shared with billboard and wireframe (`pipeline_layout.rs`). Spare bits are in `spec_shadowmask_flags`, which billboard declares inert.
- **Mover per-frame `KinematicLightParams`.** Has `_pad2`, written every frame.
- **Planar mirror.** It copies the main group-0 bytes, so a style written outside `build_uniform_data` would miss the mirror.

## Rivals

- **Split sharpness from the snap.** Ship sharpness alone, either capped at today's 96 or evaluated per pixel. Bundling wins on two counts:
  - without the snap, sharpness's headline use crawls;
  - both changes edit the same two shader sites, so the Mac `Off` and register-cliff measurement runs once.

## Tint

- **Today's combine.** Both in-scope sites add specular into the lighting sum before the albedo multiply: forward `fs_main` returns `base_color.rgb * total_light`, and mover `fs_main` returns `base_color.rgb * lighting`. Every highlight is therefore tinted by raw albedo and dimmed by dark albedo. Metals glint in their colour by accident, and dark dielectrics (green marble, lacquer) glint dark instead of white.
- **Why hue, not raw albedo.** PBR metals pair dark diffuse with a bright coloured glint. Under a raw-albedo tint, a converter that dims a metal's diffuse dims its glint too, and `_s` intensity cannot exceed 1 to compensate. Normalizing albedo to its largest channel keeps the hue and drops the brightness, so diffuse and glint are set independently.
- **Why no diffuse suppression.** A metallic input would zero diffuse and leave a metal's colour to its reflection. Probe reflections can be absent: the Off tier, capture shadow (withheld, never filled, by the reflection IP rules), faces bound to no probe, and movers. A suppressed metal would go black there.
- **Why in-slot markers, and why these values.** The legacy raw-albedo multiply is not any value of the mix, and a `_t`-only material has no sharpness, so each case needs a marker. A per-material uniform bit would hit the constraint in §Uniform constraints: Surface Depth Off rewrites the uniform. A header flag would need threading to the material plan. One- and two-channel slots and the placeholder read a missing B as 0 and a missing A as 1 (255), so B = 0 means no `_p` and A = 255 means no `_t`; authored values take 1–255 and 0–254. The slot-1 mip filter is Mitchell-Netravali with negative lobes, and its output is clamped to [0, 1], so a hard edge can ring a marker-adjacent texel onto the marker; the bake clamps the marker channels per mip to prevent it.
- **Rival: coloured `_s`** (RGB specular colour, as Doom 3 shipped it). It needs two more channels than the slot has, and the forward pass has no free sampled-texture binding for a second slot. One amount plus the albedo's hue covers metals, which is the case authors asked for.
- **Suffix collisions.** Case-insensitive, across `content/`, at e7438238: `_t` and `_m` match none.
- **Conversion.** Roughness r (glTF convention, α = r²) maps to a Blinn-Phong exponent n = 2/α² − 2 = 2/r⁴ − 2 (Walter et al. 2007, Beckmann ≈ GGX α). Under the earlier log encoding 2^(8·p/255), capped at 256, every r below about 0.3 collapsed to one value: the bottom 30% of a roughness slider, where polished stone, wet floors and chrome live. A reflection lobe matched to an exponent-256 highlight also spans several probe texels, so no texel could read as a mirror (`glossy-world-reflections` research). Storing smoothness and capping at 8192 leaves only r below about 0.125 at the cap, which is a mirror at probe and texel resolution. Metallic maps to tint directly.
