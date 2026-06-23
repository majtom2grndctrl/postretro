# Cluster PVS Static Candidates

## Goal

Reduce large-map static-world visibility cost by adding baked visibility clusters and cluster-local static draw candidates. The renderer should stop treating the whole global BVH and all material bucket ranges as the camera path's default candidate set on maps such as `stress-warren-crates`.

## Scope

### In scope

- Add one optional PRL section for visibility clusters.
- Build conservative clusters from BSP empty leaves and the portal graph.
- Bake cluster-to-cluster reachability for static world rendering.
- Map BVH leaves to owning clusters and per-cluster material bucket ranges.
- Load cluster data into `LevelWorld`.
- Use cluster visibility to bound camera static-world cull and draw candidates.
- Keep legacy PRLs and fallback visibility paths working.
- Add diagnostics and perf gates for `stress-warren-crates`.

### Out of scope

- Static lightmap or SH cache restructuring.
- Dynamic occluders and kinematic-brush visibility.
- Replacing portal traversal for fog and dynamic-light reachability.
- Shadow-cull candidate reduction. Shadow cull must keep its current conservative all-world occluder behavior.
- Cross-cell BVH leaf coalescing.
- Runtime authoring controls for cluster boundaries.
- Stable cluster IDs across map edits.

## Acceptance criteria

- [ ] `prl-build` emits a `VisibilityClusters` section for non-empty maps with portals and static geometry.
- [ ] Legacy PRLs without `VisibilityClusters` load and render through the current global BVH path.
- [ ] Cluster visibility is conservative: every cell returned by current portal traversal maps to a visible or candidate cluster.
- [ ] No camera-visible world geometry disappears on `campaign-test`, `stress-warren`, `stress-warren-lit`, or `stress-warren-crates`.
- [ ] Dynamic meshes, particles, fog, and dynamic-light reachability continue to use the existing visible-cell and fog-reachable contracts.
- [ ] Camera world cull on clustered maps avoids walking the entire global BVH when the visible cluster set is a strict subset of all clusters.
- [ ] Opaque world draw on clustered maps avoids issuing global all-leaf material bucket ranges when cluster-local ranges are available.
- [ ] Shadow cull still renders world occluders outside the camera PVS when a shadow slot cone reaches them.
- [ ] Dev diagnostics report total clusters, visible clusters, candidate BVH leaves, total BVH leaves, visible cells, and whether the frame used clustered or legacy static candidates.
- [ ] `POSTRETRO_GPU_TIMING=1` comparison on `stress-warren-crates.prl` records camera BVH cull, opaque forward, and shadow cull timings before and after the change.
- [ ] Clustered path is disabled automatically when the section is malformed, absent, or references out-of-range cells/leaves.

## Tasks

### Task 1: Baseline Metrics And Safety Gates

Add a repeatable perf capture for `stress-warren-crates.prl`. Record CPU portal traversal time, camera BVH cull GPU time, opaque forward GPU time, shadow cull GPU time if available, visible-cell count, total BVH leaves, and visible or submitted world-leaf count. Add a non-rendering validation helper that can compare current portal-visible cells against cluster candidate cells once the section exists.

### Task 2: Split PRL Pack And Load Seams

Split cluster-related extension points out of oversized files before adding behavior. Move new section assembly away from the long `pack_and_write_portals` body in `crates/level-compiler/src/pack.rs`, and move cluster decode / validation away from the main `load_prl` body in `crates/postretro/src/prl.rs`. Keep behavior unchanged in this task.

### Task 3: Wire Format

Add `VisibilityClusters` to `postretro_level_format::SectionId` with id `37`. Add a `visibility_clusters` module with a section type, `to_bytes`, `from_bytes`, and round-trip tests. The section stores cluster membership, cluster PVS, leaf ownership, and per-cluster bucket ranges. Empty lists encode as zero counts, not absent subrecords.

### Task 4: Compiler Cluster Bake

Add a compiler module that groups non-solid BSP leaves into clusters. The first heuristic is portal-graph flood clustering with limits on maximum member leaves and large-portal/open-area expansion. Solid leaves are never members. Exterior leaves may be omitted unless they own drawable geometry. The bake must produce conservative `cell -> cluster` and `cluster -> visible clusters` mappings.

