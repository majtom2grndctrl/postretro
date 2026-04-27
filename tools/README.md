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

### `scripts/`

Placeholder for future automation scripts (e.g. `new-mod.sh`, `new-level.sh`). See
`tools/scripts/README.md`.

## Map compilation

Map compilation is handled exclusively by the in-tree `prl-build` crate
(`cargo run -p postretro-level-compiler -- input.map -o output.prl`). No external
toolchain is required.
