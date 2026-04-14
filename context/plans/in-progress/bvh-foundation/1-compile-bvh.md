# Sub-plan 1 — Compile-time BVH

> **Parent plan:** [BVH Foundation](./index.md) — read first for goals and architectural commitments.
> **Scope:** all compile-time work. `prl-build` builds a global BVH over static geometry, flattens it, and writes a new `Bvh` PRL section. Retires `chunk_grouping.rs` and `CellChunks`. Folds in Milestone 3.5 review finding #1.
> **Crates touched:** `postretro-level-compiler`, `postretro-level-format`. No engine code in this sub-plan.
> **Depends on:** nothing — Milestone 3.5's `GeometryV3` vertex format is reused unchanged.
> **Blocks:** sub-plan 2 (runtime needs the `Bvh` section to load).

---

## Description

After portal generation in `prl-build`, collect all face-indices into BVH primitives. Each primitive carries `(cell_id, material_bucket_id, index_range, AABB)` where `index_range` is a contiguous slice of the shared index buffer containing triangles for that `(face, material_bucket)` pair, and `cell_id` is the face's `leaf_index` (the BSP leaf index assigned during geometry extraction — see `FaceMetaV3.leaf_index`). Build a global BVH over these primitives using the `bvh` crate (SAH-driven, CPU, deterministic). Flatten the resulting tree into two arrays: a dense node array and a dense leaf array. Define and serialize a new `Bvh` PRL section in `postretro-level-format`. Wire the section into the pack stage so every test map emits one.

This sub-plan deletes `postretro-level-compiler/src/chunk_grouping.rs` entirely. Its role is subsumed by the BVH primitive collection step — there is no `(cell, material_bucket)` grouping anymore, only BVH leaves.

### `cell_id` provenance

