# Model Texture Bake

## Goal

Make `prl-build` bake `prop_mesh` model textures during a normal level compile, content-driven (no CLI flag). A plain compile produces each placed model's base-color `.prm` in `prm-cache`, so model rendering self-heals after a cache flush. Removes the architectural coupling where a runtime asset (a model's texture) depends on a hand-staged, gitignored build-cache file that nothing regenerates.

## Background

Model materials resolve their texture at runtime by content-hashing the base-color PNG to `blake3(png_bytes)` and opening `<hex>.prm` from `.build-caches/prm-cache/`. Nothing produces that `.prm`: `prl-build` only bakes world textures named in the map's `TextureNames`, and model PNGs are neither in that list nor under `content/<mod>/textures/`. The one shipping model's `.prm` was hand-staged into the gitignored cache. Flush the cache and the texture is gone for good — the runtime degrades to the magenta/black placeholder.

The two key derivations already agree: the world baker's diffuse-only filename key is `blake3(diffuse_png_bytes)` (`texture_mips::filename_key_for`), and the runtime model loader's key is `blake3(base_color_png_bytes)` (`gltf_loader::content_hash_material_key`). Same hash of the same bytes → a compiler-baked model `.prm` is byte-for-byte what the runtime already opens. No new key scheme, no runtime change to the load path.

## Scope

### In scope

- A shared glTF base-color resolver, used by both the runtime loader and the compiler — one implementation, no duplicated URI/path logic.
- A new `prl-build` stage that discovers `prop_mesh` placements in the map, resolves each model's base-color PNG(s), and bakes them into `prm-cache` reusing the existing mip-bake primitives.
- A single-diffuse bake entry point in `texture_mips` that produces a `.prm` byte-identical to a diffuse-only world texture.
- Non-fatal degradation: a missing/unreadable glTF or PNG warns and continues, mirroring both the world baker ("missing PNGs are not an error") and the runtime (degrade-to-placeholder).

### Out of scope

- **No PRL section change.** The runtime computes model material keys itself from the glTF at load and opens `<key>.prm` directly — it never reads model keys from a PRL section (unlike world textures via `TextureCacheKeys`). The compiler only needs the `.prm` files to exist. `pack.rs` and the PRL format are untouched.
- Specular/normal slots for models. Models carry a base-color (diffuse) slot only this slice; the runtime substitutes neutral placeholders for absent slots.
- Embedded-image / `.glb` models. The runtime loader is external-`.gltf` only; the baker matches that — embedded base-color images resolve to no path and are skipped (placeholder at runtime, as today).
- Shipping committed `.prm` sidecars as content (the alternative severance design). Model textures stay a build output, consistent with world materials.
- Retiring the runtime's `content_hash_material_key` recipe. It stays; only its path-resolution helper moves to the shared crate.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| prop_mesh classname | `prop_mesh::CLASSNAME` (`"prop_mesh"`) | `MapEntityRecord.classname == "prop_mesh"` | `classname "prop_mesh"` |
| model handle | `MeshComponent.model` (`String`) | `MapEntityRecord.key_values` entry, key `"model"` | `model "<rel path>"` |
| model `.prm` filename | `cache_filename_for_key(blake3(png))` | `<64-hex>.prm` under `prm-cache/` | n/a |

The `model` value is a content-root-relative path (e.g. `models/decraniated_low_poly_retro_pixel/scene.gltf`), the same string the renderer caches under verbatim. The compiler resolves the file as `content_root.join(model)`, where `content_root` is the grandparent of the input `.map` (mirrors the runtime's `content_root_from_map` and the compiler's existing `resolve_texture_root`).

## Wire format

No new layout. A model `.prm` is the existing diffuse-only `.prm` shape: header with `slot_mask = DIFFUSE`, `bundle_hash = bundle_hash_for(Some(diffuse), None, None)`, one `Rgba8UnormSrgb` diffuse slot built by `build_diffuse_chain`. Filename stem is `blake3(diffuse_png_bytes)` hex. This is the same file a diffuse-only world texture produces today; `postretro-level-format::prm` owns the layout.

## Acceptance criteria

- [ ] Compiling a map that places a `prop_mesh` writes that model's base-color `.prm` into `prm-cache`, named `blake3(base_color_png)` hex.
- [ ] Deleting `.build-caches/` entirely and recompiling the map regenerates the model `.prm` (self-heal — no manual staging).
- [ ] Running the engine on the compiled map renders the model with its texture, not the magenta/black placeholder, on a fresh checkout after a cache flush + compile.
- [ ] A model referenced by multiple `prop_mesh` placements is baked once per distinct base-color PNG, not once per placement.
- [ ] A map with no `prop_mesh` entities produces byte-identical PRL and `prm-cache` output to before this change.
- [ ] A `prop_mesh` whose glTF is missing, malformed, or whose base-color PNG is absent/unreadable: the build logs a warning and exits zero (the model renders a placeholder at runtime, as it does today).
- [ ] The glTF base-color path resolution lives in exactly one place; the runtime loader and the compiler both call it.

