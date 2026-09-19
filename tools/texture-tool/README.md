# Texture Tool

Small Rust PNG processor for PostRetro world-material texture bundles. It writes the texture
layout consumed by the level compiler:

```text
stem.png
stem_s.png
stem_n.png
stem_h.png
```

`stem.png` is the diffuse texture, `stem_s.png` is an R8-style grayscale specular map stored as
PNG, `stem_n.png` is a tangent-space normal map, and `stem_h.png` is a grayscale height map.
Specular maps are reflectivity/intensity maps: bright pixels produce stronger direct highlights,
while dark pixels are matte or receive no highlight. Stems may include collection subdirectories
such as `metal/panel_01`; missing parent directories are created under the output directory.

### Height maps (`stem_h.png`)

`stem_h.png` is authored as a **conventional height map: white = raised, black = recessed.**
Do not pre-invert it. The level compiler (`prl-build`) inverts height to depth at bake time
(`depth = 255 - height`) when it packs the map into the engine's surface-depth channel, so authors
always think in the familiar height-map convention; only the compiled `.prm` stores depth.

Height is derived from diffuse luminance — the same signal `stem_n.png` differentiates via Sobel,
sampled one derivative step earlier (undifferentiated). It is contrast-adjusted around the mean
luminance by a profile-driven strength (see `--height-strength` below) and then posterized with
the same quantization used for the diffuse map, so height plateaus line up with diffuse texel
plateaus: the engine's aesthetic dial is terraced depth, and low quantization-level counts should
read as visibly flat plateaus.

**Linear-output guarantee:** `stem_h.png`, like `stem_s.png` and `stem_n.png`, is written with no
`sRGB`, `gAMA`, or `iCCP` PNG chunks and carries forward no color-management metadata from the
source image. `prl-build` fails the build on a color-space-tagged `_h.png`, exactly as it does for
`_s.png`/`_n.png` — see `context/lib/resource_management.md` §4 and `tools/gen_specular.py` /
`tools/gen_normal.py`, which document and enforce the same guarantee.

`stem_h.png`'s dimensions always match the diffuse output exactly, since the level compiler
hard-fails on a dimension mismatch between the two.

## Single Texture

```bash
cargo run --release --manifest-path tools/texture-tool/Cargo.toml -- \
  process \
  --src source.png \
  --stem metal_panel_01 \
  --out-dir content/dev/textures/metal \
  --tileable \
  --spec-profile painted-metal \
  --spec-scale 0.2 \
  --spec-edge-damping 0.45 \
  --normal-strength 0.5 \
  --quantize-levels 24 \
  --height-strength 1.2 \
  --height-quantize-levels 8
```

Required flags: `--src`, `--stem`, and `--out-dir`.

Output defaults to 128x128. `--size 64` is shorthand for a 64x64 texture. Use
`--size WIDTHxHEIGHT` for larger or non-square textures, such as `128x128`,
`256x256`, `64x128`, or `128x64`. The source image is center-cropped to the
requested aspect ratio, then resized with a sharp downsample filter. Use 64x64
when a texture needs a deliberately chunkier old-game read. Rare non-tileable
hero textures, such as a decorative tile centerpoint, can use larger sizes like
128x128 or 256x256.

Optional flags:

- `--tileable` blends opposite edges before quantization.
- `--spec-profile <name>` defaults to `luminance`.
- `--spec-scale <f32>` scales the selected profile's intensity. It defaults to `0.2`, matching the old luminance-derived behavior.
- `--spec-base <0..1>` overrides the profile's base reflectivity.
- `--spec-gamma <f32>` overrides the profile's luminance curve. Values must be greater than `0`.
- `--spec-edge-damping <0..1>` overrides how strongly high-contrast edges and dark seams are made less reflective.
- `--normal-strength <f32>` defaults to `0.5`.
- `--quantize-levels <u8>` defaults to `18` for `64`/`64x64`, otherwise `24`. Use `0` to request the default.
- `--height-strength <f32>` overrides the selected spec profile's default height/depth contrast strength (see table below). Values above `1.0` exaggerate relief; values below `1.0` flatten it.
- `--height-quantize-levels <u8>` defaults to the same size-based default as `--quantize-levels` (`18` for `64`/`64x64`, otherwise `24`). Use `0` to request the default. Lower level counts produce more pronounced, flatter plateaus — the intended retro read for the engine's terraced-depth parallax.

