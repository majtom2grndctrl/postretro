# BVH Leaf Clustering

> Re-anchored 2026-10-01 against `main` at 832e20c8a (after the oversize-face cut, #544). Evidence and derivation: `research.md`. Owner decisions recorded 2026-10-01 (§Decisions).

## Goal

Make each shipped BVH leaf one contiguous `(cell, material_bucket)` face run, not one `(face, material_bucket)` pair. Every world-geometry pass issues one indirect draw per leaf, and Metal encodes each draw separately. Fewer leaves shrink the node, indirect and candidate-leaf arrays, GPU cull work, and the per-draw CPU encode cost of shadow fills that miss the cache. Bake traversal, SH bytes and the cell partition stay unchanged. Lightmap charts may move within their cell blocks, and maps are re-baked.

## Scope

### In scope

- Split the geometry stage's coordinate-conversion and UV/tangent projection math into a submodule before extending the file.
- Sub-sort faces by texture index within each BSP leaf, so every `(cell, bucket)` group occupies one contiguous index-buffer range.
- Build the shipped `Bvh` section from clustered primitives, one per maximal `(cell, bucket)` run, in both BVH builds: pre-atlas and after the oversize-face cut. The live tree the bakes traverse stays one primitive per face.
- An AABB-extent bound on cluster size, plus a face-count escape hatch that restores one face per leaf.
- Rework the animated-light chunk builder's face lookup, which assumes one face per leaf.
- Make the cell partition count faces per cell, not BVH leaves, so it stays exactly today's.
- Verification that the submitted triangle set is unchanged. Measurement of leaf count, draw count, CPU encode and GPU cull, by means available on the owner's Mac, plus a GPU-timing handoff.
- Context-library updates for the new leaf granularity.

### Out of scope

- Changing the `BvhLeaf` / `BvhNode` on-disk layout. Strides stay 48 and 40, and clustering only widens `index_count`. The five layout sites need no edit: `level-format` `bvh.rs`, `render-data` `geometry.rs`, `renderer` `compute_cull.rs`, `bvh_cull.wgsl`, `candidate_cull.wgsl`.
- Clustering across cells or across material buckets. `cell_id` drives the visible-cell test and the bucket drives the draw call; both stay one per leaf.
- Changing the SAH build algorithm or replacing the `bvh` crate.
- Changing `CellDrawIndex` wire layout or version. Contract: §Cross-plan contracts.
- Clustering the bake-side tree (§Decisions).
- Changing the cell partition's result, its limits, or section 49's layout (§Decisions).
- Moving or rewriting geometry-stage tests. Task 1 moves source only.
- Drawing only visible cells' spans. That is `visible-span-draws`.
- Meshlet or cluster-cone culling, triangle-level GPU culling, any Nanite-style two-level scheme.

## Decisions

- **Owner (2026-10-01): cluster only the shipped BVH.** Clustering applies to the flattened `Bvh` section (id 19) and everything derived from it. The live tree and primitive list that bakes traverse stay one face per leaf.
- **Owner (2026-10-01): the cell partition counts faces, not BVH leaves.** `canonical_cell_partition` counts each cell's faces, which reproduces today's partition exactly, so sections 49 and 50 and the cluster-major block order of 22, 42 and 25 do not move because of clustering.
- **Owner (2026-10-01): allow lightmap chart moves.** Lightmap packing follows the new face order, and maps are re-baked. The proof for ids 22, 42 and 25 is per-face texel equivalence, not byte identity. The owner's reason: our maps are test fixtures and baked maps are expendable, and better streaming performance is a compelling enough possibility (§Lightmap impact, locality hypothesis).
- **Landing order.** This plan lands after `bake-parallelism-large-maps`, and last of the four perf briefs: upload batching → release validation → `visible-span-draws` → clustering.

## Direction

**Problem.** A leaf is one face, so the world draw count is 2·L + L·(uncached shadow slots), and wgpu's Metal backend encodes each indirect draw separately on the CPU (`research.md` §CPU encode evidence).

**Motivation, in landing order.** By the time clustering lands, the three briefs before it have taken most of the per-draw CPU cost. Release validation removes indirect validation (about 1.29 ms on the hallway, 0.54 ms on campaign-test). Visible-span-draws removes the culled leaves' draws. What remains for clustering, from the reconciled 2026-10-01 profiles (`research.md` §Reconciled render_submit):
- at the measured warm-cache views, about 0.002 ms per frame on stress and 0.004 ms on campaign;
- about 0.16 ms per uncached shadow slot on stress (about 0.01 ms on campaign), since shadow fills cone-cull with every cell visible and draw every leaf;
- smaller leaf, node and indirect arrays, and less GPU cull work.

By the `(cell, material bucket)` pair count, the leaf reduction is at most about 38% on stress and 29% on campaign. It is lower where the extent bound splits runs.

**Prior commitments.**
- `rendering_pipeline.md` §5 sets one leaf per `(face, material_bucket)`, leaves bucket-sorted into contiguous indirect slot ranges, and one global BVH. This plan changes the granularity and keeps the rest. Task 9 updates §5.
- §7.1: the camera cull writes or zeros each leaf's permanent slot, and the candidate path gathers leaves from `CellDrawIndex` spans. Both keep working unchanged over fewer, larger leaves.
- `build_pipeline.md` §Compiler pipeline step 8 states the per-face organization. Step 10 says the oversize cut rebuilds BVH and CellDrawIndex. §Cluster residency metadata says the partition counts BVH leaves per cell. Task 6 switches it to faces, which is the same count today; Task 9 updates the doc.
- §Build Cache, Determinism invariant: byte-identical output for identical inputs. Ray-traced bake bytes stay unchanged because bakes keep the per-face tree. Lightmap-family bytes change once, on purpose (§Decisions), and stay deterministic.
- `bake-parallelism-large-maps` decides that output bytes do not change, and that lever 5 starts from today's per-face leaf set. It lands first, so its byte-identity baselines are taken before this plan changes any byte. Sequencing: §Cross-plan contracts.

**Alternatives rejected.**
- *GPU draw compaction* (`multi_draw_indexed_indirect_count`) — the Metal backend's `draw_indexed_indirect_count` is a stub, so it buys nothing where the cost was measured.
- *Draw only visible cells* — `visible-span-draws` removes culled leaves' draws but still pays one draw per visible leaf, and does nothing for shadow fills, whose cone cull treats every cell as visible. It stacks with clustering rather than replacing it.
- *Cluster the bake tree too* — it shifts SH bytes on ties and moves lever 5's baseline, for no encode gain. Bake traversal cost is that plan's lever.
- *Meshlets* — overkill for low-poly content, whose `(cell, bucket)` runs are a handful of faces.

## Lightmap impact

Verified in source; confidence high except where noted. The owner allows chart moves (§Decisions).

- **Clustering itself changes no lightmap byte.** It runs after the oversize-face cut has fixed charts, UVs, density and cut lines. Its only outputs are id 19, id 37 and per-leaf chunk ranges. With the face-count partition, a default build and a `--bvh-cluster-max-faces 1` build of the same map differ only in ids 19 and 37 (A10).
- **Merging cut sub-faces only groups draws.** `plan_cut_charts` decides cuts per face from its own chart grid and density. `apply_face_cuts` gives each sub-face its own chart window and vertices before any BVH build. A leaf holding several sub-faces draws their index ranges together; each sub-face keeps its own UVs, `lightmap_block` id and chart. Chunks stay per face (Task 5, A6).
- **Task 2's reorder can move chart placement.** Cell-block packing (`cell_blocks.rs`, `fill_pool_layer_block`) and layer packing sort charts by area, descending, and break ties by chart index, which is face order. Reordering faces within a leaf can therefore swap equal-area charts' placements. Then ids 22, 42 and 25 bytes, and the vertex lightmap UVs in 17, change. Cell-block order does not change (face-count partition), so a chart moves only within its own cell's blocks.
- **A moved chart keeps its texels.** Soft-visibility seeds key on the chart's world-space frame and chart-grid texel, never on atlas position. Dilation (`dilate_edges`) runs `CHART_PADDING_TEXELS` passes, too few to cross from one chart's padding into a neighbour's. So a face's pre-encode irradiance, direction and coverage texels are identical wherever it lands (A11). Medium confidence on encoded bytes: BC6H/BC5 encode 4×4 groups and the direction atlas reduces by `--direction-texel-scale`, both by atlas position. A moved chart's encoded texels can therefore differ within codec error (A13, M9).
- **Nothing at runtime reads leaf-to-face order to address lightmaps.** Static lightmap texels resolve per vertex (`lightmap_block` id plus block-local UV); residency is per cell block. Animated compose maps chunk → cell through leaf chunk ranges (`build_chunk_cell_ids`), and a clustered leaf still owns exactly one cell. The cull shaders declare `chunk_range_*` for layout only.
- **Locality: a hypothesis, not a promised outcome.** After Task 2, the charts of faces drawn together are adjacent in chart order, so packing may place them nearer each other in the atlas. That could improve texture-cache locality when lightmaps are sampled. Two limits, read from source. Packing orders charts by area first, so chart order only decides ties. And cell-block streaming loads whole cell blocks, which already hold all of a cell's charts, so a move within a block cannot change what streams. Measured, optionally, by M11.

## Acceptance criteria

### Automated (routine `cargo test`)

- [ ] **A1.** On compiled fixtures, each `(cell, material bucket)` face group occupies exactly one contiguous index-buffer range, both before and after an oversize-face cut. Faces sharing a leaf and a texture keep their source order.
- [ ] **A2.** No BVH leaf spans more than one cell or one material bucket. Each leaf's index range is exactly the union of whole consecutive faces.
- [ ] **A3.** A fixture has several faces per bucket in each cell, textures interleaved in source order, and every run inside the extent bound. Its default build emits one leaf per `(cell, bucket)` run. Its `--bvh-cluster-max-faces 1` build emits one leaf per non-empty face. A map whose cells each hold one face per bucket gets the same leaf count from both.
- [ ] **A4.** A run whose union AABB diagonal exceeds `--bvh-cluster-max-extent` splits at face boundaries. Each piece fits the bound unless it is a single face that alone exceeds it. Split points are identical across repeated builds.
- [ ] **A5.** On clustered fixtures:
  - `CellDrawIndex` passes the loader's cross-validation.
  - Each `(cell, bucket)` pair owns exactly one span.
  - With nothing cut, the identity rebuild reproduces the pre-atlas `Bvh` section and `CellDrawIndex`, at both the default and max-faces-1 settings.
- [ ] **A6.** On an animated-light fixture with multi-face leaves, the chunk records (id 24) match the max-faces-1 build. Each leaf's chunk range is the concatenation of its faces' ranges in that build.
- [ ] **A7.** For a fixture, the live tree and primitive list are identical across the max-faces-1, default and small-extent settings. A fixture SH bake is byte-identical across them.
- [ ] **A8.** Two builds of the same fixture produce byte-identical `Bvh` and `CellDrawIndex` sections.
- [ ] **A9.** After the geometry-stage split, every existing geometry test passes unchanged, and no module outside `geometry.rs` changes an import.
- [ ] **A10.** On a fixture with an oversize face cut into sub-faces that share a run with other faces, the default build and the max-faces-1 build emit byte-identical ids 17, 22, 24, 25, 42, 49 and 50. Only 19 and 37 differ.
- [ ] **A11.** On a lit fixture baked with uncompressed irradiance, permuting the faces within a BSP leaf moves at least one chart's placement. Every face's irradiance, direction and coverage texels, read through its own placement, stay identical.
- [ ] **A12.** On fixtures with several faces per cell, the face-count partition equals the leaf-count partition computed over a max-faces-1 section: same clusters, members and primitive counts. That also holds after an oversize-face cut.
- [ ] **A13.** On a lit fixture with selected entity-shadow lights, after the A11 permutation:
  - every face's raw (pre-BC5) shadowmask texels match exactly;
  - each build's decoded BC5 masks stay within the encoder's own error of their raw masks.

  The tolerance comes from `shadowmask_bc5_encode_error_on_fixture_bakes` (`shadowmask_bake/tests.rs`), which reports max and mean absolute error per fixture but gates nothing. This AC gates on that max for the same fixture, measured once before the change.

### Automated, on demand (`#[ignore]`; compiles stress maps)

- [ ] **B1.** On `stress-warren`, `stress-warren-crates` and `campaign-test`, at every checked-in probe camera, the union of submitted index ranges is identical between a max-faces-1 build and a default build.
- [ ] **B2.** At each of those probes, the submitted triangle count grows by no more than 10% over the max-faces-1 build.
- [ ] **B3.** Each of those maps loads without a `CellDrawIndex` or `ClusterDirectory` validation error.
- [ ] **B4.** For `stress-warren-hallway-inspection` and `campaign-test`, ids 49 and 50 are byte-identical between the pre-change commit and after. Build both sides in the same mode: cold `--release`, or warm with the same cache state.

### Manual (measured findings; recorded in the PR)

Mac conditions for M2–M4: cargo `--release` engine, render scale Auto. Record idle VRAM and GPU utilization before each run (`rendering_pipeline.md` §12, Machine-state confounders), and take before and after under matching state.
- [ ] **M1. Leaf and draw counts.** Leaf count L of the shipped section, before and after, from `prl-build --verbose` BVH stats, on `stress-warren`, `stress-warren-hallway-inspection` and `campaign-test`. `stress-warren` drops by at least 60%. That threshold is likely unachievable: the `(cell, bucket)` pair count caps the drop at about 38% on stress and 29% on campaign (Open question 2). Draws per frame (2·L + L·uncached shadow slots), indirect buffer bytes, and candidate-leaf count at the B1 probes drop in proportion to L.
- [ ] **M2. CPU frame.** `[CpuTiming]` medians of `render_submit` and `work`, untraced, on the hallway and campaign-test. After is no higher than before. Also record one window that includes uncached shadow world fills, counted from the 120-frame shadow cache log's skipped-world-pass counts. That window is where clustering's CPU saving shows (about 0.16 ms per uncached slot on stress).
- [ ] **M3. Encode profile.** A `sample` profile of the main thread on the hallway, before and after. Record per-frame ms under `encode_render_pass` and the hal per-draw child `draw_indexed_indirect`, and the after/before ratio of those children beside the after/before ratio of L.
- [ ] **M4. Cull on GPU.** A Metal System Trace of the `cull` pass on `stress-warren` and the hallway: two captures before, one after. The after median exceeds the before median by no more than the spread between the two before-captures.
- [ ] **M5. Bake bytes.** Cold `--release` builds of `campaign-test` at the pre-change commit and after give byte-identical ids 23, 27, 34, 35, 41, 45, 47 and 48.
- [ ] **M6. Animated lights.** Maps with animated lights look the same as the pre-change build, by eye, at the same poses. Not machine-verified.
- [ ] **M7. Sweep record.** Per map:
  - the distribution of faces per `(cell, bucket)` run;
  - the fraction of runs the shipped extent splits;
  - the map's index count, against `primitive_sort_key`'s 2²⁰ offset mask.
- [ ] **M8. Large surfaces.** After a re-bake, the hallway's large lit surfaces look the same as the pre-change build, by eye, at fixed poses: floors, long walls, and the faces the oversize cut split, with their cut seams. Do the same on any other map with cut faces. Not machine-verified.
- [ ] **M9. Lightmap equivalence on real maps.** On a map with cut oversize faces and on `campaign-test`, record which faces' placements moved. Confirm A11's per-face equivalence on each map's uncompressed bake, and A13's shadowmask tolerance on its encoded bake.
- [ ] **M10. GPU timing handoff (not a Mac gate).** On an adapter with `TIMESTAMP_QUERY`: over a `POSTRETRO_GPU_TIMING=1` window, `cull` on `stress-warren` does not regress, and summed pass time regresses by at most 2%.
- [ ] **M11. Locality (optional).** Test the locality hypothesis. Per `(cell, bucket)` run, record the bounding rect of its charts' placements against the sum of their areas, before and after. If the rect shrinks measurably, take a Metal System Trace of the `forward` pass on the hallway, before and after. Record the result whichever way it goes; it gates nothing.

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

The reorder can move lightmap charts, which the owner allows (§Decisions). Add the A11 and A13 tests. `build_pipeline.md` §Build Cache's bump rule names atlas packing, so bump these four epochs. Their keys would miss anyway, through the geometry hash and atlas-layout fingerprint; the bump makes the invalidation explicit:
- `lightmap_layer::LAYER_FORMAT_VERSION` (`"lightmap_layer"`);
- `lightmap_layer::LIGHTMAP_SECTION_VERSION` (`"lightmap_section"`);
- `shadowmask_bake::SHADOWMASK_ATLAS_STAGE_VERSION` (`"shadowmask_atlas"`);
- `animated_light_weight_maps::STAGE_VERSION` (`"animated_lm_weight_maps"`).

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

Expose `--bvh-cluster-max-faces` on `prl-build` (`Args` in `main.rs`). Plumb one config to both production builds, the pre-atlas build in `pipeline.rs` (`StageId::BvhBuild`) and `atlas_stage::rebuild_face_identity`, through `AtlasStageInputs` → `prepare_atlas_stage`. Every other `build_bvh` caller is a test, `fixture_pipeline.rs` included, and takes the default. Under `--verbose`, `pipeline.rs` logs `bvh_build::log_stats` for the pre-atlas section only. Also log it after a post-cut rebuild replaces the section, so M1 reads the shipped leaf count. Update the `BvhPrimitive` and `collect_primitives` doc comments. Extend `empty_cut_rebuilds_the_pre_atlas_face_identity_set_unchanged` to both face modes (A5). CellDrawIndex, animated chunks and pack read the clustered section through their existing parameters, with no edit. The partition counts faces (Task 6), so the section does not move it.

### Task 4: Cluster extent bound

A clustered leaf is one frustum unit: if any part is in frustum, all of it draws. Bound it by AABB extent, not face count. Low-poly runs are a handful of faces, so a face cap would rarely bind. The real hazard is a room's floor and ceiling sharing a texture: they merge into one leaf whose AABB spans the room's height.

Add `max_extent` (meters, AABB diagonal) to `BvhClusterConfig`, exposed as `prl-build --bvh-cluster-max-extent <meters>`. Split each run greedily in face order: start a primitive with the next face, and add each following face while the union AABB's diagonal stays within the bound. A face that alone exceeds the bound forms its own primitive. Cuts fall only at face boundaries, so each primitive keeps a contiguous index range (A4). The default comes from Task 8. Until then, use a placeholder constant named as provisional. The face-count flag exists to turn clustering off. Do not tune with it. Add the A8 repeat-build test with an extent that splits runs, and extend Task 3's A7 test with that extent.

### Task 5: Animated-light chunk face mapping

`build_animated_light_chunks` (`crates/level-compiler/src/animated_light_chunks.rs`) maps a leaf's `index_offset` to one face through `face_by_offset: HashMap<u32, u32>`. Its comment calls offsets unique under "one primitive per face, one face per leaf". A `debug_assert_eq!` requires each leaf's `index_count` to equal one face's. Under clustering, a release build would chunk only a leaf's first face, and a debug build would panic.

Replace the map with a range walk over `face_index_ranges`, which is ascending in `index_offset` (`extract_geometry` and `apply_face_cuts` emit sequentially). For each leaf:
- `partition_point` to the first face at or after `leaf.index_offset`;
- walk forward while `index_offset < leaf.index_offset + leaf.index_count`, skipping zero-count faces;
- debug-assert that the walked counts sum to `leaf.index_count`;
- run today's per-face chunk logic for each face in order, appending to the leaf's range.

Leaves are sorted `(bucket, cell, index_offset)`, so chunks come out in the same order as a one-face-per-leaf build (A6). The new walk is correct for one-face leaves too, which is why it lands before Task 3. Test it on a hand-built `BvhSection` with multi-face leaves. Introduce no `HashMap` iteration.

### Task 6: Cell partition counts faces

`canonical_cell_partition` (`crates/level-format/src/cluster_directory/canonical_partition.rs`) counts, per cell, the BVH leaves with `index_count != 0`. Make it count each cell's faces instead, read from `CellRecord.face_count` in the `CellsSection` it already takes. Today the two counts are equal: every leaf is one face, and every emitted face has at least three vertices. `face_extract` drops polygons under three vertices and `apply_face_cuts` drops clipped pieces under three. Exterior and solid cells carry `face_count` 0, which `encode_cells` asserts. The compiler's `plan_cell_partition` encodes Cells from the leaf records after `remap_leaf_face_ranges`, so post-cut counts match the post-cut leaves too.

Keep `primitive_limit` and the section-49 field names and layout. Drop the BVH parameter if nothing else reads it, and update every caller:
- `cluster_directory_bake.rs` (`default_cell_partition`, the test-limits path);
- `pipeline/cell_partition.rs`;
- the loader's `validate_cells` in `level-format` `cluster_directory.rs`;
- tests in `pack/cluster_sh_payloads.rs`, `cluster_directory.rs` and `postretro` `sh_streaming/sync_manifest_test_fixture.rs`.

Rewrite `cluster_directory_semantics_validate_partition_connectivity_bounds_and_bvh_counts` to count faces. Add the A12 equivalence test. It needs no epoch bump: by the warrant above, the partition's output does not change.

### Task 7: CellDrawIndex verification

`bake_cell_draw_index` (`crates/level-compiler/src/cell_draw_index_bake.rs`) breaks a cell's runs at bucket changes, and `flatten`'s sort makes each cell's leaves in one bucket contiguous. So every `(cell, bucket)` pair yields exactly one span, of one or more leaves. Its logic does not change.

Add assertions on clustered fixtures for:
- one span per `(cell, bucket)` pair;
- loader cross-validation passing (`validate_cell_draw_index` in `level-loader` `prl_loader.rs`) (A5).

Confirm that the `is_drawable` gate (`index_count > 0`, cell `!is_solid && face_count > 0`) admits the same geometry when a leaf aggregates faces. Add a fixture where a cut face sits in a multi-face run, and confirm the rebuilt section and CellDrawIndex pass the same checks. Add the A10 fixture: a cut face in a multi-face run, compared across both face modes.

### Task 8: Measurement and probe verification

Extend the `#[ignore]` harness in `crates/postretro/src/candidate_cull_probes.rs`. Make `compile_probe_map` take extra `prl-build` flags and a distinct output path. For each probe, compare the union of submitted index ranges and the submitted triangle count between a `--bvh-cluster-max-faces 1` build and a default build (B1, B2, B3). B4 compares a pre-change build against an after build, outside the harness. Sweep `--bvh-cluster-max-extent` over a range, recording at each value:
- leaf count, node count and indirect buffer bytes;
- candidate-leaf and submitted-triangle counts at the probes;
- the run-size distribution and the fraction of runs split.

Ship as default the largest swept extent that keeps every probe within B2. Then take M1–M9, and optionally M11, under the Mac conditions in Manual, and hand M10 to a timestamp-capable adapter. Stop condition: if the median run is one face on `stress-warren`, clustering rarely fires. Then record the numbers and stop without shipping a default.

### Task 9: Context-library updates

Update these to the shipped design:
- `rendering_pipeline.md` §5: a leaf is a contiguous `(cell, bucket)` face run under an extent bound.
- `build_pipeline.md` §Compiler pipeline step 8: clustered section, per-face bake tree, the two flags.
- Step 10 and §Cluster residency metadata: the partition counts faces per cell.
- The `CellDrawIndex (id 37)` paragraph: one span per `(cell, bucket)`, of one or more leaves.

Correct the stale mirror list in the `crates/level-format/src/bvh.rs` header comment, which names four `postretro/src/...` sites. The live sites are `crates/render-data/src/geometry.rs`, `crates/renderer/src/compute_cull.rs`, `crates/renderer/src/shaders/bvh_cull.wgsl` and `crates/renderer/src/shaders/candidate_cull.wgsl`.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split of the file Task 2 edits.
**Phase 2 (concurrent):** Task 2, Task 5, Task 6 — disjoint files: `geometry.rs`, `animated_light_chunks.rs`, and the partition code. Task 5 must precede Task 3, or animated maps chunk only one face per leaf. Task 6 must precede Task 3, or clustering moves the partition.
**Phase 3 (sequential):** Task 3 — thin slice: the clustered section flows through CellDrawIndex, chunks, pack and loader end to end. Consumes Task 2's run contiguity.
**Phase 4 (concurrent):** Task 4, Task 7 — Task 4 edits `bvh_build.rs` and `main.rs`; Task 7 adds tests elsewhere.
**Phase 5 (concurrent):** Task 8, Task 9 — Task 8 sweeps Task 4's bound; Task 9 edits docs only.

## Rough sketch

The compiler already sorts leaves `(material_bucket_id, cell_id, index_offset)` in `bvh_build::flatten` and emits faces in leaf order in `geometry::build_leaf_ordered_faces`. Only a per-leaf texture sub-sort is missing. With it, a cluster is exactly a maximal `(cell, bucket)` run, the same grouping a `CellDrawIndex` span already describes.

Leaf AABBs become unions, which loosens frustum rejection. At cell granularity this costs nothing, since portal visibility already gates whole cells. Within a visible cell, a bucket becomes all-or-nothing. Task 4's bound controls that, and Task 8's sweep sets it.

Two SAH builds per `build_bvh` call add uncached BVH-stage time. The clustered tree has far fewer primitives than the per-face one.

`primitive_sort_key` masks `index_offset` to 20 bits. On a map past 2²⁰ indices, Task 2's offset shift can reorder a `(texture, cell)` group that straddles a 2²⁰ boundary, which would change that map's bake tree. M5 runs on `campaign-test`. The hallway's index count is unrecorded; Task 8 records it (M7).

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
| Each `(cell, bucket)` owns one span | Task 2 + Task 3 + `flatten`'s sort | Any change to `flatten`'s leaf sort or to `bake_cell_draw_index` run breaking | A5 |
| The cell partition is independent of leaf granularity | Task 6 (counts `CellRecord.face_count`) | Any code that again counts BVH leaves per cell; a face emitted with fewer than three vertices | A10, A12, B4 |
| Clustering changes no lightmap byte | Task 3 runs after the cut; Task 6 keeps block order | Any post-cut stage that reads leaf boundaries to place or bake texels | A10 |
| A face's lightmap and shadowmask texels do not depend on its atlas placement | Pre-existing (chart-frame seeds; chart-local dilation) | Task 2 moves placements; any seed or dilation keyed on atlas position | A11, A13 |
| Build output is deterministic | Task 2 (stable sort), Task 3 (`sort_key`), Task 4 (greedy in face order) | Any hash-map iteration feeding order, notably in Task 5 | A8 |
| Geometry stage behavior survives the split | Task 1 (moves private pure functions only) | Task 2 edits the same file next | A9 |

## Cross-plan contracts

### `visible-span-draws` (parallel brief)

That brief consumes `CellDrawIndex` (id 37) spans at runtime to draw only visible cells. The two stack: it removes culled leaves' draws, and clustering shrinks every remaining draw count, shadow fills included. Clustering keeps this `CellDrawIndex` contract, verified in source:

- **Bucket-sorted slots.** `bvh_build::flatten` sorts leaves `(material_bucket_id, cell_id, index_offset)`. Each bucket owns one contiguous slot range (`derive_bucket_ranges`), and inside it each cell's leaves form one contiguous run.
- **One bucket-coherent, maximal span per `(cell, bucket)`.** `bake_cell_draw_index` breaks a cell's runs at bucket changes. `validate_cell_draw_index` rejects a span crossing a bucket, a non-maximal pair of adjacent same-cell, same-bucket spans, out-of-order or overlapping spans, and any drawable leaf left uncovered. Within a cell, spans ascend by `leaf_start`.
- **Wire layout and version unchanged.** `CELL_DRAW_INDEX_VERSION = 1`, CSR `cell_span_offset[cell_count + 1]` plus `Span { leaf_start, leaf_count }`.

Informational, not a contract: after Task 2, a span's leaves in slot order also tile one gap-free index-buffer range. `visible-span-draws` draws over leaf slots, not index ranges, so it does not need this. Clustering does not rely on it either; each leaf needs only its own contiguous range (A2). No AC pins it.

### `bake-parallelism-large-maps`

**Overlap.**
- Lever 5 (BVH traversal) starts from today's per-face leaf set. This plan keeps the bake tree per-face (A7), so that baseline holds.
- Lever 3 (stage overlap) reasons that AnimLightChunks reads atlas placement and the BVH. Task 5 changes that stage's face lookup but not its inputs, so lever 3's dependency analysis still holds. Both plans edit `pipeline.rs` around AnimLightChunks, a merge conflict only.

**Sequencing.** Bake-parallelism lands first (§Decisions), so its byte-identity baselines come from today's bytes and this plan leaves them untouched. This plan:
- re-takes the hallway SH Bake timing, as both drafts say. With the per-face bake tree it should not move;
- takes its before-builds (B4, M5, M9) on `main` after bake-parallelism lands;
- re-warms the cache before any warm timing. Task 2 changes the geometry buffers, so the first warm build after it misses every geometry-hashed cache entry once. On the hallway that re-bakes the whole SH family.

**Section bytes.**

| Ids | Change | Cause |
|---|---|---|
| 17 | Yes | Face and index order (Task 2); vertex lightmap UVs where a chart's placement moves |
| 19 | Yes | Clustered leaves and nodes; per-leaf chunk ranges |
| 37 | Yes | Spans re-indexed; still one per `(cell, bucket)` |
| 24 | Yes | `face_index` values renumbered by Task 2; chunk order unchanged (A6) |
| 49, 50 | No | Partition counts faces (Task 6; A12, B4) |
| 22, 42, 25 | Yes, where equal-area charts swap (owner-allowed) | Task 2 only: block order unchanged, but within a cell packing ties break on face order. Per-face texels equivalent (A11, A13). Clustering itself changes none (A10) |
| 36 | Possibly | Navmesh rasterizes geometry triangles in buffer order |
| 33 | Key misses once; bytes not expected to change | SDF key hashes the index buffer in order |
| 23, 27, 34, 35, 41, 45, 47, 48 | No | Bakes traverse the unchanged per-face tree (M5) |
| 38, 46, 51 | No | Per-leaf face ranges are unchanged; none reads the BVH |

**Cache epochs and keys.** Task 2 bumps the lightmap layer, lightmap section, ShadowmaskAtlas and animated weight-map epochs, because it changes atlas packing. Nothing else bumps, and the cluster flags enter no key:
- No bake reads the clustered section. Bakes traverse the per-face tree, which no cluster setting changes (A7).
- The partition no longer reads the section (Task 6), so cluster settings cannot reach block order, and through it the lightmap layer, section and ShadowmaskAtlas keys.
- `animated_lm_weight_maps` keys on the chunk section bytes, which match across cluster settings (A6, A10).
- The face-count partition reproduces today's output, so it needs no bump either.
- Task 2's reorder changes the geometry buffers. The other stages that hash them (`sh_group`, the delta SH stages, ChunkLightList, billboard scatter, `navmesh`, `sdf_atlas`) miss once and re-bake under unchanged epochs. Their output does not change, and their keys already cover the input change.

## Open questions

1. **Shipped `--bvh-cluster-max-extent` default.** A measurement, not a design gap. Task 8 fixes the selection rule and the stop condition.
2. **Owner: what should the leaf-reduction criterion be?** M1's "`stress-warren` drops by at least 60%" is likely unachievable. The `(cell, bucket)` pair count caps the drop at about 38% on stress and 29% on campaign-test. Proposed replacement, a structural gate:
   - the default build's leaf count equals the `(cell, bucket)` run count plus extent splits;
   - extent splits stay under 10% of runs;
   - the reduction is recorded per map, not gated.

   Alternatives: a lowered fixed threshold, about 30% on stress; or gate the value instead, as M2's uncached-shadow-slot saving.
