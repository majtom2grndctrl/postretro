# glTF Mesh Loading

> Milestone 10, render-foundation track — the task after the shipped thin vertical slice
> (`context/plans/done/M10--model-pipeline-slice/`). Research notes: `research-brief.md` (sibling).

## Goal

Generalize the slice's narrow glTF loader into a real runtime loader: read arbitrary glTF geometry +
skinning attributes into the slice's locked vertex layout, resolve material textures to baked `.prm`
GPU handles by **runtime content-hashing** (retiring the hardcoded key table), and read glTF
`extras` onto the spawned entity. Builds **in place** against the contracts the slice locked — no
redesign of the vertex layout, bone palette, or CPU/GPU module split.

## Scope

### In scope

- **Image-decode-free load.** Switch the loader entry from `gltf::import` (decodes every PNG, then
  discards them) to `gltf::Gltf::open` + `gltf::import_buffers`, reading only geometry/skin/animation
  buffers. Material PNGs resolve through the engine cache, never the `gltf` crate's image decode.
- **Runtime material content-hashing.** Replace `STAGED_MATERIAL_KEYS` / `staged_material_key` with
  a function that resolves a primitive's base-color image URI to its on-disk PNG (against the glTF's
  parent dir), reads the file bytes, and computes the `.prm` filename key as `blake3(png_bytes)` —
  the same recipe `filename_key_for` uses for a diffuse-present texture. Result is the 64-char hex
  key the existing `.prm` open path already consumes.
- **Multi-primitive materials.** A mesh with multiple primitives currently merges into one stream
  but the renderer binds only `material_keys.first()`. Carry a per-primitive **submesh range**
  (material key + index range) on the loaded model; the renderer resolves every distinct key to a
  material bind group and the mesh pass draws each submesh with its own material. Single-instance,
  flat-lit (instancing + SH lighting stay the next task).
- **`extras` → entity tags.** Enable the `gltf` crate `extras` feature. Read the model's top-level
  `extras` (raw JSON), extract a `tags` array of strings (mirroring map `_tags`), and carry it onto
  the spawned entity through the existing tags mechanism (`registry.set_tags` / `try_spawn`). Absent
  or malformed `extras` yields no tags — not an error.
- **Arbitrary-asset correctness.** The loader names no asset-specific path, URI, or key. Loading any
  conformant skinned/rigid glTF produces correct geometry, materials, and tags.

### Out of scope (non-goals)

