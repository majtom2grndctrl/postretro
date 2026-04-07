# Task 05: New PRL Sections

**Crate:** `postretro-level-format`
**Files:** `src/lib.rs` · new section modules
**Depends on:** Task 04

---

## Context

The PRL format currently has two live sections: Geometry (ID 1) and ClusterVisibility (ID 2). ClusterVisibility is retired. Three new sections are added to carry the BSP tree, leaf metadata, and leaf PVS. The geometry section (ID 1) is retained but gains a leaf-grouping invariant — faces are ordered contiguously by leaf so the engine can draw per-leaf without an index lookup.

The existing RLE compression codec in `postretro-level-format/src/visibility.rs` is retained and reused for the leaf PVS section.

---

## What to Build

### New section IDs (add to `src/lib.rs` constants)

| Constant | ID | Contents |
|----------|----|----------|
| `SECTION_BSP_NODES` | 12 | Flat array of BSP interior nodes |
| `SECTION_BSP_LEAVES` | 13 | Flat array of BSP leaf records |
| `SECTION_LEAF_PVS` | 14 | Per-leaf RLE-compressed PVS bitsets |

IDs 12–14 are chosen to leave the reserved range 3–10 available for future use (textures, lighting, nav mesh, etc.).

Retire `SECTION_CLUSTER_VISIBILITY` (ID 2) and `SECTION_VISIBILITY_CONFIDENCE` (ID 11). Keep the constants defined but mark them `#[deprecated]` or document them as retired. Do not remove — old .prl files may have these sections; the engine must skip them gracefully (unknown section IDs are already skipped by the loader).

### `src/bsp.rs` (new file)

**`BspNodesSection`** — serializes/deserializes the flat node array. Each node record:
- `plane_normal: [f32; 3]`
- `plane_distance: f32`
- `front: i32` — positive = node index; negative = `(-1 - leaf_index)` (sentinel encoding for leaves)
- `back: i32` — same encoding

**`BspLeavesSection`** — serializes/deserializes the flat leaf array. Each leaf record:
- `face_start: u32` — index into the geometry section's face list
- `face_count: u32`
- `bounds_min: [f32; 3]`
- `bounds_max: [f32; 3]`
- `pvs_offset: u32` — byte offset into the LeafPvs section blob
- `pvs_size: u32` — byte length of this leaf's RLE-compressed PVS
- `is_solid: u8` — 1 if solid, 0 if empty (padding to align)

The leaf index is implicit (position in the array). Leaf 0 = leaves[0], etc.

### `src/leaf_pvs.rs` (new file)

**`LeafPvsSection`** — stores the concatenated RLE-compressed PVS bitsets for all empty leaves. Solid leaves have `pvs_offset = 0` and `pvs_size = 0` in their `BspLeavesSection` record. Reuse the existing `rle_compress` / `rle_decompress` functions from `src/visibility.rs`.

### Geometry section — leaf ordering invariant

Add documentation (not a format change) stating that faces in the geometry section are ordered contiguously by leaf: all faces for leaf 0 come first, then all faces for leaf 1, etc. The `face_start` / `face_count` fields in `BspLeavesSection` index into this ordering. The compiler (Task 04 output → pack stage) must emit faces in this order.

### `src/pack.rs` in the compiler

Update `pack_and_write` (in `postretro-level-compiler`) to write the three new sections in order: Geometry (ID 1), BspNodes (ID 12), BspLeaves (ID 13), LeafPvs (ID 14). Remove writing of ClusterVisibility (ID 2) and VisibilityConfidence (ID 11).

---

## Acceptance Criteria

- `cargo check` and `cargo test` for both `postretro-level-format` and `postretro-level-compiler` pass.
- Unit test in `postretro-level-format`: write a minimal BspNodesSection, read it back, verify round-trip. Same for BspLeavesSection and LeafPvsSection.
- Read-back validation in `pack.rs` passes for a compiled test map (existing read-back validation pattern in `pack.rs` — extend to cover new sections).
- `prl-build assets/maps/test.map -o assets/maps/test.prl` produces a file containing all three new sections.
- The produced .prl file does not contain section IDs 2 or 11.
- `cargo test` — zero failures.
