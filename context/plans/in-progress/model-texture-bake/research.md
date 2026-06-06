# Research — Model Texture Bake

Grounding notes for the spec. Not maintained after the plan ships.

## Key-derivation match (the linchpin)

The compiler and runtime already agree on the model `.prm` key:

- Runtime: `crates/postretro/src/model/gltf_loader.rs:161` `content_hash_material_key` → `blake3(base_color_png_bytes)`, hex (`:176`).
- Compiler: `crates/level-compiler/src/texture_mips.rs:197` `filename_key_for`, diffuse-present arm `*blake3::hash(d).as_bytes()` (`:203`).

Same hash of the same bytes. A diffuse-only world `.prm` and a model `.prm` for the same PNG are the same file. Confirmed both are `blake3` of the raw PNG bytes (not decoded pixels).

## Runtime load path (unchanged by this plan)

- `main.rs:1910` level-load sweep → `render/mod.rs:2671` `load_skinned_model` → `:2759` `resolve_skinned_model_material` → `load_textures(..., prm_cache_root)` → `loaded_texture.rs:324` opens `prm_cache_root.join("<hex>.prm")`, placeholder on read failure.
- The runtime gets the key from the glTF itself (`content_hash_material_key`), **not** from any PRL section. So the compiler needs only to make the `.prm` exist — no PRL/pack change. This is why "no PRL section" is in out-of-scope.
- `prm_cache_root` runtime origin: `crates/postretro/src/startup/worker.rs:103` `derive_prm_cache_root_dev_layout` → `<workspace>/.build-caches/prm-cache`.

## Compiler-side inputs already present

- `crates/level-compiler/src/main.rs:751` calls `texture_mips::bake_texture_mips(&geo_result.texture_names.names, &texture_root, &prm_cache_root)` — world textures only; `texture_names` comes from the map's `TextureNames`, which never lists model PNGs.
- `prm_cache_root` resolved at `main.rs:750` via `resolve_prm_cache_root_via_cargo` → `<workspace>/.build-caches/prm-cache` (same dir the runtime reads).
- `texture_root` via `resolve_texture_root` (`main.rs:93`) = `<content_root>/textures`, where `content_root` = parent of the map's dir. A model bake needs `content_root` itself (models live under `content/<mod>/models/...`, not `.../textures/`).
- Map entities parsed into `map_data.map_entities: Vec<MapEntityRecord>` (`map_data.rs:468`); `MapEntityRecord` has `classname` and `key_values: Vec<(String,String)>` (`map_data.rs:122`).
- The compiler crate has **no** `gltf` or `percent-encoding` dependency today (`crates/level-compiler/Cargo.toml`). Both are workspace deps already used by the runtime.

## prop_mesh contract

- `crates/postretro/src/scripting/builtins/prop_mesh.rs:17` `CLASSNAME = "prop_mesh"`; model handle read from the `"model"` KVP (`:39`). Empty/absent → renders nothing.
- Handle is a content-root-relative path; runtime opens it as `content_root.join(handle)` (`render/mod.rs:2677` `resolve_model_open_path_and_handle`), caches under the verbatim handle.
- `content_root_from_map` (`main.rs:83`) = grandparent of the `.prl`/`.map` path.

## Bake primitives reusable for the single-diffuse path

In `texture_mips.rs`, all private to the crate, all reusable by a new sibling function: `decode_png_rgba` (`:591`), `build_diffuse_chain`, `filename_key_for` (`:197`), `bundle_hash_for` (`:163`), `expected_level_count`, `cache_filename_for_key`, `atomic_write` (`:601`), `PrmFile`/`PrmSlot`/`PrmHeader`/`PrmSlots`/`PrmFormat`. The world loop's diffuse branch (`:722-733`) is the slice to factor: decode → `build_diffuse_chain` → one `Rgba8UnormSrgb` slot. Cache-hit short-circuit at `:704-714`.

## Resolver to share

Runtime `resolve_image_path` (`gltf_loader.rs:139`): percent-decode URI, `parent_dir.join`. Base-color-URI extraction inlined in `content_hash_material_key` (`:162-171`): `material.pbr_metallic_roughness().base_color_texture()` → `Source::Uri { uri }` (skip `Source::View`). These two pieces are the entire shared surface. The runtime opens the document with `gltf::Gltf::open` (no image import) at `:237` — the compiler resolver should do the same (cheap; no PNG decode).

## Out-of-scope confirmations

- `581e80bb…` assertion (`gltf_loader.rs:881`) tests the **key recipe**, not manual staging. It stays and must keep passing after Task D's refactor.
- `.build-caches/` is gitignored (`.gitignore:27`); model `.prm` was never committed — confirming the cache cannot be the system of record.
- M10 manual-staging note: `context/plans/done/M10--model-pipeline-slice/findings.md:221-242`.