- **Model handle / cache, and `MeshComponent`-carries-handle wiring.** The roadmap files this under
  the next task (*Mesh render pass + `MeshComponent`*: "`MeshComponent` carries a model handle … the
  slice's asset was hardcoded behind a seam this resolves"). This task keeps the single-active-model
  upload; multi-model coexistence arrives with its consumer (classname spawning). *(This is a
  scope correction from the research brief, which floated establishing the handle here — the
  roadmap's next bullet owns it.)*
- **Classname spawning** — next task. The hardcoded spawn seam stays.
- **Many-instance draw, SH lighting, bone-palette indexing/scaling** — next task (*Mesh render pass*)
  and the lighting steps.
- **Tangent generation (MikkTSpace).** No M10 consumer (meshes are SH-lit, not normal-mapped).
  Read `TANGENT_0` if present; keep the placeholder when absent — unchanged from the slice.
- **Model normal / metallic-roughness textures.** Only base-color resolves; the lighting steps bake
  and bind the rest.
- **Model-PNG bake *discovery*.** This task is runtime-only. The model's base-color `.prm` is assumed
  present in the cache (for the dev asset it already is — content-hashing reproduces the pre-baked
  `581e80bb…` key). Automating prl-build discovery of model PNGs rides with classname spawning.
- **Multi-mesh documents.** One mesh per model (first mesh, all primitives). Documents with multiple
  meshes are deferred.
- **Embedded / `.glb` images** (`gltf::image::Source::View`) — base-color must be an external URI;
  an embedded source resolves to no key (placeholder), as today.
- **Shadow casting, dynamic direct lighting** — separate M10 steps.

## Acceptance criteria

- [ ] Loader names no asset-specific path, URI, or material key — `STAGED_MATERIAL_KEYS` is gone.
- [ ] A primitive's base-color material key equals `blake3(<referenced PNG file bytes>)` rendered as
      64-char lowercase hex; the dev asset's primitive resolves to `581e80bb91c2d2e6fbed2aca5ba8fc0252aa7485579ea21376eeb294e972f0f1`
      (behavior-preserving) with no hardcoded table.
- [ ] A glTF whose base-color PNG file is **missing** still loads geometry and returns a key (or the
      zero sentinel) without an `Import` error — proving images are not decoded at load.
- [ ] A multi-primitive model exposes one submesh range per primitive; index ranges partition the
      merged index buffer with no gaps or overlaps; every index stays in vertex range.
- [ ] The renderer builds one material bind group per distinct submesh key and the mesh pass records
      one draw per submesh; a model with N materials renders all N (not just the first).
- [ ] A glTF carrying `extras` `{ "tags": ["a","b"] }` spawns an entity whose `get_tags` returns
      those tags; absent/malformed `extras` spawns with no tags and no error.
- [ ] Malformed/unsupported glTF still returns `Err`, never panics (slice invariant preserved).
- [ ] `cargo build` and `cargo test` pass workspace-wide (the `gltf` `extras` feature change builds
      every crate).
- [ ] **Manual-visual:** the dev model renders unchanged from the slice — upright, textured, animated
      (this also clears the slice's run-pending coordinate gate).

## Tasks

### Task 1: Image-decode-free loader entry
Swap `load_model`'s `gltf::import` for `gltf::Gltf::open` + `gltf::import_buffers` (base = glTF
parent dir). All existing readers (positions, skin, animation) consume the returned `buffer::Data`
unchanged. No image data loaded. This is the foundation the material task builds on (it needs the
parent dir + the no-decode guarantee).

### Task 2: Runtime content-hash materials + submesh ranges
Add a `blake3` dependency to the `postretro` crate. Replace `staged_material_key` with a resolver:
base-color URI → `parent_dir.join(uri)` → read bytes → `blake3` hex (zero sentinel when the URI is
absent, embedded, or the file is unreadable). In `load_mesh`, record a per-primitive submesh range
(key + `[start, len)` into the merged index buffer) alongside the merged stream. Restructure
`LoadedModel` to carry submesh ranges instead of a bare `material_keys` vec. Update
`resolve_skinned_model_material` → resolve every submesh key to a `LoadedTexture`/bind group, and
`mesh_pass` to store per-submesh `(bind_group, index_range)` and iterate them in `record_draw`
(single instance, base index 0 — unchanged).

### Task 3: `extras` → entity tags
Enable `features = ["extras"]` on the workspace `gltf` dependency. In the loader, read the model's
top-level `extras` `RawValue`, `serde_json`-parse a `{ "tags": [String] }` shape, and carry
`tags: Vec<String>` on `LoadedModel`. Plumb the tags to the spawn seam: `load_skinned_model` returns
`Option<Vec<String>>` (None = load failed / no spawn; `Some(tags)` = success), and
`spawn_mesh_entity_if_loaded` applies them via `try_spawn(transform, &tags)` (replacing its `bool`
gate). Update the single caller (`main.rs` level-install).

## Sequencing

**Phase 1 (sequential):** Task 1 — the no-decode entry + parent-dir handle everything else needs.
**Phase 2 (concurrent):** Task 2, Task 3 — materials/submeshes and extras/tags are independent
(different `LoadedModel` fields, different call sites). Each extends `LoadedModel` and the loader,
so coordinate the one struct's field additions.

## Rough sketch

- Loader: `crates/postretro/src/model/gltf_loader.rs`. `LoadedModel` swaps `material_keys: Vec<String>`
  for a `submeshes: Vec<Submesh>` (`{ material_key: String, indices: Range<u32> }` or equivalent) and
  gains `tags: Vec<String>`. `load_mesh` returns submesh ranges; `load_model` reads `document.extras()`.
- Material key: mirror `filename_key_for`'s diffuse case — `*blake3::hash(png_bytes).as_bytes()`
  hex-encoded. The hex→`[u8;32]` round-trips through the existing `parse_blake3_key`.
- Renderer: `crates/postretro/src/render/mod.rs` `resolve_skinned_model_material` becomes a
  per-submesh loop (reuse `load_textures` + `build_material_bind_group` per distinct key);
  `crates/postretro/src/render/mesh_pass.rs` `UploadedModel` holds `Vec<(BindGroup, Range<u32>)>`;
  `record_draw` sets each submesh's group 1 + draws its index range.
- Tags: existing `registry.rs` `try_spawn(transform, tags)` / `set_tags`. No new entity surface.
- Coordinate basis stays identity (`mesh_pass.rs:19–30`, verified) — the manual-visual gate is the
  only outstanding confirmation.

## Boundary inventory

| Name | glTF wire | Rust | JS / Luau | FGD KVP |
|---|---|---|---|---|
| model tags | `extras.tags` (JSON `string[]`) | `LoadedModel.tags: Vec<String>` → entity tags | `world.query({ tag })` (existing) | n/a |
| base-color key | image URI → PNG bytes | `blake3` hex `String` → `[u8;32]` | n/a | n/a |

`extras.tags` strings pass through verbatim (author-defined; same vocabulary as map `_tags`). No new
casing rules — tags reuse the existing entity-tag channel end to end.

## Open questions

- **`extras` tag shape.** This spec reads `extras.tags` as a `string[]` to mirror map `_tags`. If a
  later task (skeletal hit zones) needs per-*node* extras (`head` / `limb` on a specific bone), that
  is a different, per-node read it owns — model-level tags here do not preclude it. Confirm the
  `tags` key name is the desired convention before promotion.
- **`load_skinned_model` return type.** Returning `Option<Vec<String>>` couples a renderer method to
  entity metadata. Acceptable because the seam is explicitly temporary (the next task rewrites the
  spawn/handle path); flagged in case a cleaner load-game-side split is preferred now.