### Task 5: Static Candidate Mapping

Map each BVH leaf to the cluster that owns its `cell_id`. Build per-cluster, per-material-bucket leaf ranges or leaf lists. Do not coalesce across cells. If the global BVH leaf order prevents compact ranges for a cluster, store flat leaf indices and let the renderer compact or dispatch from those indices. Validate that every drawable BVH leaf belongs to exactly one cluster.

### Task 6: Runtime Load And Validation

Decode `VisibilityClusters` into `LevelWorld`. Validate all cell IDs against `LevelWorld.leaves`, all BVH leaf indices against `BvhTree.leaves`, and all material bucket IDs against loaded texture buckets. Invalid clustered data logs a warning and disables the clustered path for that level.

### Task 7: Cluster-Aware Camera Cull

Extend per-frame visibility so the camera path derives visible clusters from the camera leaf and the cluster PVS. Keep `VisibleCells` and `fog_reachable` unchanged for existing consumers. Add a camera cull path that uses the visible cluster set to restrict static BVH traversal or indirect command generation to cluster-local candidates.

### Task 8: Cluster-Local Draw Submission

Change opaque world drawing so clustered maps issue material bucket draws only for cluster-local candidates. Preserve the legacy `draw_indirect_buckets` path for legacy PRLs and fallbacks. On adapters without efficient multi-draw indirect, the clustered path must reduce the fallback loop count as well.

### Task 9: Diagnostics And Tests

Add diagnostics for cluster counts, visible clusters, candidate leaves, and path selection. Add compiler tests for conservative clustering and malformed-section rejection. Add runtime tests for legacy fallback, out-of-range validation, and same-pixel intent on simple connected rooms.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2. Baseline metrics and split seams reduce risk before format work.

**Phase 2 (sequential):** Task 3. The format crate contract blocks compiler and runtime work.

**Phase 3 (concurrent):** Task 4 and Task 6. Compiler cluster generation and runtime decode can proceed against the Task 3 section contract.

**Phase 4 (sequential):** Task 5. Candidate mapping consumes compiler clusters and the BVH leaf ownership contract.

**Phase 5 (sequential):** Task 7, then Task 8. Draw submission depends on the camera cull path's candidate representation.

**Phase 6 (sequential):** Task 9. Diagnostics and tests consume the final path.

## Rough sketch

Current camera path:

```text
portal traversal -> VisibleCells -> GPU global BVH walk -> global bucket ranges
```

Target camera path for clustered PRLs:

```text
camera cell -> camera cluster
camera cluster PVS -> visible clusters
visible clusters -> cluster-local BVH/draw candidates
portal traversal -> VisibleCells remains available for exact cell tests
```

`crates/postretro/src/visibility.rs` still owns `VisibleCells` and `fog_reachable`. Do not change the meaning of `VisibleCells::Culled(Vec<u32>)` or `VisibleCells::DrawAll`.

`crates/postretro/src/compute_cull.rs` currently owns `ComputeCullPipeline`, `draw_indirect_buckets`, the fixed 4096-cell mask, and the full-leaf indirect buffer. The clustered path may extend this owner or add a sibling camera cull owner, but it must keep shadow cull compatible with the existing global buffers.

`crates/postretro/src/shaders/bvh_cull.wgsl` currently uses one `@workgroup_size(1, 1, 1)` invocation to walk the whole flat tree. The clustered path should avoid starting from the global root when cluster-local root ranges exist. If the first implementation instead compacts candidate leaves, the shader may become a leaf-list writer rather than a tree walker for clustered camera cull.

`crates/level-compiler/src/bvh_build.rs` builds one `BvhPrimitive` per face and flattens a global SAH BVH. Keep that global BVH for bake-time ray tracing, legacy rendering, diagnostics, and shadow cull. Cluster candidates are an added camera-rendering surface, not a replacement for the global BVH.

`crates/level-compiler/src/pack.rs::pack_and_write_portals` is the current section funnel. Do not grow the function further; Task 2 creates a smaller helper before `VisibilityClusters` is added to packing.