Spec profiles also set the default `_h.png` height/depth strength (overridable with
`--height-strength`); a cobblestone/masonry-like profile wants pronounced, plateau-like steps,
while a smooth glass or metal profile wants almost none:

| Profile | Intended use | Default height strength |
|---------|--------------|--------------------------|
| `luminance` | Backward-compatible diffuse-luminance response. | `1.0` |
| `matte` | Very low response for dull surfaces. | `0.5` |
| `concrete` | Low response with strong seam and scuff damping. | `1.1` |
| `polished-stone` | Moderate base reflectivity while keeping dark seams subdued. | `1.5` |
| `painted-metal` | Higher response with moderate edge damping. | `0.25` |
| `glass` | Bright response, less tied to diffuse luminance. | `0.1` |
| `screen` | Bright, broad response for glossy display surfaces. | `0.15` |
| `water` | Bright wet response, only partly tied to diffuse luminance. | `0.05` |

## Batch Manifest

```bash
cargo run --release --manifest-path tools/texture-tool/Cargo.toml -- \
  batch --manifest textures.manifest --out-dir content/dev/textures/skymall
```

The manifest is UTF-8 text. Empty lines and lines starting with `#` are ignored. Existing
seven-field entries continue to work:

```text
src|stem|size|tileable|spec_scale|normal_strength|quantize_levels
```

Entries may also include optional trailing spec controls, and, after those, optional trailing
height controls:

```text
src|stem|size|tileable|spec_scale|normal_strength|quantize_levels|spec_profile|spec_base|spec_gamma|spec_edge_damping|height_strength|height_quantize_levels
```

`size` may be blank to use the 128x128 default. It also accepts a square value
(`64`) or explicit dimensions (`WIDTHxHEIGHT`). `tileable` must be `true` or
`false`. `quantize_levels` may be blank or `0` to use the size-based default.
Relative source paths resolve from the manifest file's directory. Blank optional trailing spec
fields use the selected profile's defaults. `spec_profile` defaults to `luminance`; `spec_base` and
`spec_edge_damping` are `0..1` overrides; `spec_gamma` must be greater than `0`. `height_strength`
blank uses the selected profile's default height strength (see table above); `height_quantize_levels`
may be blank or `0` to use the same size-based default as `quantize_levels`. `height_strength` and
`height_quantize_levels` may be supplied even when the four spec-control fields before them are
left blank.

Example:

```text
# src|stem|size|tileable|spec_scale|normal_strength|quantize_levels|spec_profile|spec_base|spec_gamma|spec_edge_damping|height_strength|height_quantize_levels
sources/panel.png|metal_panel_01||true|0.2|0.5|
sources/chunky-panel.png|metal_panel_64_01|64|true|0.2|0.5|
sources/medallion.png|floor_medallion_01|128x128|false|0.24|0.45|24
sources/tall-inset.png|decorative_tile_center_01|64x128|false|0.24|0.45|24
sources/glass.png|glass_panel_01|128|false|0.2|0.35|24|glass|||
sources/wet-floor.png|floor_wet_01|128|true|0.35|0.5|24|water|0.42|1.0|0.2
sources/cobble.png|cobble_floor_01|128|true|0.24|0.5|24|polished-stone||||1.6|6
```

## Contact Sheet

```bash
cargo run --release --manifest-path tools/texture-tool/Cargo.toml --bin contact -- \
  --dir content/dev/textures/skymall \
  --out /private/tmp/skymall-contact.png \
  --manifest textures.manifest
```

With `--manifest`, the sheet uses manifest order and shows a 2x2 repeat preview for entries marked
tileable. Without a manifest, it recursively scans `--dir` for diffuse PNGs that have matching
`_s.png` and `_n.png` siblings; tileability is unknown in scan mode, so repeat previews are omitted.
