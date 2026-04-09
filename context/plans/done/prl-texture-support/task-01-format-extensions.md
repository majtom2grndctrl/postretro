# Task 01: PRL Format Extensions

> **Phase:** PRL Texture Support
> **Dependencies:** none.
> **Produces:** Updated `postretro-level-format` crate with extended vertex format, extended `FaceMeta`, and new `TextureNames` section. Consumed by Task 02 (compiler writes) and Task 03 (engine reads).

---

## Goal

Extend the PRL binary format in `postretro-level-format` to carry texture data. Three changes: the geometry section's vertex format gains UV coordinates; `FaceMeta` gains a texture index; a new section stores the texture name string table.

---

## Implementation Guidance

### Geometry section vertex format

`GeometrySection` currently stores `vertices: Vec<[f32; 3]>`.

Change to `Vec<[f32; 5]>` — adding `[u, v]` after the position. This is a **breaking change** to the section's binary layout.

Handle the break by adding a new section ID `GeometryV2 = 3` (leaving `Geometry = 1` unchanged for backwards compatibility). The engine reads whichever version is present; old files with `Geometry = 1` fall back to zero UVs.

Rename the type to `GeometrySectionV2` (or similar) and keep the old type for reading legacy files if needed. The compiler writes `GeometryV2`; the engine reads `GeometryV2` if present, falls back to `Geometry` otherwise.

### FaceMeta extension

`FaceMeta` in the geometry section currently has:
```rust
pub struct FaceMeta {
    pub index_offset: u32,
    pub index_count: u32,
    pub leaf_index: u32,
}
```

Add `texture_index: u32`. Use `u32::MAX` as the sentinel for "no texture" (produces checkerboard in the engine).

This field is only present in `GeometryV2` — not in the legacy `Geometry = 1` section.

### TextureNames section

Add a new section with ID `TextureNames = 16`. It stores a flat list of texture name strings:

```
Header:
  count: u32  — number of texture names

Body (for each name):
  length: u32  — byte length of name string (excluding null terminator)
  data: [u8]   — UTF-8 bytes
```

Reading: decode each string in order; index in the list matches `FaceMeta.texture_index`.

Keep it simple — no deduplication logic in the format layer. The compiler is responsible for building a deduplicated list.

### Backwards compatibility

Files without a `TextureNames` section load without error. The engine treats all faces as having no texture (checkerboard fallback). This preserves loading of existing `.prl` files compiled before this change.

Files with `Geometry = 1` (old layout) and no `TextureNames` section: engine reads positions, fills UVs to zero.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Vertex format change | New section ID `GeometryV2 = 3` to avoid breaking old files |
| FaceMeta texture_index missing-texture sentinel | `u32::MAX` |
| TextureNames encoding | Length-prefixed UTF-8 strings |
| Old file compatibility | Load without crash; fall back to checkerboard |

---

## Acceptance Criteria

1. `GeometrySectionV2` round-trips with 5-float vertices and extended `FaceMeta` (all fields preserved).
2. `TextureNames` section round-trips: a list of strings written then read back produces identical strings in order.
3. Reading a file with old `Geometry = 1` section succeeds; old format struct is unchanged.
4. Reading a file with no `TextureNames` section succeeds; texture list is empty.
5. All existing `postretro-level-format` tests pass.