`crates/postretro/src/prl.rs` owns `LevelWorld` and PRL decode. Do not add the whole cluster decoder inline; Task 2 creates a helper before `LevelWorld` gains the optional cluster field.

Cluster PVS must be conservative. It may include extra clusters. It must not exclude a cluster that contains a cell current portal traversal can reach from the same camera cell.

Same-cell BVH leaf coalescing is allowed only as a later optimization. This plan does not require it. If a task discovers that global leaf order makes cluster-local ranges ineffective, prefer a flat candidate-index buffer over cross-cell coalescing.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Visibility clusters section | `SectionId::VisibilityClusters` | PRL section id `37` | n/a | n/a | n/a |
| Cluster id | `u32` | little-endian `u32` | n/a | n/a | n/a |
| Cell id | `u32` | little-endian `u32`, BSP leaf index | n/a | n/a | n/a |
| BVH leaf index | `u32` | little-endian `u32`, index into `BvhSection::leaves` | n/a | n/a | n/a |
| Material bucket id | `u32` | little-endian `u32`, same value as BVH leaf `material_bucket_id` | n/a | n/a | n/a |

## Wire Format

`VisibilityClusters` uses little-endian integers. It is optional. Absence means legacy global camera cull.

Header:

| Field | Type | Meaning |
|---|---|---|
| `version` | `u32` | Section-internal version. Start at `1`. |
| `cluster_count` | `u32` | Number of clusters. |
| `cell_count` | `u32` | Number of `cell_to_cluster` entries. Must match BSP leaf count or be `0` for empty maps. |
| `cluster_word_count` | `u32` | Words per cluster PVS bitset. Equals `ceil(cluster_count / 32)`. |
| `cluster_cell_index_count` | `u32` | Flat member-cell index count. |
| `cluster_leaf_index_count` | `u32` | Flat candidate BVH leaf index count. |
| `cluster_bucket_range_count` | `u32` | Flat cluster-local material bucket range count. |

Payload order:

1. `cell_to_cluster`: `u32 * cell_count`. Sentinel `u32::MAX` means no cluster. Solid leaves may use the sentinel.
2. Cluster records, `cluster_count` entries:
   - `cell_start: u32`
   - `cell_count: u32`
   - `leaf_start: u32`
   - `leaf_count: u32`
   - `bucket_range_start: u32`
   - `bucket_range_count: u32`
3. Cluster PVS bitsets: `u32 * cluster_count * cluster_word_count`. Bit `j` means cluster `j` is a render candidate.
4. Flat member-cell indices: `u32 * cluster_cell_index_count`.
5. Flat BVH leaf indices: `u32 * cluster_leaf_index_count`.
6. Flat cluster bucket ranges, `cluster_bucket_range_count` entries:
   - `material_bucket_id: u32`
   - `leaf_start: u32`
   - `leaf_count: u32`

The `leaf_start` / `leaf_count` fields in cluster bucket ranges refer to the flat cluster BVH leaf index array, not the global BVH leaf array. This lets one visible cluster issue compact material-local candidate slices even when global BVH leaves are bucket-sorted for the legacy path.

Loader rejection rules:

- Reject unsupported `version`.
- Reject `cluster_word_count != ceil(cluster_count / 32)` unless `cluster_count == 0`.
- Reject out-of-bounds starts and counts.
- Reject any `cell_to_cluster` cluster id outside `0..cluster_count`, except `u32::MAX`.
- Reject any flat member-cell id outside `LevelWorld.leaves`.
- Reject any flat BVH leaf index outside `BvhTree.leaves`.
- Reject any cluster bucket range that points outside the flat cluster leaf index array.

## Open questions

- Should the first camera implementation walk cluster-local BVH roots or compact leaf-index candidates directly? The spec allows either. The deciding factor is which path reduces both camera cull and opaque draw time with less shader and buffer churn.
- Should cluster PVS replace portal traversal for static world cells immediately? The MVP keeps portal traversal for exact cells and diagnostics, then uses clusters only to bound static candidates.
- What exact pass-time win gates promotion from draft to ready? The draft requires measurement, but a numeric threshold should be chosen after one baseline capture on the current machine.
