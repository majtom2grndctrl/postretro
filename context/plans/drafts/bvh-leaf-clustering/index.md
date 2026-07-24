# BVH Leaf Clustering

## Goal

Raise BVH leaf granularity from one leaf per `(face, material_bucket)` pair to one leaf per contiguous `(cell, material_bucket)` face run. Shrinks node count, indirect draw buffer, and candidate-leaf lists proportionally to faces-per-cell-per-bucket, and raises per-draw triangle counts from ~2 triangles to a whole cell-bucket group.

## Scope

### In scope

- Sub-sort faces by texture index within each BSP leaf so every `(cell, bucket)` group occupies one contiguous index-buffer range.
- Cluster BVH primitives: one primitive per maximal `(cell, bucket)` run instead of one per face.
- A bounded cluster-size knob so a large cell's bucket does not become a single all-or-nothing frustum unit.
- Rework the animated-light chunk builder's face lookup, which currently assumes one face per leaf.
- Correctness verification that the submitted triangle set is unchanged, and measurement of the draw-count / submitted-triangle trade-off.

### Out of scope

- Changing `BvhLeaf` / `BvhNode` on-disk layout. Strides stay 48 and 40; clustering only widens `index_count`, so the five byte-identical mirror sites need no layout edit.
- Clustering across cells or across material buckets. `cell_id` drives the visible-cell test and the bucket drives the draw call; both stay one-per-leaf.
- Changing the BVH build algorithm or replacing the `bvh` crate.
- Changing `CellDrawIndex` wire layout. Span semantics are unchanged; spans simply collapse toward one per `(cell, bucket)`.
- Splitting `crates/level-compiler/src/geometry.rs` (1355 lines). See Open questions.
- Meshlet/cluster-cone culling, triangle-level GPU culling, or any Nanite-style two-level scheme.

## Acceptance criteria

- [ ] For every compiled map, each `(cell, material bucket)` face group occupies exactly one contiguous range of the index buffer.
- [ ] No BVH leaf spans more than one cell or more than one material bucket.
- [ ] On `stress-warren`, `stress-warren-crates`, and `campaign-test`, the set of triangles submitted by the camera path is identical before and after clustering, at every checked-in camera probe.
- [ ] Compiled leaf count on `stress-warren` drops by at least 60% versus the pre-change build.
- [ ] Indirect draw buffer size and per-frame candidate-leaf count drop proportionally to leaf count.
- [ ] Submitted triangle count at each camera probe grows by no more than 10% versus the pre-change build.
- [ ] `cull` GPU pass time on `stress-warren` does not regress; the summed GPU pass time does not regress by more than 2% over a `POSTRETRO_GPU_TIMING=1` window.
- [ ] Two builds of the same map from the same input produce byte-identical `Bvh` and `CellDrawIndex` sections.
- [ ] Every clustered map loads without a `CellDrawIndex` validation error, and each cell's spans still lie within a single material bucket.
- [ ] Maps with animated lights render identically to the pre-change build; every leaf's animated-light chunk range covers all faces the leaf owns.
- [ ] A map whose cells each hold one face per bucket produces the same leaf count as before.

## Tasks

### Task 1: Contiguous `(cell, bucket)` index ranges

Faces are emitted in BSP-leaf order by `build_leaf_ordered_faces` in `crates/level-compiler/src/geometry.rs`, so index ranges are already grouped per cell, but within a cell they follow `leaf.face_indices` order and interleave textures. Sub-sort each leaf's face list by the face's texture index (the value that becomes `FaceMeta::texture_index`, resolved through the `texture_indices` lookup already built in `extract_geometry`) before pushing to the ordered list, breaking ties by the existing face index so ordering stays deterministic. This makes every `(cell, bucket)` group exactly one contiguous index-buffer range. Face indices shift as a result: every downstream consumer keyed by face index — `face_index_ranges`, `FaceMeta`, lightmap charts — is built after this reorder in the same stage, so no cross-stage remap is needed, but confirm no stage caches a pre-reorder face index.

### Task 2: Cluster primitives in the BVH build

In `collect_primitives` (`crates/level-compiler/src/bvh_build.rs`), emit one `BvhPrimitive` per maximal run of consecutive faces sharing `(leaf_index, texture_index)` rather than one per face, skipping faces with `index_count == 0`. The primitive's `index_offset` is the run's first face offset, `index_count` the sum over the run, and the AABB the union of the run's face AABBs. Keep the existing `sort_key` scheme so builder input stays deterministic. Split a run into multiple primitives when it exceeds the cluster bound from Task 3. The existing leaf sort in `flatten` — stable by `(material_bucket_id, cell_id, index_offset)` — is unchanged and still yields contiguous per-bucket leaf ranges.

### Task 3: Cluster size bound

A whole-cell-bucket leaf is one frustum unit: if any part is in frustum, all of it draws. Add a bound that splits a run into multiple primitives when it would exceed a maximum face count per cluster, exposed as a `prl-build --bvh-cluster-max-faces <n>` flag mirroring the existing per-flag precedent, with a default chosen from the Task 6 measurements. A value of `1` reproduces pre-change one-face-per-leaf output exactly and is the regression escape hatch. Splitting must cut the run at a face boundary so each resulting primitive keeps a contiguous index range.

### Task 4: Animated-light chunk face mapping

