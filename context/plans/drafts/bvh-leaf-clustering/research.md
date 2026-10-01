# BVH Leaf Clustering — research notes

Derivation behind `index.md`. Not handed to task agents.

## CPU encode evidence (2026-10-01)

Host: Radeon Pro 5300M, Metal, cargo `--release` engine build, render scale Auto. Profiles taken with `sample` on the main thread during steady play at spawn. `[CpuTiming]` windows taken in separate, untraced runs (`POSTRETRO_CPU_TIMING=1 RUST_LOG=info`), first 15 s skipped.

**Draw count.** Depth prepass and forward each call `ComputeCullPipeline::draw_indirect` → `compute_cull::draw_indirect_buckets`, which issues `multi_draw_indexed_indirect(buffer, offset, range.leaf_count)` per material bucket. `range.leaf_count` covers every leaf in the bucket, culled ones included: the cull zeroes a culled leaf's args, it does not drop the draw. Each uncached shadow slot does the same over its own sub-region (`shadow_cull.rs`). Draws per frame = 2·L + L·(uncached shadow slots).

| Map | L (BVH leaves) | Draws/frame (no uncached slots) |
|---|---|---|
| `stress-warren-hallway-inspection` | 8,437 | 16,874 |
| `campaign-test` | 774 | 1,548 |

**Backend.** wgpu 29.0.1. `wgpu-hal` Metal `draw_indexed_indirect` loops `for _ in 0..draw_count`, encoding one `drawIndexedPrimitives…indirectBuffer…` per draw. `draw_indexed_indirect_count` is an empty `//TODO` body, so GPU-side draw compaction is unavailable on Metal.

**Per-draw encode cost** (ms per frame, main thread, inside `CommandEncoder::finish` → `encode_render_pass`):

| Child | Hallway | campaign-test |
|---|---|---|
| `draw_indexed_indirect` (hal → driver) | 0.958 | 0.090 |
| `multi_draw_indirect` / `DrawContext::add` (incl. `DrawBatcher::add` indirect validation) | 0.843 | 0.075 |
| Sum | 1.80 | 0.165 |

Ratio ≈ 11×, tracking the draw-count ratio (10.9×). `encode_render_pass` total: 3.82 ms hallway, 2.01 ms campaign-test; the remainder (pass open/begin, bind groups, pipelines, validation-pass injection) is per pass, not per draw.

**`[CpuTiming]` medians (ms):**

| Map | `work` | `render` | `render_record` | `render_submit` |
|---|---|---|---|---|
| Hallway | 12.138 | 9.838 | 2.831 | 6.296 |
| campaign-test | 10.134 | 7.737 | 3.224 | 4.479 |

`render_submit` holds `CommandEncoder::finish` and queue submit. Leaf reduction cuts the per-draw share in proportion, in every pass that draws world geometry indirectly, including shadow fills.

**GPU timing.** This adapter lacks `TIMESTAMP_QUERY`, so `POSTRETRO_GPU_TIMING` reports unsupported. Per-pass GPU time on this Mac comes from an Instruments Metal System Trace (`rendering_pipeline.md` §12, "Without timestamp support").

## BVH builds and consumers

`bvh_build::build_bvh` returns `(Bvh, Vec<BvhPrimitive>, BvhSection)`: a live tree and primitive list the bakes traverse, and the flattened section that ships.

| Build | Call site | Live tree + primitives read by | Section read by |
|---|---|---|---|
| Pre-atlas | `pipeline.rs`, `StageId::BvhBuild` | SH (`ShBakeCtx`), delta plan (`DeltaBakePlanInputs`), delta SH, direct SH, animated direct SH, entity-shadow selection, billboard scatter, `ChunkLightListInputs` | `bake_cell_draw_index`; atlas preparation's cell partition when nothing is cut |
| Post-cut | `atlas_stage::rebuild_face_identity`, only when `plan_cut_charts` cut a face | Fused lightmap/shadowmask bake, `WeightMapInputs` | `bake_cell_draw_index`, `plan_cell_partition` → `canonical_cell_partition`, `build_placed_animated_light_chunks`, pack (with chunk ranges stamped) |

When nothing is cut, `pipeline.rs` keeps the pre-atlas products for the post-atlas stages.

## Cut sub-faces

`face_cut::apply_face_cuts` walks faces in order and emits each cut face's sub-faces at the parent's slot, cloning its `FaceMeta` (same `leaf_index`, same `texture_index`), with consecutive fresh index ranges. `remap_leaf_face_ranges` relies on the same slot rule. So a per-leaf texture sub-sort done in `extract_geometry` survives the cut, and a parent's sub-faces sit inside the parent's `(cell, bucket)` run in the rebuilt BVH.

A cut face's padded chart exceeded one pool layer (2,048 texels; ≈ 82 m at the 0.04 m default density), so each sub-face is far larger than any plausible extent bound and normally lands in its own leaf. Merging would still be correct: charts, chunks and blocks are per face, not per leaf.

## Cell partition coupling

`canonical_cell_partition` (level-format, also run by the loader's ClusterDirectory validation) counts BVH leaves with `index_count != 0` per cell and greedily packs cells into clusters under `primitive_limit` (`DEFAULT_PRIMITIVE_LIMIT = 64`, stored in section 49). Fewer leaves per cell means fewer, larger clusters. That changes 49 and 50, and through cluster-major block order, 22, 42, 25 and vertex `lightmap_block` ids in 17. `build_pipeline.md` §Cluster residency metadata already says the partition counts BVH leaves per cell.

## Bake-tree identity across the face reorder

Per-face primitives are sorted by `primitive_sort_key(texture, cell, index_offset)`. A stable per-leaf sort by texture keeps same-texture faces in `leaf.face_indices` order, so within each `(texture, cell)` group the face order is unchanged; only the `index_offset` values shift. The sorted primitive list holds the same faces with the same AABBs in the same order, so `Bvh::build` yields the same tree and traversal tests the same triangles in the same order. One limit: the key masks `index_offset` to 20 bits. On a map with more than 2²⁰ indices, a `(texture, cell)` group whose offsets straddle a 2²⁰ boundary may order differently once offsets shift.

SH-family bakes otherwise read geometry only as a vertex AABB (order-independent) and as triangles reached through primitives.
