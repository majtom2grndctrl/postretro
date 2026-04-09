# PRL Texture Support

> **Status:** ready
> **Depends on:** Phase 3 (textured BSP rendering, texture loader, render pipeline).
> **Related:** `context/lib/rendering_pipeline.md` · `context/lib/build_pipeline.md` · `context/lib/development_guide.md`

---

## Goal

Add texture data to the PRL pipeline so that `.prl` levels render with the same textured appearance as BSP levels. Phase 3 shipped the full GPU rendering path (shader, texture loader, bind groups, draw batching) keyed off `FaceMeta` and `TexturedVertex`. This plan wires texture data through the one place it's currently missing: the PRL binary format and the compiler that produces it.

**Current state:** PRL levels render with zero UVs and placeholder textures (per the Phase 3 PRL fallback).
**After this plan:** PRL levels load textures from disk by name, compute UVs at compile time, and render with the same textured pipeline as BSP.

---

## Scope

### In scope

- Extract texture projection vectors (s_axis, t_axis, offsets, scale) from shambler's face data in the compiler
- Compute UVs at compile time; store in PRL vertex data
- Add texture names section to PRL binary format (section ID 16)
- Extend `GeometrySection` vertex format from `[f32; 3]` to `[f32; 5]` (position + UV)
- Extend format `FaceMeta` to include `texture_index: u32`
- Engine PRL loader reads new sections, builds `TextureSubRange` lists, loads textures
- PRL rendering path uses same `LevelGeometry` → renderer flow as BSP

### Out of scope

- Lightmap UVs (no second UV channel; same as BSP Phase 3 scope)
- Material derivation for PRL faces (already works: `derive_material` reads `texture_name` from `FaceMeta`, which this plan populates)
- Animated textures, emissive bypass (Phase 5+)
- Tool texture filtering in PRL compiler (follow-up if needed)

---

## Shared Context

### Coordinate space

PRL vertex positions are already stored in engine space (Y-up, meters). UV computation must use **Quake-space coordinates** (Z-up, before transform) for the same reason as BSP: `PlanarTextureProjection::project()` expects Quake-space input. The compiler holds both representations — project UVs before applying the coordinate transform.

### Vertex format change

`GeometrySection` currently stores vertices as `Vec<[f32; 3]>`. This changes to `Vec<[f32; 5]>` — the two added floats are normalized base UVs. The engine maps this directly to `TexturedVertex { position, base_uv, vertex_color }` at load time (vertex_color filled to white, same as BSP).

This is a **breaking format change**. Version the section or add a new section ID to distinguish from old geometry sections. Recommended: bump the geometry section's internal version field (if one exists) or use a new section ID (`GeometryV2 = 3`).

### Texture names section

A new `TextureNames` section (ID 16) stores a flat list of null-terminated or length-prefixed texture name strings. `FaceMeta.texture_index` is an index into this list. `u32::MAX` is the sentinel for "no texture" (produces checkerboard fallback in the engine).

### FaceMeta in format crate

`postretro-level-format`'s `FaceMeta` (currently: `index_offset`, `index_count`, `leaf_index`) gains a `texture_index: u32` field. This is a binary format change — version or section-ID guard it.

### Engine render path

The engine's PRL render path in `main.rs` currently converts `LevelWorld.vertices: Vec<[f32; 3]>` to `TexturedVertex` with zero UVs. After this plan:
1. PRL loader reads extended vertices (position + UV) and texture name list
2. Engine populates `FaceMeta` with `texture_name`, `texture_index`, `texture_dimensions`, `material` (same fields as BSP)
3. Engine builds `TextureSubRange` lists per leaf (same function as BSP path)
4. Engine loads textures via `texture::load_textures()` using same texture root resolution
5. `LevelGeometry` passed to renderer is identical in structure to BSP path

The renderer itself requires no changes.

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | PRL format extensions | `task-01-format-extensions.md` | none | Extend `GeometrySection` vertex format, `FaceMeta`, and add `TextureNames` section. |
| 02 | Compiler texture data | `task-02-compiler-texture-data.md` | 01 | Extract projection vectors from shambler, compute UVs, write texture sections to PRL. |
| 03 | Engine PRL texture rendering | `task-03-engine-prl-textures.md` | 01, 02 | Engine reads new PRL sections, builds TextureSubRanges, loads textures, renders textured. |

---

## Execution Order

All three tasks are sequential:

```
+------------------+     +------------------+     +------------------+
| 01 Format        | --> | 02 Compiler      | --> | 03 Engine        |
| extensions       |     | texture data     |     | PRL textures     |
+------------------+     +------------------+     +------------------+
```

Task 01 defines the binary contract that 02 writes and 03 reads. Tasks 02 and 03 cannot start until 01 is done.

---

## Acceptance Criteria

1. `cargo run -p postretro -- assets/maps/test.prl` shows textured PRL geometry with uniform flat lighting. Same visual result as BSP.
2. Missing textures produce checkerboard placeholder (same as BSP). No crash.
3. PRL levels compiled from `.map` files with the updated compiler contain UV data and texture names.
4. Old PRL files (without texture sections) still load without crashing — engine falls back to checkerboard/solid-unlit if texture section is absent.
5. Module boundary holds: texture loading and UV computation are not mixed into the renderer.
6. All existing PRL tests pass. No regression on BSP rendering.

---

## Open Questions

None identified. Phase 3 resolved all analogous questions for BSP; answers carry directly.
