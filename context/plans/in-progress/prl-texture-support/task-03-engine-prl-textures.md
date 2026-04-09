# Task 03: Engine PRL Texture Rendering

> **Phase:** PRL Texture Support
> **Dependencies:** Task 01 (format), Task 02 (compiler produces new PRL files).
> **Produces:** Engine reads PRL texture data and renders textured PRL levels through the same pipeline as BSP.

---

## Goal

Update the engine's PRL loader and `main.rs` integration to read texture names and UV data from `.prl` files, load textures from disk, build `TextureSubRange` lists, and pass the same `LevelGeometry` structure to the renderer that BSP levels produce. No renderer changes required.

---

## Implementation Guidance

### PRL loader changes (`prl.rs`)

`LevelWorld` currently stores `vertices: Vec<[f32; 3]>` and `face_meta: Vec<FaceMeta>` (index_offset, index_count only).

After this task:
- `LevelWorld.vertices` holds `Vec<TexturedVertex>` (position + UV + white vertex_color) — same type as BSP
- `LevelWorld.face_meta` holds the engine's `bsp::FaceMeta` (with texture_index, texture_name, texture_dimensions, material) — same type as BSP

When reading a `GeometrySectionV2` section:
1. For each vertex, read `[f32; 5]` — split into position (3) and raw UVs (2).
2. Read the `TextureNames` section to get the texture name list.
3. For each face, look up its `texture_index` → texture name.
4. Normalize UVs: raw texel-space UVs need `/ texture_dimensions`. At PRL load time the actual PNG dimensions are not yet known — store the raw UVs for now, and normalize after texture loading (in `main.rs` after `load_textures()` returns dimensions). Or normalize in `main.rs` as a post-load pass.

**Simpler alternative:** Store raw UVs (texel-space) in `TexturedVertex.base_uv` temporarily and normalize them in `main.rs` once texture dimensions are known. The BSP path normalizes at construction; the PRL path normalizes as a post-load step. Both are valid.

For the `material` field on `FaceMeta`, use the same `material::derive_material()` call as BSP, passing the texture name string.

When reading a legacy `Geometry = 1` section (old PRL file): populate `TexturedVertex.base_uv` as `[0.0, 0.0]`, `FaceMeta.texture_name` as empty string, `FaceMeta.texture_index` as `None`.

### TextureSubRange build

After loading geometry, call the same logic that BSP uses to build per-leaf `TextureSubRange` lists. The BSP path in `bsp.rs` has:
- `sort_indices_by_leaf_and_texture()` — sort index buffer by (leaf, texture)
- `build_leaf_texture_sub_ranges()` — produce per-leaf sub-range lists

These functions operate on `FaceMeta` and `BspLeafData`/`LeafData`. PRL `LeafData` has the same shape (`face_start`, `face_count`, `texture_sub_ranges`). Either reuse the BSP functions directly (if they're not BSP-specific) or duplicate the logic for PRL `LeafData`. Prefer reuse — move to a shared location if needed.

### Integration in `main.rs`

The PRL path in `main.rs` currently:
1. Converts `world.vertices: Vec<[f32; 3]>` → `Vec<TexturedVertex>` with zero UVs
2. Passes empty `texture_set` and empty `leaf_texture_sub_ranges`

After this task:
1. Extract texture names from `LevelWorld` (populated by loader)
2. Call `texture::load_textures(texture_names, texture_root)` — same as BSP path
3. Normalize UVs in `TexturedVertex` using loaded texture dimensions (post-load pass)
4. Build `TextureSubRange` lists
5. Pass `texture_set` and `leaf_texture_sub_ranges` to renderer

Texture root resolution is the same function (`resolve_texture_root`) as BSP — given `assets/maps/test.prl`, the texture root is `assets/textures/`.

### UV normalization post-load pass

After `load_textures()` returns `TextureSet`:
```
for each vertex:
    let (w, h) = texture_set.textures[face.texture_index].dimensions()
    vertex.base_uv[0] /= w as f32
    vertex.base_uv[1] /= h as f32
```

This requires knowing which texture index each vertex belongs to. Iterate faces, get the face's `texture_index`, normalize that face's vertex range. Keep it simple.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| UV normalization timing | Post-load pass in main.rs after texture dimensions are known |
| Legacy PRL compatibility | Read old Geometry=1 section, fill zero UVs, no texture |
| TextureSubRange build | Reuse or adapt BSP sorting/sub-range functions |
| LevelWorld vertex type | Change to `Vec<TexturedVertex>` to match BSP path |
| Material derivation | Same `derive_material()` call as BSP, no new code |

---

## Acceptance Criteria

1. `cargo run -- assets/maps/test.prl` shows textured PRL geometry with the same textures as the equivalent BSP level.
2. Missing textures produce checkerboard. Warning logged. No crash.
3. Old PRL files (without `GeometryV2` or `TextureNames` sections) still load and render (solid-unlit or checkerboard).
4. PVS and frustum culling still work. Draw counts change when navigating between rooms.
5. Module boundary holds: `prl.rs` contains no texture loading logic; texture loading stays in `texture.rs` and `main.rs`.
6. `cargo check`, `cargo clippy -- -D warnings`, and `cargo test` pass for the full workspace.