Each face in the geometry pipeline is already tagged with `FaceMetaV3.leaf_index` — the BSP leaf index assigned during `extract_geometry()` in `geometry.rs`. That index is the runtime cell ID (the same value the engine's `find_leaf()` returns). Faces never straddle BSP leaf boundaries: the geometry extractor walks each hull down the BSP tree and emits one face per empty-leaf fragment, so a face always belongs to exactly one cell. BVH primitive collection reads `leaf_index` directly from each face's `FaceMetaV3`; no new face→cell tracking is needed.

### Finding #1 drive-by

The compiler's face → index pipeline is rewritten end-to-end in this sub-plan. `FaceMetaV3.index_offset/index_count` are **retired** — they are not carried into the new `BvhSection` and will be removed from the `GeometryV3` format. All index ranges are now owned by BVH leaves. The Milestone 3.5 review's critical finding lands here as a side effect, not as a separate task.

---

## PRL section layout

New `Bvh` section in `postretro-level-format`:

```
Header (fixed):
  u32        node_count
  u32        leaf_count
  u32        root_node_index
  u32        padding

Node array (node_count entries):     — stride: 40 bytes
  f32 × 3    aabb_min.xyz                   (12 bytes, offset  0)
  u32        skip_index                      ( 4 bytes, offset 12)
  f32 × 3    aabb_max.xyz                   (12 bytes, offset 16)
  u32        left_child_or_leaf_index        ( 4 bytes, offset 28)
  u32        flags          (bit 0: is_leaf) ( 4 bytes, offset 32)
  u32        padding        (reserved, 0)   ( 4 bytes, offset 36)

  Field semantics:
  - Internal nodes (flags & 1 == 0): `left_child_or_leaf_index` is unused — zero it.
    Left child is always at `current_index + 1` in DFS order; `skip_index` is the
    right-subtree entry point (jump here on AABB reject).
  - Leaf nodes (flags & 1 != 0): `left_child_or_leaf_index` = index into `leaf_array`.
    `skip_index` is still valid (points past this node for DFS continuation).

Leaf array (leaf_count entries):      — stride: 40 bytes
  f32 × 3    aabb_min.xyz                   (12 bytes, offset  0)
  u32        material_bucket_id             ( 4 bytes, offset 12)
  f32 × 3    aabb_max.xyz                   (12 bytes, offset 16)
  u32        index_offset                   ( 4 bytes, offset 28)
  u32        index_count                    ( 4 bytes, offset 32)
  u32        cell_id                        ( 4 bytes, offset 36)
```

The interleaved layout (vec3 + u32 pairs) matches the WGSL storage buffer struct layout exactly — `vec3<f32>` in a storage buffer occupies 12 bytes (not 16), so no padding is inserted between fields. Strides are 40 bytes (6×f32 + 4×u32); natural (4-byte) alignment throughout; no additional padding required.

Leaves are sorted by `material_bucket_id` in the flat leaf array, so each bucket owns a contiguous slice. At load time the runtime scans the sorted leaf array once (O(leaf_count)) to build a per-bucket `(first_slot, count)` table; this table is not stored in the PRL section. The indirect buffer is sized to `leaf_count` slots (one permanent slot per leaf, indexed by leaf array position). See "Data contracts" in `index.md` for slot-range bookkeeping.

`material_bucket_id` is an index into the level's material bucket list — each bucket is an `(albedo_texture, normal_map_texture)` pair. The existing geometry pipeline assigns a `material_bucket_id` per face at brush-side projection time (see `geometry.rs`). BVH primitive collection reads it from the face's metadata; no new assignment logic is needed.

Allocate a new section id. Retire `SectionId::CellChunks = 18` — delete the variant and all read/write code. No backward compat: maps compiled before this sub-plan ships will fail to load.

The layout is serialized in leaf-sorted-by-bucket, depth-first-node order so the runtime can upload it directly to GPU storage buffers with no post-processing.

---

## Acceptance criteria

- [ ] `bvh` crate added to `postretro-level-compiler/Cargo.toml`
- [ ] BVH builds deterministically: identical input geometry produces identical flattened node/leaf arrays byte-for-byte
- [ ] Every triangle in the source geometry maps to exactly one BVH leaf
- [ ] BVH leaf AABBs tightly bound each leaf's triangle set
- [ ] `chunk_grouping.rs` removed; no references remain in the compiler
- [ ] `FaceMetaV3.index_offset/index_count` retired — fields removed from `GeometryV3` format; no callers remain
- [ ] New `Bvh` section id allocated in `postretro-level-format/src/lib.rs`
- [ ] `SectionId::CellChunks` variant and all supporting code deleted
- [ ] `Bvh` section write → read round-trip preserves all fields byte-for-byte
- [ ] Truncated-section and malformed-header inputs reject cleanly with a clear error
- [ ] Leaf array is sorted by `material_bucket_id`; each bucket's slot range is contiguous
- [ ] Every test map in `assets/maps/` compiles and emits a `Bvh` section
- [ ] `cargo test -p postretro-level-compiler` passes
- [ ] `cargo test -p postretro-level-format` passes
- [ ] `GeometrySectionV3` → `GeometrySection`, `VertexV3` → `Vertex`, `FaceMetaV3` → `FaceMeta` renamed throughout both crates; no `V3` type names remain
- [ ] `SectionId::GeometryV3` → `SectionId::Geometry`; `SectionId::GeometryV2` and `GeometrySectionV2`/`FaceMetaV2` deleted along with all dead legacy code
- [ ] `cargo clippy -p postretro-level-compiler -p postretro-level-format -- -D warnings` clean

---

## Implementation tasks

1. Add `bvh = "..."` to `postretro-level-compiler/Cargo.toml`.

2. Implement BVH primitive collection in the geometry pipeline: walk face/index data, emit one primitive per `(face, material_bucket)` pair with its `index_range`, `AABB`, and `cell_id` (= `FaceMetaV3.leaf_index`). Feed into `bvh::Bvh::build`.

3. Flatten the built tree into a dense node array and a dense leaf array. Sort leaves by `material_bucket_id` before writing so each bucket's slot range is contiguous (required for `multi_draw_indexed_indirect`). The flattening must be deterministic — identical input → identical buffer byte-for-byte.

4. Delete `postretro-level-compiler/src/chunk_grouping.rs` and all references. Remove `FaceMetaV3.index_offset` and `FaceMetaV3.index_count` from `GeometryV3` — these fields are retired; all index ranges are now owned by BVH leaves.

5. Unit tests for compiler: deterministic build, leaf-primitive coverage (every source triangle in exactly one leaf), AABB tightness, single-face and multi-texture test fixtures.

6. Add `SectionId::Bvh` in `postretro-level-format/src/lib.rs`. Delete `SectionId::CellChunks`, `cell_chunks.rs`, and all read/write callers.

7. Implement `BvhSection` with the layout above. Write/read round-trip tests (byte-identical), truncation rejection, malformed header rejection. Match the existing section test patterns in `postretro-level-format`.

8. Wire `BvhSection` into the `prl-build` pack stage. Confirm every test map compiles and emits a `Bvh` section.

9. Rename versioned types throughout `postretro-level-format` and `postretro-level-compiler`: `GeometrySectionV3` → `GeometrySection`, `VertexV3` → `Vertex`, `FaceMetaV3` → `FaceMeta`, `SectionId::GeometryV3` → `SectionId::Geometry`. Delete `GeometrySectionV2`, `FaceMetaV2`, and `SectionId::GeometryV2` — the V2 legacy load path in `postretro/src/prl.rs` becomes dead code once the `Bvh` section is required (pre-V3 maps can't emit one and will fail to load regardless); delete it there too.

---

## Notes for implementation

- **Deterministic build.** The `bvh` crate is SAH-driven and should be deterministic given identical input, but verify with the round-trip test: build twice, compare buffers byte-for-byte. If non-determinism surfaces, the cause is almost certainly input ordering — sort primitives by a stable key before feeding the builder.
- **Multi-texture UV normalization.** Finding #1 from the Milestone 3.5 review involved UVs going stale when faces were reordered. The new pipeline should normalize UVs at primitive collection time, before BVH construction, so the BVH leaf's index range points at vertices with already-correct UVs. Verify on a two-texture test map with different `(w, h)` dimensions.
- **`cell_id` is already available.** `FaceMetaV3.leaf_index` carries the BSP leaf index assigned during geometry extraction. This is the runtime cell ID. No new face→cell tracking work is required; the primitive collector just reads it directly.
- **Baker handoff (Milestone 5).** The built `bvh::Bvh` object and its primitive collection must be accessible to the SH baker (`lighting-foundation/2-sh-baker.md`), which is a sibling module in `postretro-level-compiler`. Structure the implementation so that `bvh::Bvh::build` and the primitive `Vec` are produced by a public function (e.g., `pub fn build_bvh(geometry: &CompiledGeometry) -> (bvh::Bvh, Vec<BvhPrimitive>)`) that both the flattening/pack stage and the baker can call. The baker traverses the live tree on the CPU; the pack stage flattens it to `BvhSection`. Do not force the baker to round-trip through the PRL section.
