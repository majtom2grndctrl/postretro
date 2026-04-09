# Task 02: Compiler Texture Data

> **Phase:** PRL Texture Support
> **Dependencies:** Task 01 (format extensions).
> **Produces:** Updated `postretro-level-compiler` that writes UV data and texture names into compiled `.prl` files.

---

## Goal

Extract texture projection data from the map parser, compute UVs at compile time, and write the texture name table and per-vertex UVs to the PRL binary using the format types from Task 01.

---

## Implementation Guidance

### What shambler provides

The map parser (shambler/shalrath) exposes texture projection data per face. Inspect what fields are available on the face type returned by shambler — specifically the texture-axis vectors, offsets, and scale that define `PlanarTextureProjection`. The compiler's `parse.rs` currently extracts only `texture: String` from each face; the projection fields are present but discarded.

Read `parse.rs` to find where face data is extracted from shambler. The shambler face likely has fields like `u_axis`, `v_axis`, `u_offset`, `v_offset`, `u_scale`, `v_scale` (or equivalent). Add these to the compiler's `Face` struct in `map_data.rs`.

### Face struct extension

In `postretro-level-compiler/src/map_data.rs`, extend `Face` with projection fields:

```rust
pub struct Face {
    pub vertices: Vec<Vec3>,   // Vertex positions (engine space, post-transform)
    pub normal: Vec3,
    pub distance: f32,
    pub texture: String,
    // New:
    pub tex_u_axis: Vec3,      // Quake-space U projection axis
    pub tex_v_axis: Vec3,      // Quake-space V projection axis
    pub tex_u_offset: f32,
    pub tex_v_offset: f32,
    pub tex_scale_u: f32,
    pub tex_scale_v: f32,
}
```

The exact field names depend on what shambler exposes — match the source. Store them in **Quake space** (Z-up) so UV computation can use them directly without re-converting.

Also store the **Quake-space vertex positions** alongside the engine-space ones, or apply the inverse coordinate transform before UV projection. The existing coordinate transform converts vertices to engine space (Y-up) before storing — UV projection needs the original Quake-space positions.

Simplest approach: keep engine-space vertices in `Face.vertices` (used for geometry), and apply the inverse transform (engine → Quake) during UV computation. Alternatively, keep a separate `quake_vertices: Vec<Vec3>` field in `Face`. Either is fine — use judgment on what's cleaner.

### Geometry extraction UV computation

In `postretro-level-compiler/src/geometry.rs`, the `extract_geometry()` function currently produces vertices as `[f32; 3]` (position only). Change to `[f32; 5]` using `GeometrySectionV2`.

For each vertex, compute UV:

```
u = dot(quake_pos, face.tex_u_axis) / face.tex_scale_u + face.tex_u_offset
v = dot(quake_pos, face.tex_v_axis) / face.tex_scale_v + face.tex_v_offset
```

Then normalize by texture dimensions:
```
uv_normalized = [u / tex_width, v / tex_height]
```

Texture dimensions are not known at compiler time (PNGs are a runtime asset). Store **un-normalized texel-space UVs** in the PRL file. The engine normalizes them at load time using the loaded texture's dimensions — same as it would if projection were done at runtime.

Alternatively, store a 1/width and 1/height scale in the `TextureNames` section per texture. This avoids engine-side normalization but requires the compiler to know texture dimensions. Since texture dimensions are not available to the compiler, **store un-normalized UVs and let the engine normalize**.

Update `FaceMeta` to include `texture_index` — an index into the deduplicated texture name list built during compilation.

### Texture name deduplication

Build a `Vec<String>` of unique texture names in encounter order. For each face, look up or insert the face's texture name to get its index. This is the list written as the `TextureNames` section.

### Pack step

In `pack.rs` (or wherever the PRL is assembled), write:
- `GeometrySectionV2` (ID 3) instead of `Geometry` (ID 1)
- `TextureNames` (ID 16) with the deduplicated name list

---

## Key Decisions

| Item | Resolution |
|------|------------|
| UV normalization | Store texel-space UVs (un-normalized). Engine normalizes at load time using loaded texture dimensions. |
| Quake-space UV computation | Use face projection axes/offsets extracted from shambler. Apply to Quake-space positions. |
| Texture dimensions at compile time | Not available — do not require PNG files at compile time. |
| Duplicate texture names | Deduplicate in compiler; one entry per unique name in TextureNames section. |

---

## Acceptance Criteria

1. Compiling a `.map` with textured brushes produces a `.prl` containing a `GeometrySectionV2` section with 5-float vertices and a `TextureNames` section.
2. UV values in vertex data are non-zero for faces with non-axis-aligned texture projections.
3. Different faces referencing the same texture name get the same `texture_index`.
4. Faces with no texture data (if any) get `texture_index = u32::MAX`.
5. `cargo check` and `cargo test` pass for `postretro-level-compiler`.
6. Existing compiler tests still pass (geometry structure unchanged; only vertex layout extends).
