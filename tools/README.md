# tools/

Developer-side helpers for the Postretro project. The Python scripts here are not built
by `cargo`; `texture-tool/` is a Cargo crate and is the one exception.

## Contents

### `gen_specular.py`

Generates packed specular/PBR textures from per-channel source maps. Run with `python3
tools/gen_specular.py --help` for usage. Used during asset authoring; not invoked by the
build pipeline.

### `gen_normal.py`

Helper for producing normal maps from height/source data. Run with `python3
tools/gen_normal.py --help` for usage.

### `gen_emissive.py`

Generates static emissive `_e.png` siblings from diffuse textures. It retains
bright source-color texels as sRGB content. Every texture takes one default
luminance cutoff (0.75), whatever its name's prefix; `--cutoff` lowers it to keep
more of a sign's pattern. How strongly the result glows is the mod's
`materials.toml`, not the generator. Its tests run with
`python3 -m unittest discover -s tools/tests -p test_gen_emissive.py`. Output PNGs are deliberately untagged (no `sRGB`, `gAMA`, or
`iCCP` chunk); `_e` is an sRGB-content convention and `prl-build` accepts it
regardless of PNG color-space metadata. Run with `python3 tools/gen_emissive.py
--help` for usage.

### `migrate_neon_prefix.py`

Moves a mod off the retired `neon_` texture prefix, which once gave a texture a
glow strength of 4.0. It renames each `textures/<collection>/neon_x.png` and its
slot siblings to `x`, rewrites `.map` face references, and writes
`emissive = 4.0` into the mod's `materials.toml` for each renamed texture with an
`_e` map (an existing entry for the old name is carried to the new key instead).
Dry run by default; `--apply` performs it. It refuses, writing nothing, on a name
collision or a new name whose prefix is a known material prefix. Its prefix list
copies `MATERIAL_PREFIXES`; a `postretro-render-data` test keeps the two in step.
Standard library only (Python 3.11+). Run with `python3
tools/migrate_neon_prefix.py <mod root> [--apply]`; its tests run with
`python3 -m unittest discover -s tools/tests -p test_migrate_neon_prefix.py`.

### `gen_fluid_textures.py`

Generates the dev mod's tileable fluid surface textures (`content/dev/textures/fluids/`):
water, neon water, sludge, lava and slime, each on a periodic domain and quantized to a
small palette so single-texel motion stays visible. Standard library only, fixed seeds.
Run with `python3 tools/gen_fluid_textures.py --out content/dev/textures/fluids`.

### `gen_specular_texel_fixtures.py`

Generates the exact-value specular fixtures in `content/dev/textures/test/` used by the
`specular-texel-sharpness` dev map: a `_p` smoothness ramp, a `_t` tint inlay and a carved
`_h`+`_p` gloss tile, each 16×16 with its `_s` sibling. Standard library only, no randomness;
siblings are untagged linear grey. Its test checks every byte. Run with
`python3 tools/gen_specular_texel_fixtures.py`; its tests run with
`python3 -m unittest discover -s tools/tests -p test_gen_specular_texel_fixtures.py`.

### `gen_dev_glyphs.py`

Generates the dev mod's placeholder input glyph art: one PNG per input name for
keyboard and mouse, Xbox, PlayStation, and Nintendo, drawn from shapes and a
small bitmap font. Standard library only (no Pillow). Run with `python3
tools/gen_dev_glyphs.py --out content/dev/ui/glyphs`.

### `blender_model_rebake.py`

Turns a downloaded high-poly source glTF (Sketchfab, Rodin, etc.) into an
engine-ready low-poly character/prop with a freshly baked texture atlas: joins
all mesh nodes into one, welds and Collapse-decimates to a target triangle
count, re-unwraps, and Cycles-bakes the diffuse albedo onto a single atlas. The
output satisfies the model loader's constraints (one mesh node, one material,
feet-at-origin, base-color-only — see `context/lib/resource_management.md` §7).

Requires Blender 4.5 LTS (headless). Run from the source glTF's own directory so
its `textures/` resolve:

```sh
blender --background --python tools/blender_model_rebake.py -- \
    <source.gltf> <out_atlas.png> <out.gltf> <atlas_res> <target_tris>
```

Then bake the atlas into the runtime `.prm` cache:

```sh
postretro-tool bake-model-textures <out.gltf>
```

### `mixamo_to_gltf.py`

Converts Mixamo FBXs to engine glTF: a rig plus clips into one character, or
clips into a shared library. Runs in headless Blender, which rejects ASCII FBX.
The script's docstring and `--help` cover every flag. Library and binding
contract: `docs/character-animation.md`.