## Tasks

### Task A: Shared glTF base-color resolver

Create a small workspace crate (proposed `crates/model-assets`, package `postretro-model-assets`) depending only on `gltf` and `percent-encoding`. It exposes:

- A path resolver equivalent to the runtime's current private `resolve_image_path` (percent-decode the URI, join `parent_dir`).
- A function that, given a glTF file path, opens the document (no buffer/image import) and returns the deduplicated set of resolved base-color PNG paths across all materials — the base-color-URI match currently inlined in `content_hash_material_key`, applied per material. `Source::View` (embedded) materials contribute no path.

Returning a `Vec` (not one path) matches the runtime, which resolves a key per submesh material — a multi-material model has multiple distinct base-color PNGs, all of which must bake.

### Task B: Single-diffuse bake entry point in `texture_mips`

Add a function to `crates/level-compiler/src/texture_mips.rs` that bakes one base-color PNG into `prm-cache` and returns its 32-byte key. It reuses the existing primitives — `decode_png_rgba`, `build_diffuse_chain`, `filename_key_for(Some(d), None, None)`, `bundle_hash_for(Some(d), None, None)`, `expected_level_count`, `PrmFile`/`PrmSlot`, the cache-hit short-circuit, and `atomic_write` — emitting a DIFFUSE-only `.prm`. No bake algorithm is duplicated; this is the diffuse-only slice of the existing per-texture loop body factored for a caller that supplies a PNG path directly rather than a `TextureNames` entry. Independent of Task A (no glTF knowledge — it takes a PNG path/bytes).

### Task C: `prl-build` model-texture stage

Add a stage to `crates/level-compiler/src/main.rs`, near the existing "Texture mip bake" stage. It:

1. Resolves `content_root` from the input `.map` (grandparent of the input; add a helper mirroring `resolve_texture_root`'s parent logic).
2. Walks `map_data.map_entities` for records with `classname == "prop_mesh"`, reading the `"model"` key from `key_values`; collects the distinct non-empty model handles.
3. For each handle, resolves `content_root.join(model)`, calls Task A to get base-color PNG paths, and bakes each distinct path via Task B into the same `prm_cache_root` already resolved at the texture-mip stage.
4. Warns and continues on any missing/malformed glTF or unreadable PNG; never fails the build.

Depends on Task A (resolver) and Task B (baker). The map entities are already parsed into `map_data.map_entities`; the `prm_cache_root` is already in scope from the texture-mip stage.

### Task D: Point the runtime loader at the shared resolver

Refactor `gltf_loader::content_hash_material_key` (and remove the now-redundant private `resolve_image_path`) to call the Task A resolver. Behavior is unchanged: same base-color-URI match, same percent-decode/join, same `blake3` → hex, same zero-sentinel degradation. The existing key-recipe test (the `581e80bb…` assertion) must still pass unchanged — it pins the recipe the shared resolver now backs. Depends on Task A.

## Sequencing

**Phase 1 (concurrent):** Task A — new shared crate; Task B — `texture_mips` single-diffuse baker. Independent (different crates, no shared types).
**Phase 2 (concurrent):** Task C — compiler stage, consumes A + B; Task D — runtime loader refactor, consumes A. Different crates; no shared files.

## Open questions

- **Shared crate vs. shared module.** A new `crates/model-assets` is the proposed home because `level-format` (the only existing shared crate) is the wire-format crate and should not gain a `gltf` dependency. If a whole crate feels heavy for a ~20-line resolver, the fallback is still a single home (e.g. the resolver living in the runtime crate, re-exported) — but it must not be duplicated into the compiler. Decision needed before Task A lands.
- **Stage placement / cache reporting.** The model bake can run as its own progress stage or fold into the existing "Texture mip bake" stage. Folding keeps one timing line; a separate stage makes the model-vs-world split legible in `--verbose`. Either satisfies the AC.
- **`done/M10` findings note.** `context/plans/done/M10--model-pipeline-slice/findings.md` documents the manual-staging workaround as a known gap. It is a historical record (not maintained), so it needs no edit — but the durable note that model textures are now a compiler output belongs in `context/lib/build_pipeline.md` at promotion, not during drafting.
