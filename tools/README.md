# tools/

Developer-side helpers for the Postretro project. These are not shipped with the engine
and are not built by `cargo`.

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
bright source-color texels as sRGB content, with `neon_` textures using a lower
default cutoff. Output PNGs are deliberately untagged (no `sRGB`, `gAMA`, or
`iCCP` chunk); `_e` is an sRGB-content convention and `prl-build` accepts it
regardless of PNG color-space metadata. Run with `python3 tools/gen_emissive.py
--help` for usage.

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
cargo run -p xtask -- bake-model-textures <out.gltf>
```

### `scripts/`

Placeholder for future automation scripts (e.g. `new-mod.sh`, `new-level.sh`). See
`tools/scripts/README.md`.

## Python tool setup

The `gen_specular.py`, `gen_normal.py`, and `gen_emissive.py` helpers require a
`uv`-managed virtual environment. Run once from the repo root:

```sh
uv venv && source .venv/bin/activate
uv pip install Pillow numpy   # numpy only required by gen_normal.py
```

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
