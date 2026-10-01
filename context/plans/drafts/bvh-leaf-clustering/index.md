# BVH Leaf Clustering

> Re-anchored 2026-10-01 against `main` at 832e20c8a (after the oversize-face cut, #544). Evidence and derivation: `research.md`. Two owner questions block promotion (Open questions 1 and 2).

## Goal

Make each shipped BVH leaf one contiguous `(cell, material_bucket)` face run, not one `(face, material_bucket)` pair. Every world-geometry pass issues one indirect draw per leaf, culled leaves included, and Metal encodes each draw separately. Per-frame CPU encode cost and the indirect, node and candidate-leaf arrays all shrink in proportion to the leaf count. Bake traversal and SH bytes stay unchanged.

## Scope

### In scope

- Split the geometry stage's coordinate-conversion and UV/tangent projection math into a submodule before extending the file.
- Sub-sort faces by texture index within each BSP leaf, so every `(cell, bucket)` group occupies one contiguous index-buffer range.
- Build the shipped `Bvh` section from clustered primitives, one per maximal `(cell, bucket)` run, in both BVH builds: pre-atlas and after the oversize-face cut. The live tree the bakes traverse stays one primitive per face.
- An AABB-extent bound on cluster size, plus a face-count escape hatch that restores one face per leaf.
- Rework the animated-light chunk builder's face lookup, which assumes one face per leaf.
- Verification that the submitted triangle set is unchanged. Measurement of leaf count, draw count, CPU encode and GPU cull, by means available on the owner's Mac, plus a GPU-timing handoff.
- Context-library updates for the new leaf granularity.

### Out of scope

- Changing the `BvhLeaf` / `BvhNode` on-disk layout. Strides stay 48 and 40, and clustering only widens `index_count`. The five layout sites need no edit: `level-format` `bvh.rs`, `render-data` `geometry.rs`, `renderer` `compute_cull.rs`, `bvh_cull.wgsl`, `candidate_cull.wgsl`.
- Clustering across cells or across material buckets. `cell_id` drives the visible-cell test and the bucket drives the draw call; both stay one per leaf.
- Changing the SAH build algorithm or replacing the `bvh` crate.
- Changing `CellDrawIndex` wire layout or version. Contract: §Cross-plan contracts.
- Clustering the bake-side tree. Pending owner confirmation (Open question 1).
- Changing how the cell partition counts primitives. Pending owner decision (Open question 2).
- Moving or rewriting geometry-stage tests. Task 1 moves source only.
- Drawing only visible cells' spans. That is `visible-span-draws`.
- Meshlet or cluster-cone culling, triangle-level GPU culling, any Nanite-style two-level scheme.

## Direction

**Problem.** A leaf is one face, so the world draw count is 2·L + L·(uncached shadow slots), and wgpu's Metal backend encodes and validates each indirect draw on the CPU. On the hallway stress map that costs 1.79 ms per frame, ~11× campaign-test's cost, matching the draw-count ratio (`research.md` §CPU encode evidence).

**Prior commitments.**
- `rendering_pipeline.md` §5 sets one leaf per `(face, material_bucket)`, leaves bucket-sorted into contiguous indirect slot ranges, and one global BVH. This plan changes the granularity and keeps the rest. Task 8 updates §5.
- §7.1: the camera cull writes or zeros each leaf's permanent slot, and the candidate path gathers leaves from `CellDrawIndex` spans. Both keep working unchanged over fewer, larger leaves.
- `build_pipeline.md` §Compiler pipeline step 8 states the per-face organization. Step 10 says the oversize cut rebuilds BVH and CellDrawIndex. §Cluster residency metadata says the partition counts BVH leaves per cell. Clustering changes what that count measures (Open question 2).
- §Build Cache, Determinism invariant: byte-identical output for identical inputs. Bake bytes stay unchanged because bakes keep the per-face tree.
- `bake-parallelism-large-maps` decides that output bytes do not change, and that lever 5 starts from today's per-face leaf set. Keeping the bake tree per-face honors both. Sequencing: §Cross-plan contracts.

**Alternatives rejected.**
- *GPU draw compaction* (`multi_draw_indexed_indirect_count`) — the Metal backend's `draw_indexed_indirect_count` is a stub, so it buys nothing where the cost was measured.
- *Draw only visible cells* — `visible-span-draws` removes culled leaves' draws but still pays one draw per visible leaf, and does nothing for shadow fills, whose cone cull treats every cell as visible. It stacks with clustering rather than replacing it.
- *Cluster the bake tree too* — it shifts SH bytes on ties and moves lever 5's baseline, for no encode gain. Bake traversal cost is that plan's lever.
- *Meshlets* — overkill for low-poly content, whose `(cell, bucket)` runs are a handful of faces.

## Acceptance criteria

### Automated (routine `cargo test`)

- [ ] **A1.** On compiled fixtures, each `(cell, material bucket)` face group occupies exactly one contiguous index-buffer range, both before and after an oversize-face cut. Faces sharing a leaf and a texture keep their source order.
- [ ] **A2.** No BVH leaf spans more than one cell or one material bucket. Each leaf's index range is exactly the union of whole consecutive faces.
- [ ] **A3.** A fixture has several faces per bucket in each cell, textures interleaved in source order, and every run inside the extent bound. Its default build emits one leaf per `(cell, bucket)` run. Its `--bvh-cluster-max-faces 1` build emits one leaf per non-empty face. A map whose cells each hold one face per bucket gets the same leaf count from both.
- [ ] **A4.** A run whose union AABB diagonal exceeds `--bvh-cluster-max-extent` splits at face boundaries. Each piece fits the bound unless it is a single face that alone exceeds it. Split points are identical across repeated builds.
- [ ] **A5.** On clustered fixtures:
  - `CellDrawIndex` passes the loader's cross-validation.
  - Each `(cell, bucket)` pair owns exactly one span.
  - Within each span, the leaves in slot order tile one gap-free index range.
  - With nothing cut, the identity rebuild reproduces the pre-atlas `Bvh` section and `CellDrawIndex`, at both the default and max-faces-1 settings.
- [ ] **A6.** On an animated-light fixture with multi-face leaves, the chunk records (id 24) match the max-faces-1 build. Each leaf's chunk range is the concatenation of its faces' ranges in that build.
- [ ] **A7.** For a fixture, the live tree and primitive list are identical across the max-faces-1, default and small-extent settings. A fixture SH bake is byte-identical across them.
- [ ] **A8.** Two builds of the same fixture produce byte-identical `Bvh` and `CellDrawIndex` sections.
- [ ] **A9.** After the geometry-stage split, every existing geometry test passes unchanged, and no module outside `geometry.rs` changes an import.

### Automated, on demand (`#[ignore]`; compiles stress maps)

- [ ] **B1.** On `stress-warren`, `stress-warren-crates` and `campaign-test`, at every checked-in probe camera, the union of submitted index ranges is identical between a max-faces-1 build and a default build.
- [ ] **B2.** At each of those probes, the submitted triangle count grows by no more than 10% over the max-faces-1 build.
- [ ] **B3.** Each of those maps loads without a `CellDrawIndex` or `ClusterDirectory` validation error.

### Manual (measured findings; recorded in the PR)

Mac conditions for M2–M4: cargo `--release` engine, render scale Auto. Record idle VRAM and GPU utilization before each run (`rendering_pipeline.md` §12, Machine-state confounders), and take before and after under matching state.
- [ ] **M1. Leaf and draw counts.** Leaf count L of the shipped section, before and after, from `prl-build --verbose` BVH stats, on `stress-warren`, `stress-warren-hallway-inspection` and `campaign-test`. `stress-warren` drops by at least 60%. Draws per frame (2·L + L·uncached shadow slots), indirect buffer bytes, and candidate-leaf count at the B1 probes drop in proportion to L.
- [ ] **M2. CPU frame.** `[CpuTiming]` medians of `render_submit` and `work`, untraced, on the hallway and campaign-test. After is no higher than before.
- [ ] **M3. Encode profile.** A `sample` profile of the main thread on the hallway, before and after. Record per-frame ms under `encode_render_pass` and its per-draw children (`draw_indexed_indirect`, `DrawContext::add`), and the after/before ratio of those children beside the after/before ratio of L.
- [ ] **M4. Cull on GPU.** A Metal System Trace of the `cull` pass on `stress-warren` and the hallway: two captures before, one after. The after median exceeds the before median by no more than the spread between the two before-captures.
- [ ] **M5. Bake bytes.** Cold `--release` builds of `campaign-test` at the pre-change commit and after give byte-identical ids 23, 27, 34, 35, 41, 45, 47 and 48.
- [ ] **M6. Animated lights.** Maps with animated lights look the same as the pre-change build, by eye, at the same poses. Not machine-verified.
- [ ] **M7. Sweep record.** Per map:
  - the distribution of faces per `(cell, bucket)` run;
  - the fraction of runs the shipped extent splits;
  - the ClusterDirectory cluster count before and after.
- [ ] **M8. GPU timing handoff (not a Mac gate).** On an adapter with `TIMESTAMP_QUERY`: over a `POSTRETRO_GPU_TIMING=1` window, `cull` on `stress-warren` does not regress, and summed pass time regresses by at most 2%.

## Tasks

### Task 1: Split the geometry stage's projection math

The split has not landed. `crates/level-compiler/src/geometry.rs` is 1,252 lines. About 509 are non-test source, which crosses `development_guide.md` §2.1's yellow flag, and about 742 are the inline test module. Move the coordinate-conversion and texture-projection block to a private submodule (`geometry/projection.rs`):
- `engine_to_quake`, `engine_normal_to_quake` and `quake_to_engine_dir`;
- `compute_tangent_basis`;
- `uv_axes_engine_space` and `standard_uv_axes`;
- `compute_texel_uv`, `standard_texel_uv` and `valve_texel_uv`.

That is about 210 lines of private pure functions over `Face` and `DVec3`, with no dependency on the BSP tree or the emission loop. These stay at their current paths, because sibling modules import them as `crate::geometry::{..}`:
- `extract_geometry` and `extract_kinematic_mover_geometry`;
- `build_leaf_ordered_faces` and `log_stats`;
- `FaceIndexRange` and `GeometryResult`.

`geometry.rs` imports the moved functions it calls. The tests reach the one moved function they call directly, `engine_to_quake`, through `use super::*`, so that import also resolves it. The file lands near 1,040 lines, about 300 of them source. Behavior-preserving: no signature outside the moved block changes and no test changes (A9).

### Task 2: Contiguous `(cell, bucket)` index ranges

`extract_geometry` emits faces in BSP-leaf order through `build_leaf_ordered_faces(tree, exterior_leaves)`. Within a leaf, faces follow `leaf.face_indices` and interleave textures. Give `build_leaf_ordered_faces` a `texture_indices: &[u32]` parameter: the per-source-face texture index `extract_geometry` already builds before the call. Stable-sort each leaf's faces by that index before pushing them. Stable matters. Faces sharing a texture keep their `leaf.face_indices` order, so the per-face bake primitives still sort to the same sequence with the same AABBs, and the bake tree stays identical (A7, M5). Face indices and index offsets shift. Every consumer keyed by face index (`face_index_ranges`, `FaceMeta`, lightmap charts) is built later in the same build, and every cached stage that reads face order hashes the geometry buffers, so it misses rather than serving a stale face index.

The oversize-face cut keeps the property. `face_cut::apply_face_cuts` emits a cut face's sub-faces at the parent's slot with the parent's `FaceMeta`, so they stay inside the parent's run. Two tests guard cross-leaf order, `faces_ordered_by_empty_leaf` and `leaf_face_ranges_are_contiguous`; both stay green. Extend the second with within-leaf bucket contiguity and same-texture order (A1).

### Task 3: Clustered section, per-face bake tree

In `crates/level-compiler/src/bvh_build.rs`, `build_bvh` keeps its return shape `(Bvh, Vec<BvhPrimitive>, BvhSection)` with a split meaning:
- The tree and primitive list are today's per-face build, unchanged: `collect_primitives`, `Bvh::build`, `validate_traverse_iterator_depth`. Every bake keeps traversing it. Add the A7 test.
- The section comes from a second SAH build over clustered primitives, then `flatten`, whose `(material_bucket_id, cell_id, index_offset)` leaf sort is unchanged.

A new `collect_cluster_primitives` emits one `BvhPrimitive` per maximal run of consecutive faces sharing `(leaf_index, texture_index)`, skipping faces with `index_count == 0`:
- `index_offset` is the run's first face offset;
- `index_count` is the run's total;
- the AABB is the union of the faces' AABBs;
- `sort_key` comes from the existing `primitive_sort_key`.

Add a `BvhClusterConfig` and `build_bvh_with(geo, &BvhClusterConfig)`. `build_bvh(geo)` calls it with `BvhClusterConfig::default()`, so the roughly 180 bake-test call sites stay unchanged. The face-count mode is `1` or unbounded (the default). `1` skips the second build and flattens the per-face tree: today's section over the reordered geometry, the comparison build for B1, B2, A3, A6 and A7.

Expose `--bvh-cluster-max-faces` on `prl-build` (`Args` in `main.rs`). Plumb one config to both production builds, the pre-atlas build in `pipeline.rs` (`StageId::BvhBuild`) and `atlas_stage::rebuild_face_identity`, through `AtlasStageInputs` → `prepare_atlas_stage`. Every other `build_bvh` caller is a test, `fixture_pipeline.rs` included, and takes the default. Under `--verbose`, `pipeline.rs` logs `bvh_build::log_stats` for the pre-atlas section only. Also log it after a post-cut rebuild replaces the section, so M1 reads the shipped leaf count. Update the `BvhPrimitive` and `collect_primitives` doc comments. Extend `empty_cut_rebuilds_the_pre_atlas_face_identity_set_unchanged` to both face modes (A5). The partition, CellDrawIndex, animated chunks and pack read the clustered section through their existing parameters, with no edit.

### Task 4: Cluster extent bound

A clustered leaf is one frustum unit: if any part is in frustum, all of it draws. Bound it by AABB extent, not face count. Low-poly runs are a handful of faces, so a face cap would rarely bind. The real hazard is a room's floor and ceiling sharing a texture: they merge into one leaf whose AABB spans the room's height.

Add `max_extent` (meters, AABB diagonal) to `BvhClusterConfig`, exposed as `prl-build --bvh-cluster-max-extent <meters>`. Split each run greedily in face order: start a primitive with the next face, and add each following face while the union AABB's diagonal stays within the bound. A face that alone exceeds the bound forms its own primitive. Cuts fall only at face boundaries, so each primitive keeps a contiguous index range (A4). The default comes from Task 7. Until then, use a placeholder constant named as provisional. The face-count flag exists to turn clustering off. Do not tune with it. Add the A8 repeat-build test with an extent that splits runs, and extend Task 3's A7 test with that extent.

### Task 5: Animated-light chunk face mapping

`build_animated_light_chunks` (`crates/level-compiler/src/animated_light_chunks.rs`) maps a leaf's `index_offset` to one face through `face_by_offset: HashMap<u32, u32>`. Its comment calls offsets unique under "one primitive per face, one face per leaf". A `debug_assert_eq!` requires each leaf's `index_count` to equal one face's. Under clustering, a release build would chunk only a leaf's first face, and a debug build would panic.

Replace the map with a range walk over `face_index_ranges`, which is ascending in `index_offset` (`extract_geometry` and `apply_face_cuts` emit sequentially). For each leaf:
- `partition_point` to the first face at or after `leaf.index_offset`;
- walk forward while `index_offset < leaf.index_offset + leaf.index_count`, skipping zero-count faces;
- debug-assert that the walked counts sum to `leaf.index_count`;
- run today's per-face chunk logic for each face in order, appending to the leaf's range.

Leaves are sorted `(bucket, cell, index_offset)`, so chunks come out in the same order as a one-face-per-leaf build (A6). The new walk is correct for one-face leaves too, which is why it lands before Task 3. Test it on a hand-built `BvhSection` with multi-face leaves. Introduce no `HashMap` iteration.

### Task 6: CellDrawIndex and partition verification

`bake_cell_draw_index` (`crates/level-compiler/src/cell_draw_index_bake.rs`) breaks a cell's runs at bucket changes, and `flatten`'s sort makes each cell's leaves in one bucket contiguous. So every `(cell, bucket)` pair yields exactly one span, of one or more leaves. Its logic does not change.

Add assertions on clustered fixtures for:
- one span per `(cell, bucket)` pair;
- leaves in slot order tiling one gap-free index range per span (the §Cross-plan contracts guarantee);
- loader cross-validation passing (`validate_cell_draw_index` in `level-loader` `prl_loader.rs`) (A5).

Confirm that the `is_drawable` gate (`index_count > 0`, cell `!is_solid && face_count > 0`) admits the same geometry when a leaf aggregates faces. Add a fixture where a cut face sits in a multi-face run, and confirm the rebuilt section and CellDrawIndex pass the same checks. Count partition clusters on the fixtures with `canonical_cell_partition` at both face modes and report the change. The partition code is not edited (Open question 2).

### Task 7: Measurement and probe verification

Extend the `#[ignore]` harness in `crates/postretro/src/candidate_cull_probes.rs`. Make `compile_probe_map` take extra `prl-build` flags and a distinct output path. For each probe, compare the union of submitted index ranges and the submitted triangle count between a `--bvh-cluster-max-faces 1` build and a default build (B1, B2, B3). Sweep `--bvh-cluster-max-extent` over a range, recording at each value:
- leaf count, node count and indirect buffer bytes;
- candidate-leaf and submitted-triangle counts at the probes;
- the run-size distribution and the fraction of runs split.

Ship as default the largest swept extent that keeps every probe within B2. Then take M1–M7 under the Mac conditions in Manual, and hand M8 to a timestamp-capable adapter. Stop condition: if the median run is one face on `stress-warren`, clustering rarely fires. Then record the numbers and stop without shipping a default.

### Task 8: Context-library updates

Update these to the shipped design:
- `rendering_pipeline.md` §5: a leaf is a contiguous `(cell, bucket)` face run under an extent bound.
- `build_pipeline.md` §Compiler pipeline step 8: clustered section, per-face bake tree, the two flags.
- Step 10 and §Cluster residency metadata: what the partition's leaf count now measures, per the Open question 2 outcome.
- The `CellDrawIndex (id 37)` paragraph: one span per `(cell, bucket)`; a span's leaves tile one index range.

Correct the stale mirror list in the `crates/level-format/src/bvh.rs` header comment, which names four `postretro/src/...` sites. The live sites are `crates/render-data/src/geometry.rs`, `crates/renderer/src/compute_cull.rs`, `crates/renderer/src/shaders/bvh_cull.wgsl` and `crates/renderer/src/shaders/candidate_cull.wgsl`.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split of the file Task 2 edits.
**Phase 2 (concurrent):** Task 2, Task 5 — Task 2 edits `geometry.rs` and Task 5 edits `animated_light_chunks.rs`. Task 5 must precede Task 3, or animated maps chunk only one face per leaf.
**Phase 3 (sequential):** Task 3 — thin slice: the clustered section flows through CellDrawIndex, partition, chunks, pack and loader end to end. Consumes Task 2's run contiguity.
**Phase 4 (concurrent):** Task 4, Task 6 — Task 4 edits `bvh_build.rs` and `main.rs`; Task 6 adds tests elsewhere.
**Phase 5 (concurrent):** Task 7, Task 8 — Task 7 sweeps Task 4's bound; Task 8 edits docs only.

## Rough sketch

The compiler already sorts leaves `(material_bucket_id, cell_id, index_offset)` in `bvh_build::flatten` and emits faces in leaf order in `geometry::build_leaf_ordered_faces`. Only a per-leaf texture sub-sort is missing. With it, a cluster is exactly a maximal `(cell, bucket)` run, the same grouping a `CellDrawIndex` span already describes.

Leaf AABBs become unions, which loosens frustum rejection. At cell granularity this costs nothing, since portal visibility already gates whole cells. Within a visible cell, a bucket becomes all-or-nothing. Task 4's bound controls that, and Task 7's sweep sets it.

Two SAH builds per `build_bvh` call add uncached BVH-stage time. The clustered tree has far fewer primitives than the per-face one.

`primitive_sort_key` masks `index_offset` to 20 bits. On a map past 2²⁰ indices, Task 2's offset shift can reorder a `(texture, cell)` group that straddles a 2²⁰ boundary, which would change that map's bake tree. M5 runs on `campaign-test`. The hallway's index count is unrecorded; Task 7 records it with M1.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A `(cell, bucket)` group's faces are consecutive in the index buffer | Task 2 (stable per-leaf texture sort) | Any later change to face emission order; `apply_face_cuts` (sub-faces take the parent's slot); Task 4 splits only at face boundaries | A1, A2 |
| A leaf never spans two cells or two buckets | Task 3 (run key `(leaf_index, texture_index)`) | Task 4 subdivides runs, never merges them | A2, A3 |
| Bakes traverse the per-face tree whatever the cluster setting | Task 3 (tree and primitives from the per-face build) | Any change that hands the clustered tree to a bake input (`ShBakeCtx`, `DeltaBakePlanInputs`, `ChunkLightListInputs`, `WeightMapInputs`, the fused lightmap stage); Task 2's face reorder (stable sort keeps primitive order) | A7, M5 |
| Pre-atlas and post-cut builds use one cluster config | Task 3 (config through `AtlasStageInputs`) | `rebuild_face_identity` call site | A5 |
| Leaf array index is the permanent indirect draw slot | Pre-existing | Every runtime buffer sized from leaf count is sized at load from the section | B3, M1 |
| Submitted triangle set is unchanged | Task 3 (a run's range is the sum of its faces' ranges) | Task 4 splitting; `is_drawable` now evaluated per cluster | B1 |
| Every leaf's animated-light chunk range covers all faces it owns | Task 5 (range walk) | Task 4 changes leaf boundaries | A6 |
| Each `(cell, bucket)` owns one span; a span's leaves tile one index range | Task 2 + Task 3 + `flatten`'s sort | Any change to `flatten`'s leaf sort or to `bake_cell_draw_index` run breaking | A5 |
| Build output is deterministic | Task 2 (stable sort), Task 3 (`sort_key`), Task 4 (greedy in face order) | Any hash-map iteration feeding order, notably in Task 5 | A8 |
| Geometry stage behavior survives the split | Task 1 (moves private pure functions only) | Task 2 edits the same file next | A9 |

## Cross-plan contracts

### `visible-span-draws` (parallel brief)

That brief consumes `CellDrawIndex` (id 37) spans at runtime to draw only visible cells. The two stack: it removes culled leaves' draws, and clustering shrinks every remaining draw count, shadow fills included. Clustering keeps this `CellDrawIndex` contract, verified in source:

- **Bucket-sorted slots.** `bvh_build::flatten` sorts leaves `(material_bucket_id, cell_id, index_offset)`. Each bucket owns one contiguous slot range (`derive_bucket_ranges`), and inside it each cell's leaves form one contiguous run.
- **One bucket-coherent, maximal span per `(cell, bucket)`.** `bake_cell_draw_index` breaks a cell's runs at bucket changes. `validate_cell_draw_index` rejects a span crossing a bucket, a non-maximal pair of adjacent same-cell, same-bucket spans, out-of-order or overlapping spans, and any drawable leaf left uncovered. Within a cell, spans ascend by `leaf_start`.
- **Wire layout and version unchanged.** `CELL_DRAW_INDEX_VERSION = 1`, CSR `cell_span_offset[cell_count + 1]` plus `Span { leaf_start, leaf_count }`.

Clustering adds one guarantee: a span's leaves, in slot order, tile one gap-free index-buffer range (A5). Today it does not hold, because a cell's same-bucket faces interleave with other textures in the index buffer. `visible-span-draws` may rely on it only once this plan has landed.

### `bake-parallelism-large-maps`

**Overlap.**
- Lever 5 (BVH traversal) starts from today's per-face leaf set. This plan keeps the bake tree per-face (A7), so that baseline holds.
- Lever 3 (stage overlap) reasons that AnimLightChunks reads atlas placement and the BVH. Task 5 changes that stage's face lookup but not its inputs, so lever 3's dependency analysis still holds. Both plans edit `pipeline.rs` around AnimLightChunks, a merge conflict only.

**Sequencing.** Whichever plan lands second:
- re-takes the hallway SH Bake timing, as both drafts already say; under this plan's design it should not move;
- re-takes its byte-identity baselines on `main` after the first lands, because this plan changes output bytes;
- does not straddle the merge with a warm pinned-conditions measurement without a re-warm. Task 2 changes the geometry buffers, so the first warm build after it misses every geometry-hashed cache entry once. On the hallway that re-bakes the whole SH family.

**Section bytes.**

| Ids | Change | Cause |
|---|---|---|
| 17 | Yes | Face and index order (Task 2); vertex `lightmap_block` ids follow block order |
| 19 | Yes | Clustered leaves and nodes; per-leaf chunk ranges |
| 37 | Yes | Spans re-indexed; still one per `(cell, bucket)` |
| 24 | Yes | `face_index` values renumbered by Task 2; chunk order unchanged (A6) |
| 49, 50 | Yes, unless Open question 2 keeps the partition | The partition counts leaves per cell |
| 22, 42, 25 | Likely | Cluster-major block order follows the partition; face renumbering can move packing ties |
| 36 | Possibly | Navmesh rasterizes geometry triangles in buffer order |
| 33 | Key misses once; bytes not expected to change | SDF key hashes the index buffer in order |
| 23, 27, 34, 35, 41, 45, 47, 48 | No | Bakes traverse the unchanged per-face tree (M5) |
| 38, 46, 51 | No | Per-leaf face ranges are unchanged; none reads the BVH |

**Cache epochs and keys.** None bumps under this design. The cluster flags change only uncached products: the section, CellDrawIndex, partition and chunks. Every cached stage downstream of them already keys on what moves:
- the lightmap layers, lightmap section memo and ShadowmaskAtlas memo key on atlas layout, including block order;
- `animated_lm_weight_maps` keys on the chunk section bytes and the layout fingerprint;
- geometry-hashed stages (`sh_group`, the delta SH stages, ChunkLightList, billboard scatter, `navmesh`, `sdf_atlas`) miss once on Task 2's reorder.

If the owner instead clusters the bake tree (Open question 1), fold the cluster settings into every ray-tracing stage's key and bump each one's epoch:
- `SH_GROUP_STAGE_VERSION` and `INDIRECT_DELTA_SH_STAGE_VERSION`;
- `DIRECT_SH_STAGE_VERSION`, `DIRECT_SH_DELTA_STAGE_VERSION` and `ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION`;
- `BILLBOARD_DIRECT_SCATTER_STAGE_VERSION` and `ANIMATED_BILLBOARD_DIRECT_SCATTER_STAGE_VERSION`;
- `CHUNK_LIGHT_LIST_STAGE_VERSION`, `LAYER_FORMAT_VERSION` and `SHADOWMASK_ATLAS_STAGE_VERSION`;
- `animated_light_weight_maps::STAGE_VERSION`.

Without that, a flag-only edit would serve entries traced through a different tree.

## Open questions

1. **Owner: keep the bake tree per-face?** Both builds return the live tree that bakes traverse, and the draft required choosing between costing a clustered bake tree and keeping a fine-grained one. This spec assumes per-face: SH bytes, lever 5's baseline and bake cost stay unchanged, for a second SAH build per call. The alternative changes Task 3, M5, the A7 identity row, the cache list above, and the bake-parallelism re-take, which becomes mandatory.
2. **Owner: what should the cell partition count?** `canonical_cell_partition` counts non-empty BVH leaves per cell against `primitive_limit` (64 by default, stored in id 49), and the loader re-derives it from id 19. Clustering cuts per-cell leaf counts, so clusters coarsen. That changes SH streaming and lightmap residency grain, and the bytes of 49, 50, 22, 42 and 25. The options:
   - accept the coarser clusters and judge them from M7;
   - re-derive `DEFAULT_PRIMITIVE_LIMIT` for clustered leaves;
   - count a quantity clustering preserves, such as triangles (`index_count / 3`), with a re-derived limit, which changes partitioning on every map.

   As written, the partition code is untouched (first option).
3. **Shipped `--bvh-cluster-max-extent` default.** A measurement, not a design gap. Task 7 fixes the selection rule and the stop condition.
