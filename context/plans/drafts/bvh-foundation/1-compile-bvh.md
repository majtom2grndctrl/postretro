# Sub-plan 1 — Compile-time BVH

> **Parent plan:** [BVH Foundation](./index.md) — read first for goals and architectural commitments.
> **Scope:** all compile-time work. `prl-build` builds a global BVH over static geometry, flattens it, and writes a new `Bvh` PRL section. Retires `chunk_grouping.rs` and `CellChunks`. Folds in Milestone 3.5 review finding #1.
> **Crates touched:** `postretro-level-compiler`, `postretro-level-format`. No engine code in this sub-plan.
> **Depends on:** nothing — Milestone 3.5's `GeometryV3` vertex format is reused unchanged.
> **Blocks:** sub-plan 2 (runtime needs the `Bvh` section to load).

---

## Description

After portal generation in `prl-build`, collect all face-indices into BVH primitives. Each primitive carries `(material_bucket_id, index_range, AABB)` where `index_range` is a contiguous slice of the shared index buffer containing triangles for that `(face, material_bucket)` pair. Build a global BVH over these primitives using the `bvh` crate (SAH-driven, CPU, deterministic). Flatten the resulting tree into two arrays: a dense node array and a dense leaf array. Define and serialize a new `Bvh` PRL section in `postretro-level-format`. Wire the section into the pack stage so every test map emits one.

This sub-plan deletes `postretro-level-compiler/src/chunk_grouping.rs` entirely. Its role is subsumed by the BVH primitive collection step — there is no `(cell, material_bucket)` grouping anymore, only BVH leaves.

### Finding #1 drive-by

The compiler's face → index pipeline is rewritten end-to-end in this sub-plan. `FaceMetaV3.index_offset/index_count` either get rewritten consistently during BVH construction or are retired from the `GeometryV3` section entirely (decided during implementation). Either way, the Milestone 3.5 review's critical finding lands here as a side effect, not as a separate task.

---

## PRL section layout

New `Bvh` section in `postretro-level-format`:

```
Header (fixed):
  u32        node_count
  u32        leaf_count
  u32        root_node_index
  u32        padding

Node array (node_count entries):
  f32 × 6    aabb_min.xyz, aabb_max.xyz
  u32        left_child_or_leaf_index
  u32        right_child_or_leaf_index
  u32        flags          (bit 0: is_leaf)
  u32        padding

Leaf array (leaf_count entries):
  f32 × 6    aabb_min.xyz, aabb_max.xyz
  u32        material_bucket_id
  u32        index_offset
  u32        index_count
  u32        padding
```

Allocate a new section id. Retire `SectionId::CellChunks = 18` — delete the variant and all read/write code. No backward compat: maps compiled before this sub-plan ships will fail to load.

The layout is serialized in tree order so the runtime can upload it directly to a GPU storage buffer with no post-processing.

---

## Acceptance criteria

- [ ] `bvh` crate added to `postretro-level-compiler/Cargo.toml`
- [ ] BVH builds deterministically: identical input geometry produces identical flattened node/leaf arrays byte-for-byte
- [ ] Every triangle in the source geometry maps to exactly one BVH leaf
- [ ] BVH leaf AABBs tightly bound each leaf's triangle set
- [ ] `chunk_grouping.rs` removed; no references remain in the compiler
- [ ] `FaceMetaV3.index_offset/index_count` staleness bug is no longer reachable (either fixed or retired)
- [ ] New `Bvh` section id allocated in `postretro-level-format/src/lib.rs`
- [ ] `SectionId::CellChunks` variant and all supporting code deleted
- [ ] `Bvh` section write → read round-trip preserves all fields byte-for-byte
- [ ] Truncated-section and malformed-header inputs reject cleanly with a clear error
- [ ] Every test map in `assets/maps/` compiles and emits a `Bvh` section
- [ ] `cargo test -p postretro-level-compiler` passes
- [ ] `cargo test -p postretro-level-format` passes
- [ ] `cargo clippy -p postretro-level-compiler -p postretro-level-format -- -D warnings` clean

---

## Implementation tasks

1. Add `bvh = "..."` to `postretro-level-compiler/Cargo.toml`.

2. Implement BVH primitive collection in the geometry pipeline: walk face/index data, emit one primitive per `(face, material_bucket)` pair with its `index_range` and `AABB`. Feed into `bvh::Bvh::build`.

3. Flatten the built tree into a dense node array and a dense leaf array. The flattening must be deterministic — identical input → identical buffer byte-for-byte.

4. Delete `postretro-level-compiler/src/chunk_grouping.rs` and all references. Rewrite `FaceMetaV3` so `index_offset/index_count` either stay consistent through the BVH construction pipeline or get removed from `GeometryV3` entirely (decided during implementation — finding #1 fix lands here as a side effect).

5. Unit tests for compiler: deterministic build, leaf-primitive coverage (every source triangle in exactly one leaf), AABB tightness, single-face and multi-texture test fixtures.

6. Add `SectionId::Bvh` in `postretro-level-format/src/lib.rs`. Delete `SectionId::CellChunks`, `cell_chunks.rs`, and all read/write callers.

7. Implement `BvhSection` with the layout above. Write/read round-trip tests (byte-identical), truncation rejection, malformed header rejection. Match the existing section test patterns in `postretro-level-format`.

8. Wire `BvhSection` into the `prl-build` pack stage. Confirm every test map compiles and emits a `Bvh` section.

---

## Notes for implementation

- **Deterministic build.** The `bvh` crate is SAH-driven and should be deterministic given identical input, but verify with the round-trip test: build twice, compare buffers byte-for-byte. If non-determinism surfaces, the cause is almost certainly input ordering — sort primitives by a stable key before feeding the builder.
- **Multi-texture UV normalization.** Finding #1 from the Milestone 3.5 review involved UVs going stale when faces were reordered. The new pipeline should normalize UVs at primitive collection time, before BVH construction, so the BVH leaf's index range points at vertices with already-correct UVs. Verify on a two-texture test map with different `(w, h)` dimensions.
- **No CPU-side cell metadata in BVH leaves yet.** Sub-plan 2's portal integration may want cell IDs on leaves; if so, add the field then. Don't pre-add it here without a consumer.