```sh
# Build a library. The rig FBX sits inside the clip folder; none of its own
# animation enters the library. To add a clip, place its FBX there and rerun.
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir clips/ --base rig.fbx --library \
    --output content/mymod/models/clip_libraries/mixamo_rifle/library.gltf

# Convert a character: mesh and skeleton only, bound to that library.
# The path is relative to --output, with forward slashes.
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir trooper/ --base trooper.fbx \
    --clip-library ../clip_libraries/mixamo_rifle/library.gltf \
    --output content/mymod/models/trooper/model.gltf
```

A library keeps every bone the rig has; pass `--strip-face` to drop facial bones
its meshes lack. `--library` and `--clip-library` cannot be combined. Numbered
`mixamorig<N>:` prefixes normalize to `mixamorig:` in every mode, including
`--no-tag`.

To bind an already-exported character with no Blender run, stub `bpy` and
`mathutils`, import the script, and call `bind_gltf_to_clip_library(gltf_path,
clip_library)`. It strips clips, compacts the `.bin` and stamps the reference in
place. It also tags a numbered or untagged rig; a tagged canonical model keeps its
tags. `tools/tests/test_mixamo_to_gltf.py` shows the stubbing.

### `telemetry_reduce.py`

Summarizes one telemetry file written with `POSTRETRO_TELEMETRY=<path>` by the
engine, `prl-build`, or both appending to the same path. It joins the file per
run and per load: producer states and exit reason, bake stage durations, each
load's phase table, geometry install and matching bake, cancelled generations,
SH and lightmap miss buckets per level, and CPU stage and GPU pass p50 and max
across timing windows. Standard library only. Run with `python3
tools/telemetry_reduce.py <file.jsonl>`, or add `--json` for the same summary
as JSON. Its tests run with
`python3 -m unittest discover -s tools/tests -p test_telemetry_reduce.py`.

### `texture-tool/`

Rust PNG processor that writes a whole world-material bundle — diffuse, `_s`
specular, `_n` normal, and `_h` height (Surface Depth) — from one source image,
at a chosen size and quantization. Unlike the scripts above it is Cargo-built,
not Python:
`cargo run --release --manifest-path tools/texture-tool/Cargo.toml -- --help`.
Flags, spec profiles, and the batch-manifest format:
`tools/texture-tool/README.md`. Author-facing height-map guidance:
`docs/level_design.md` §Surface Depth.

### `scripts/`

Placeholder for future automation scripts (e.g. `new-mod.sh`, `new-level.sh`). See
`tools/scripts/README.md`.

## Requirements

Python 3.8+ for all standalone (non-Blender) scripts in this directory.

Install the third-party deps into a venv, from the repo root:

```sh
python3 -m venv .venv && source .venv/bin/activate
pip install -r tools/requirements.txt
```

(A `uv`-managed venv works the same way: `uv venv && uv pip install -r tools/requirements.txt`.)

Per-script deps:

| Script | Third-party deps |
| --- | --- |
| `gen_specular.py` | Pillow |
| `gen_normal.py` | Pillow, numpy (optional — see below) |
| `gen_emissive.py` | Pillow |
| `gen_sdf_shadow_fixture.py` | none (stdlib only) |
| `gen_stress_map.py` | none (stdlib only) |
| `inspect_fbx_ascii.py` | none (stdlib only) |
| `telemetry_reduce.py` | none (stdlib only) |

`gen_stress_map.py` imports three sibling helper modules, `stress_environment.py`
(environment-volume shapes and particle emitters), `stress_zones.py` (zoned
layout for `--preset env-volumes`) and `stress_street.py` (the street map for
`--preset street`). All are stdlib-only and are not entry points. See
`content/dev/maps/stress-env-volumes.README.md` for the env-volumes fixture and
`content/dev/maps/stress-street.README.md` for the street.

The generator's tests run with the standard library:
`python3 -m unittest discover -s tools/tests -p test_gen_stress_map.py`.

`gen_normal.py` imports `numpy` inside a `try`/`except`: if it's missing, the
script still runs but falls back to a flat/neutral normal map instead of the
Sobel-filtered one.

`blender_model_rebake.py`, `prop_to_gltf.py`, `mixamo_to_gltf.py`, and
`extract_cyberpunk_weapons.py` import `bpy` (and `mathutils`), Blender's own
Python API. They run **inside Blender's bundled Python interpreter** (`blender
--background --python tools/<script>.py -- ...`), not a standalone
interpreter — nothing from `requirements.txt` needs to be pip-installed for
them.

### Invocation

```sh
python3 tools/gen_specular.py --input <path> --recursive
python3 tools/gen_normal.py   --input <path> --recursive
python3 tools/gen_emissive.py --input <path> --recursive
```

Pass `--help` to any generator for full option reference.

## Map compilation

Map compilation is handled exclusively by the in-tree `prl-build` crate
(`cargo run -p postretro-level-compiler -- input.map -o output.prl`). No external
toolchain is required.