`build_animated_light_chunks` in `crates/level-compiler/src/animated_light_chunks.rs` builds a `HashMap<u32, u32>` from leaf `index_offset` to face index, documented at the call site as relying on "one primitive per face, one face per leaf", with an explicit instruction not to reintroduce a linear scan. Replace it with a mapping from a leaf's `(index_offset, index_count)` range to the set of faces it covers. Because Task 1 guarantees a leaf's faces are consecutive, a sorted array of face start offsets plus a binary search for the range's lower bound and a forward walk to `index_offset + index_count` is sufficient — no interval tree needed. Every face in the leaf's range contributes its chart to that leaf's chunk build, so a clustered leaf's chunk range covers all its faces.

### Task 5: Verify CellDrawIndex and loader validation

`bake_cell_draw_index` in `crates/level-compiler/src/cell_draw_index_bake.rs` derives maximal contiguous per-cell runs broken at bucket changes. Clustering does not change its logic, but it changes the shape of its output: a cell touching K buckets now owns K spans of one leaf each in the common case. Confirm the loader's cross-validation of the section still passes, that the debug invariant assertions hold, and that the `is_drawable` gate (`index_count > 0`, cell `!is_solid && face_count > 0`) still admits exactly the same geometry now that a leaf aggregates faces.

### Task 6: Measurement and probe verification

Extend the existing probe harness in `crates/postretro/src/candidate_cull_probes.rs`, which already compiles `stress-warren`, `stress-warren-crates`, and `campaign-test` and compares the candidate path against the tree walk via `candidate_cull_mirror`. Add an assertion that the union of submitted index ranges — not the leaf set, which necessarily changes — is identical between a `--bvh-cluster-max-faces 1` build and a clustered build at each probe camera. Record leaf count, node count, indirect buffer bytes, candidate-leaf count, submitted triangle count, and `POSTRETRO_GPU_TIMING=1` pass times for both builds at several cluster bounds, and pick the shipped default from the results.

## Sequencing

**Phase 1 (sequential):** Task 1 — every later task depends on contiguous `(cell, bucket)` index ranges.
**Phase 2 (sequential):** Task 2 — establishes clustered primitives that Tasks 3–5 all consume.
**Phase 3 (concurrent):** Task 3, Task 4, Task 5 — independent consumers of the clustered leaf array, no shared files.
**Phase 4 (sequential):** Task 6 — consumes the cluster bound from Task 3 to sweep values.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A leaf's faces are consecutive in the index buffer | Task 1 (per-leaf texture sub-sort) | Any later change to face emission order in the geometry stage; Task 3's run splitting must cut at face boundaries | AC 1 |
| A leaf never spans two cells or two buckets | Task 2 (run key is `(leaf_index, texture_index)`) | Task 3's splitting only subdivides runs, never merges them | AC 2, AC 9 |
| Leaf array index is the permanent indirect draw slot | Pre-existing; unchanged by this plan | Task 2 changes leaf count, so every buffer sized from leaf count must be resized together | AC 5 |
| Submitted triangle set is unchanged | Task 2 (union of a run's ranges equals the sum of its faces' ranges) | Task 3 splitting; Task 5's `is_drawable` gate now evaluated per cluster rather than per face | AC 3, AC 6 |
| Every leaf's animated-light chunk range covers all faces it owns | Task 4 (range-based face lookup) | Task 3 splitting changes leaf boundaries after chunk ranges are reasoned about | AC 9 |
| Build output is deterministic | Task 1 (tie-break by face index), Task 2 (unchanged `sort_key`) | Any hash-map iteration order introduced in Task 4 | AC 8 |

## Rough sketch

The enabling observation is that the compiler already sorts leaves by `(material_bucket_id, cell_id, index_offset)` in `bvh_build::flatten`, and already emits faces in BSP-leaf order in `geometry::build_leaf_ordered_faces`. The only missing piece is a per-leaf sub-sort by texture. Once that lands, a cluster is exactly a maximal `(cell, bucket)` run, which is also exactly what a `CellDrawIndex` span describes — so the compiler is already computing the grouping, just one level too late to act on it.

Leaf AABBs become unions, which loosens frustum rejection. This costs nothing at cell granularity — portal visibility already gates whole cells — but within a visible cell a bucket becomes all-or-nothing. Task 3's bound is the control for that; the measurement in Task 6 decides where it sits.

The doc comment at the top of `crates/level-format/src/bvh.rs` names four downstream mirror sites under stale `postretro/src/...` paths. The live sites are `crates/render-data/src/geometry.rs`, `crates/renderer/src/compute_cull.rs`, `crates/renderer/src/shaders/bvh_cull.wgsl`, and `crates/renderer/src/shaders/candidate_cull.wgsl`. Correct the comment while working in this file.

## Open questions

- `crates/level-compiler/src/geometry.rs` is 1355 lines, past the ~800-line split-before-extend threshold. Task 1 is a few lines inside an existing function rather than new functionality, so this plan does not mandate a split — but the file is a standing candidate, and the UV/tangent projection math is the obvious seam.
- The right default for `--bvh-cluster-max-faces` is unknown until Task 6 measures. If submitted triangles regress badly at every bound above 1, the plan's premise is wrong and it should stop after Task 6 rather than ship a default.
- Clustering by `(cell, bucket)` ignores co-planarity. A cell's floor and ceiling with the same texture merge into one leaf with a tall AABB. A co-planarity or AABB-extent secondary split may beat a flat face-count bound; Task 6's sweep should record enough to tell.
