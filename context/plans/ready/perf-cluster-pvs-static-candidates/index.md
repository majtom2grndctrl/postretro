# Cluster PVS Static Candidates

## Goal

Reduce large-map static-world visibility cost by adding baked visibility clusters and compact static draw candidates. The renderer should stop treating the whole global BVH and all material bucket ranges as the camera path's default candidate set on maps such as `stress-warren-crates`.

## Scope

### In scope

- Add one optional PRL section for visibility clusters.
- Build conservative clusters from BSP empty leaves and the portal graph.
- Bake cluster-to-cluster reachability for static world rendering.
- Map BVH leaves to owning clusters and per-cluster compact material bucket slices.
- Load cluster data into `LevelWorld`.
- Use cluster visibility to bound camera static-world cull and compact draw candidates.
- Keep legacy PRLs and fallback visibility paths working.
- Add diagnostics and perf capture for `stress-warren-crates`.

### Out of scope

- Static lightmap or SH cache restructuring.
- Dynamic occluders and kinematic-brush visibility.
- Replacing portal traversal for fog and dynamic-light reachability.
- Shadow-cull candidate reduction. Shadow cull must keep its current cone-only, camera-PVS-independent occluder behavior.
- Cross-cell BVH leaf coalescing.
- Cluster-local BVH root/node payloads. The MVP uses flat candidate BVH leaf indices instead.
- Runtime authoring controls for cluster boundaries.
- Stable cluster IDs across map edits.

## Acceptance criteria

- [ ] `prl-build` emits a `VisibilityClusters` section for non-empty maps with portals and static geometry. The compile gate is non-empty portal data and non-empty BVH leaves.
- [ ] Legacy PRLs without `VisibilityClusters` load with `visibility_clusters == None` and select the legacy static-candidate mode. This is a CPU fixture test plus review gate; no GPU frame is required.
- [ ] Cluster visibility is conservative for static drawing: every drawable portal-visible cell (`!is_solid && face_count > 0`) maps to a visible or candidate cluster. Validation projects drawable runtime-visible cells to clusters on sampled/simple fixtures and requires that set to be a subset of the cluster candidate set; graph reachability may over-include.
- [ ] For checked-in camera fixtures in `content/dev/maps/perf-fixtures/cluster-pvs-static-candidates.json`, clustered CPU candidate coverage includes every BVH leaf that the legacy CPU submitted-leaf mirror would submit on `campaign-test`, `stress-warren`, `stress-warren-lit`, and `stress-warren-crates`. The ignored CPU coverage test compiles source `.map` files into `target/postretro-perf/prl/` on demand and checks in no new generated PRL fixtures. This replaces pixel or GPU visibility tests.
- [ ] On at least the checked `stress-warren-crates` fixture, clustered mode reports `visible_clusters < total_clusters` and `candidate_bvh_leaves < total_bvh_leaves`. A conservative whole-connected-map candidate set is not an acceptable implementation of this plan.
- [ ] Dynamic meshes, particles, fog, and dynamic-light reachability continue to use the existing visible-cell and fog-reachable contracts. This is a test plus review gate: mesh and particle collectors still consume `VisibleCells`; fog and dynamic-light isolation still consume `fog_reachable`.
- [ ] Clustered camera candidate building does not inspect all global BVH leaves when the visible cluster set is a strict subset of all clusters. A CPU unit test with a synthetic clustered world verifies the builder iterates only the unioned candidate leaf indices.
- [ ] Clustered camera cull still applies the existing exact visible-cell and frustum tests to each candidate leaf before writing an indirect command. The acceptance predicate is shared with the CPU submitted-leaf mirror.
- [ ] Clustered camera submission produces a compact CPU-side draw plan: draw-indirect command records only for surviving candidate leaves, and active-bucket records only for material buckets with at least one surviving command.
- [ ] Shadow cull remains independent of camera PVS. This is a CPU-visible config/review gate: shadow cull keeps the all-ones visible-cell buffer and global BVH buffers, not clustered camera candidates.
- [ ] Clustered depth pre-pass and opaque forward drawing are recorded from the same compact draw plan object: one indirect command list plus one active-bucket table.
- [ ] Dev diagnostics report total clusters, visible clusters, candidate BVH leaves, total BVH leaves, visible cells, and whether the frame used clustered or legacy static candidates.
- [ ] Before/after submitted world-leaf count uses one metric source: BVH leaves that pass the same camera visible-cell and frustum tests before an indirect command is written.
- [ ] Clustered path is disabled automatically when the section is malformed, absent, or references out-of-range cells/leaves.

## Tasks

### Task 1: Baseline Metrics And Safety Gates

Add a repeatable CPU-visible perf capture for `stress-warren-crates.prl` using 120 warmup frames and 240 capture frames. Add the checked-in fixture file `content/dev/maps/perf-fixtures/cluster-pvs-static-candidates.json` and a headless ignored test or dev harness entry in `crates/postretro/src/visibility_perf.rs` that consumes it. The harness compiles source `.map` files into `target/postretro-perf/prl/` on demand with `prl-build`, loads the generated PRL through `load_prl`, and calls the runtime visibility and static-candidate helpers directly. It never opens a window or creates a GPU device.

Fixture schema: each entry has `name`, `map_path`, optional `source_origin_quake_units`, required `runtime_position_meters`, `yaw_degrees`, `pitch_degrees`, `horizontal_fov_degrees`, `warmup_frames`, and `capture_frames`. Runtime helpers consume `runtime_position_meters`, `yaw_degrees`, `pitch_degrees`, and `horizontal_fov_degrees`. Only `source_origin_quake_units` is metadata for humans; Quake source coordinates convert to runtime engine meters as `(-Y, Z, -X) * 0.0254`. The first fixture uses `stress-warren-crates.map` `player_spawn` source origin `[-2560, -1920, 96]`, runtime position `[48.768, 2.4384, 65.024]`, yaw `45deg`, pitch `0deg`, horizontal FOV `100deg`, 120 warmup frames, and 240 capture frames.

The harness writes one JSON summary under `target/postretro-perf/cluster-pvs-static-candidates.json`. The root object contains `fixtures: [...]`, one entry per checked fixture. Each fixture summary carries the fixture name, map path, warmup frame count, capture frame count, capture-only `portal_traversal_total_us` and `portal_traversal_mean_us` excluding warmup, visible-cell count, visible-cluster count when present, candidate BVH leaf count, total BVH leaves, submitted world-leaf count, active-bucket count, and static-candidate mode (`legacy` or `clustered`). Submitted world-leaf count means the CPU mirror iterates the active candidate leaves and applies the same camera visible-cell predicate, leaf AABB frustum predicate, and final pre-command acceptance used by the active camera draw path; use that definition before and after clustered changes. Do not make GPU timing a test requirement. If `POSTRETRO_GPU_TIMING=1` is available during manual profiling, it may be recorded alongside the JSON summary, but acceptance does not depend on GPU timing infrastructure. Add `crates/postretro/src/static_world_cull.rs` with pure helper functions for the shared static BVH leaf acceptance predicate and submitted-leaf counting; the harness uses those helpers. Add the pure validation-helper API and test scaffold that will compare drawable runtime-visible cells projected to clusters against cluster candidate cells; Tasks 6-7 complete the meaningful clustered assertions once the section and runtime candidates exist.

### Task 2: Split PRL Pack And Load Seams

Split the specific pack/load seams later tasks need before adding behavior. In `crates/level-compiler/src/pack.rs`, add `append_visibility_sections(sections: &mut Vec<SectionBlob>, visibility: VisibilitySectionPayloads)` or the closest local equivalent; it appends optional visibility-related sections without growing the long `pack_and_write_portals` body. Task 5 will extend `VisibilitySectionPayloads` with `VisibilityClusters` bytes and pass it through this helper. In `crates/postretro/src/prl.rs`, add `decode_visibility_extensions(...) -> VisibilityExtensions`, where the placeholder `VisibilityExtensions` initially contains no behavior-changing fields. The helper accepts raw optional section data, decoded BSP leaves or `LevelWorld` leaf records, decoded `BvhTree`, and derived bucket ranges when precomputed, then returns validated optional data for `LevelWorld`; Task 6 will add the `visibility_clusters` field and validation. Keep behavior unchanged in this task.

### Task 3: Wire Format

Add `VisibilityClusters` to `postretro_level_format::SectionId` with id `37`. Implementation must add `SectionId::VisibilityClusters = 37` and map it in `SectionId::from_u32`. Add a `visibility_clusters` module with a section type, `to_bytes`, `from_bytes`, malformed-byte tests, and round-trip tests.

The task owns this exact byte contract because later task agents will not read the full plan. The section uses little-endian `u32`s and starts with a 28-byte header: `version`, `cluster_count`, `cell_count`, `cluster_word_count`, `cluster_cell_index_count`, `cluster_leaf_index_count`, and `cluster_bucket_range_count`. `version` starts at `1`. `cluster_word_count` equals `ceil(cluster_count / 32)` and is `0` when `cluster_count == 0`. Payload order is `cell_to_cluster` entries, cluster records, cluster PVS bitsets, flat member-cell indices, flat owned BVH leaf indices, then flat per-cluster bucket slices. Cluster records are 24 bytes: `cell_start`, `cell_count`, `leaf_start`, `leaf_count`, `bucket_range_start`, `bucket_range_count`. Bucket slices are 12 bytes: `material_bucket_id`, `leaf_start`, `leaf_count`. Empty lists encode as zero counts, not absent subrecords.

`cell_to_cluster` uses `u32::MAX` as the no-cluster sentinel. Cluster PVS bit `j` uses `word_index = j / 32` and `bit_mask = 1u32 << (j % 32)`; padding bits for indices `>= cluster_count` in the final word must be zero. Decode rejects unsupported version, mismatched word count, computed-length overflow, trailing bytes, and set PVS padding bits. The `leaf_start` / `leaf_count` fields in cluster records define the cluster's owned range in the flat cluster BVH leaf index array. The `leaf_start` / `leaf_count` fields in per-cluster bucket slices index that same flat cluster-owned leaf-index array, not the global BVH leaf array. For cluster C, slices lie within C's owned range, do not overlap, cover exactly that cluster leaf range, and are sorted by `material_bucket_id`. Visible cluster ranges are unioned at runtime to form camera candidates.

### Task 4: Compiler Cluster Bake

Add `crates/level-compiler/src/visibility_clusters.rs` with a compiler-side intermediate named `CompilerVisibilityClusters`. This task owns clustering only: `cell_to_cluster`, flat `member_cells`, per-cluster `cell_start` / `cell_count` ranges, and `cluster_pvs` words. Include empty `leaf_start` / `leaf_count` and `bucket_range_start` / `bucket_range_count` fields in the intermediate records so Task 5 can fill BVH leaf ownership and bucket slices without changing the handoff shape. Task 5 completes BVH leaf ownership, bucket slices, and final PRL section emission. The shared drawable-cell predicate is `!is_solid && face_count > 0`, evaluated against the packed/encoded BSP leaf records that runtime consumes after exterior culling.

The first clustering heuristic is portal-graph flood clustering with a tunable maximum member-leaf limit; implementation may add open-area heuristics later, but MVP correctness is conservative coverage. Production `cluster_pvs` must be fixture-independent and must use a nontrivial visibility limiter, such as portal-to-portal clipped PVS over portal polygons or a flood with conservative portal-window/frustum clipping. Raw connected-component reachability or an all-clusters-visible bitset is not sufficient on connected maps because it can mark every cluster visible from every cluster while satisfying only the local conservatism checks. Add a compiler unit fixture with connected multi-cluster rooms and at least one occluded branch; the fixture must prove at least one source cluster has `visible_cluster_count < total_clusters` while still including all drawable clusters visible through the clipped portal solution. Fixtures validate sampled runtime visible clusters are subsets of production `cluster_pvs`, and the `stress-warren-crates` fixture must show a strict candidate reduction (`visible_clusters < total_clusters`, `candidate_bvh_leaves < total_bvh_leaves`) once Tasks 5-7 consume the section. `cluster_pvs` must include the source cluster when the source owns drawable leaves. Solid leaves are never members. Exterior leaves may be omitted only when they are not drawable by that predicate. Non-solid, non-drawable interior leaves may use the sentinel and force legacy fallback when the camera is in one. Every drawable cell by that predicate must have a cluster. The bake must produce conservative `cell -> cluster` and `cluster -> visible clusters` mappings.

### Task 5: Static Candidate Mapping

Map each BVH leaf to the cluster that owns its `cell_id`, consuming `CompilerVisibilityClusters` from `crates/level-compiler/src/visibility_clusters.rs`. The expected input shape is `cell_to_cluster`, flat `member_cells`, per-cluster `cell_start` / `cell_count` ranges, `cluster_pvs` words, and cluster records with empty leaf and bucket ranges reserved for this task. A BVH leaf's owning cell comes from `BvhLeaf.cell_id`, derived from compiler `BvhPrimitive.cell_id`, which comes from the face's `FaceMeta.leaf_index`. Faces are emitted per BSP leaf; a BVH leaf must not span multiple cells. Build flat per-cluster owned BVH leaf index lists and per-material-bucket slices into those lists. Fill the reserved `leaf_start` / `leaf_count` and `bucket_range_start` / `bucket_range_count` fields. Do not build cluster-local BVH roots. Do not coalesce across cells. A drawable BVH leaf is a BVH leaf with `index_count > 0` whose owning packed/encoded cell satisfies `!is_solid && face_count > 0`.

Bucket slices index the flat cluster-owned BVH leaf-index array, not global BVH leaves. For each cluster, bucket slices must lie within that cluster's owned flat leaf range, cover that range exactly, not overlap, and be sorted by `material_bucket_id`. Within each bucket slice, global BVH leaf indices are sorted ascending so Task 8 can emit deterministic draw commands without re-sorting cluster-owned storage. The flat cluster leaf index array contains every and only drawable BVH leaf (`index_count > 0` and drawable owning cell); compiler validation rejects extras, duplicates, and omissions. Validate that every drawable BVH leaf belongs to exactly one cluster. This task starts `prl-build` emission of the final `VisibilityClusters` section through the Task 2 packing seam when portal data and BVH leaves are non-empty.

### Task 6: Runtime Load And Validation

Decode `VisibilityClusters` into `LevelWorld`. Add the optional field as `visibility_clusters: Option<_>`. Because `load_prl` builds `LevelWorld` at the end, validate the section against decoded intermediate BSP leaves and decoded `BvhTree`, then store the validated value on `LevelWorld`. Only `VisibilityClusters` decode/validation errors are caught and degraded to `None`; unrelated PRL errors still propagate and fail load. On `VisibilityClusters` error, `load_prl` logs once, clears `visibility_clusters`, and selects the legacy static-candidate mode.

Runtime validation repeats the format and semantic checks, not only ID bounds. Reject unsupported version, mismatched `cluster_word_count`, `cluster_word_count != 0` when `cluster_count == 0`, `cell_count != leaves.len()` for non-empty maps, out-of-bounds starts and counts, checked-add overflow for every `start + count`, set PVS padding bits, any `cell_to_cluster` id outside `0..cluster_count` except `u32::MAX`, flat member-cell OOB, duplicate flat member-cell ids, solid leaves in member-cell ranges, member cells listed under cluster C whose `cell_to_cluster[cell] != C`, non-sentinel `cell_to_cluster` entries that do not appear exactly once in that cluster's member-cell range, drawable `cell_to_cluster == u32::MAX` cells, flat BVH leaf OOB, per-cluster bucket slices outside the cluster's owned flat leaf range, bucket slices that overlap, leave gaps, or are not sorted by `material_bucket_id`, duplicate BVH leaf references, and missing drawable BVH leaves. Validate each material bucket id against `BvhTree::derive_bucket_ranges`; `postretro_level_format::geometry::NO_TEXTURE` is valid only when present in derived ranges and follows current draw-time fallback behavior, regardless of texture-name bounds. `BvhLeaf.material_bucket_id` is the bucket id. Renderer-side texture upload mismatch is not section validation; current draw-time texture fallback behavior remains unchanged.

### Task 7: Cluster-Aware Camera Cull

Extend per-frame visibility so the camera path derives visible clusters from the camera leaf and the cluster PVS. Add a non-breaking per-frame visibility status/fallback reason output, separate from `VisibleCells` and `fog_reachable`. If that status reports a camera leaf with no valid cluster, including a sentinel non-drawable interior leaf, or an existing fallback path such as solid-leaf camera, exterior-camera, or no-portals, renderer uses the legacy global BVH camera path for that frame or level. Keep `VisibleCells` and `fog_reachable` unchanged for existing consumers.

Install validated cluster data through the existing level-load handoff: `LevelWorld` -> `level_world_to_geometry` -> `LevelGeometry` -> renderer resources. Add a renderer-owned sibling to `ComputeCullPipeline` named `ClusteredStaticCandidateBuilder`, with CPU output `CameraStaticDrawPlan`. Each clustered frame gathers visible-cluster flat BVH leaf candidates, iterates candidate `BvhTree.leaves`, applies the shared static BVH leaf acceptance helper from `crates/postretro/src/static_world_cull.rs`, groups surviving leaves by `material_bucket_id`, builds compact draw-indirect commands plus an active-bucket table on the CPU, and uploads those buffers before the depth pre-pass. Task 7 must also wire the first renderable clustered camera path: when `CameraStaticDrawPlan.mode == clustered`, both the depth pre-pass and opaque forward pass draw from that compact indirect buffer and active-bucket table; when the mode is legacy, they keep using `ComputeCullPipeline` and `draw_indirect_buckets`. Task 7 migrates the Task 1 harness and CPU submitted-leaf mirror to the same helper in the same change. `CameraStaticDrawPlan` reports static-candidate mode, visible-cluster count, candidate BVH leaf count, submitted world-leaf count, and active-bucket count for diagnostics and the CPU harness. Do not dispatch the global-root `bvh_cull.wgsl::cull_main` camera path on clustered camera frames. Keep `ComputeCullPipeline` and `bvh_cull.wgsl` available for legacy camera fallback and global shadow cull compatibility.

### Task 8: Compact Draw Submission

Refine the clustered depth pre-pass and opaque forward submission added in Task 7 so both passes consume the same `CameraStaticDrawPlan` through a small world draw-submission abstraction with two variants: legacy global bucket ranges from `draw_indirect_buckets`, and clustered compact active buckets. Surviving candidates are emitted grouped by `material_bucket_id`. Active records are sorted by `material_bucket_id`; commands within each active record are sorted by global BVH leaf index. Use a compact active-bucket command table: one CPU-known record per material bucket with at least one surviving candidate, with `material_bucket_id`, `indirect_start`, and `indirect_count`. Each record indexes a contiguous range of draw-indirect commands in the compact camera indirect buffer. Because the table is CPU-built before render pass recording, render loops can iterate active records without readback or indirect-count feature work. Absent buckets have no active record and are skipped in both multi-draw and fallback-loop paths; fallback loops iterate active records only. Preserve the legacy `draw_indirect_buckets` path for legacy PRLs and fallbacks. Shadow passes continue using global shadow buffers. The existing BVH cull-status overlay is legacy-GPU-cull-only unless this task also feeds it explicit CPU clustered status; in clustered mode it must be disabled or labelled as unavailable rather than showing stale `ComputeCullPipeline` status.

### Task 9: Diagnostics And Tests

Add diagnostics for total clusters, visible clusters, candidate BVH leaves, total BVH leaves, visible cells, and clustered/legacy path selection. Surface them in the dev-tools Diagnostics panel Performance tab, with stable labels matching those names; logging may mirror the panel but is not sufficient by itself. Add or verify checked-in camera fixtures for `campaign-test`, `stress-warren`, `stress-warren-lit`, and `stress-warren-crates` in `content/dev/maps/perf-fixtures/cluster-pvs-static-candidates.json`, using the Task 1 schema. Add compiler tests for conservative clustering and emitted section correctness. Add `postretro_level_format` decode/round-trip tests for malformed byte sections. Add runtime load semantic-validation tests for PRLs with no `VisibilityClusters` section selecting legacy static-candidate mode, legacy fallback, out-of-range validation, duplicate BVH leaf references, missing drawable BVH leaves, raw cluster candidates as a superset of legacy/drawable portal-visible leaves, and deterministic candidate-set equivalence after the shared visible-cell, frustum, and final pre-command predicates on simple fixtures. Add a regression-style CPU perf fixture assertion that the checked `stress-warren-crates` clustered frame has `visible_clusters < total_clusters` and `candidate_bvh_leaves < total_bvh_leaves`. Do not require pixel rendering tests.

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
visible clusters -> flat BVH leaf candidates
portal traversal -> VisibleCells
flat candidates + VisibleCells + frustum -> compact camera indirect buffer
compact camera indirect buffer + active-bucket table -> depth pre-pass and forward material-bucket draws
```

`crates/postretro/src/visibility.rs` still owns `VisibleCells` and `fog_reachable`. Do not change the meaning of `VisibleCells::Culled(Vec<u32>)` or `VisibleCells::DrawAll`.

Clustered/legacy path selection uses a separate per-frame visibility status/fallback reason output. Do not encode fallback reason into `VisibleCells` or `fog_reachable`.

`crates/postretro/src/compute_cull.rs` currently owns `ComputeCullPipeline`, `draw_indirect_buckets`, the fixed 4096-cell mask, and the full-leaf indirect buffer. The clustered path adds a sibling camera draw-candidate owner that builds compact camera indirect commands on the CPU and uploads them before the depth pre-pass. It must keep `ComputeCullPipeline` compatible with legacy camera fallback and shadow cull's existing global buffers.

`crates/postretro/src/shaders/bvh_cull.wgsl` currently uses one `@workgroup_size(1, 1, 1)` invocation to walk the whole flat tree. The clustered camera path should not start from the global root or reuse this shader for camera cull. Keep the shader for legacy camera fallback and shadow cull.

`crates/level-compiler/src/bvh_build.rs` builds one `BvhPrimitive` per face and flattens a global SAH BVH. Keep that global BVH for bake-time ray tracing, legacy rendering, diagnostics, and shadow cull. Cluster candidates are an added camera-rendering surface, not a replacement for the global BVH.

`crates/level-compiler/src/pack.rs::pack_and_write_portals` is the current section funnel. Do not grow the function further; Task 2 creates a smaller helper before `VisibilityClusters` is added to packing.

`crates/postretro/src/prl.rs` owns `LevelWorld` and PRL decode. Do not add the whole cluster decoder inline; Task 2 creates a helper before `LevelWorld` gains the optional cluster field.

Cluster PVS must be conservative for static drawing. `cluster_pvs[C]` is a fixture-independent, portal-clipped or portal-window-bounded conservative superset over drawable clusters. It includes C itself when C owns drawable leaves. It may include extra clusters, but it must not be raw connected-component reachability or an all-clusters-visible bitset. Fixtures validate sampled runtime visible clusters are subsets of the production `cluster_pvs`.

`VisibleCells::Culled(Vec<u32>)` is the drawable-only visibility set. `fog_reachable` is wider non-solid reachability. Static candidate validation covers drawable portal-visible cells (`!is_solid && face_count > 0`). It may use raw `portal_vis::portal_traverse` plus drawable projection, or use the drawable projection directly. It must not use fog reachability as the draw set.

Same-cell BVH leaf coalescing is allowed only as a later optimization. This plan does not require it. The MVP deliberately uses a flat candidate-index buffer over cross-cell coalescing or cluster-local BVH roots.

Every drawable BVH leaf/cell must have a cluster for clustered rendering. Non-solid, non-drawable interior cells may use `u32::MAX`; if the camera is in one, disable clustered camera candidates for that level or frame and use the legacy global path.

If camera leaf has no valid cluster, or portal traversal uses a fallback path such as solid-leaf camera, exterior-camera, or no-portals, renderer uses the legacy global BVH camera path for that frame or level.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Visibility clusters section | `SectionId::VisibilityClusters` | PRL section id `37` | n/a | n/a | n/a |
| Cluster id | `u32` | little-endian `u32` | n/a | n/a | n/a |
| Cell id | `u32` | little-endian `u32`, runtime BSP leaf index | n/a | n/a | n/a |
| BVH leaf index | `u32` | little-endian `u32`, index into `BvhSection::leaves` | n/a | n/a | n/a |
| Material bucket id | `u32` | little-endian `u32`, same value as BVH leaf `material_bucket_id` | n/a | n/a | n/a |

## Wire Format

`VisibilityClusters` uses little-endian integers. It is optional. Absence means legacy global camera cull.

All cell IDs and `cell_to_cluster` entries use the runtime BSP leaf index space: `LevelWorld.leaves` / `BspLeavesSection` order. For non-empty maps, `cell_count` must equal the BSP leaf count. Empty maps use `cell_count == 0`. The shared drawable-cell predicate is `!is_solid && face_count > 0`. Solid leaves and non-drawable leaves may use `u32::MAX`; every drawable cell by that predicate must have a cluster, and a camera in any sentinel non-drawable interior cell uses legacy fallback. Exterior cells may be omitted only when they are not drawable by that predicate.

Header:

| Field | Type | Meaning |
|---|---|---|
| `version` | `u32` | Section-internal version. Start at `1`. |
| `cluster_count` | `u32` | Number of clusters. |
| `cell_count` | `u32` | Number of `cell_to_cluster` entries. Must match BSP leaf count or be `0` for empty maps. |
| `cluster_word_count` | `u32` | Words per cluster PVS bitset. Equals `ceil(cluster_count / 32)`. |
| `cluster_cell_index_count` | `u32` | Flat member-cell index count. |
| `cluster_leaf_index_count` | `u32` | Flat owned BVH leaf index count. |
| `cluster_bucket_range_count` | `u32` | Flat per-cluster material bucket slice count. |

Header size: 28 bytes.

Payload order:

1. `cell_to_cluster`: `u32 * cell_count`. Sentinel `u32::MAX` means no cluster. Solid leaves may use the sentinel.
2. Cluster records, `cluster_count` entries, 24 bytes each:
   - `cell_start: u32`
   - `cell_count: u32`
   - `leaf_start: u32`
   - `leaf_count: u32`
   - `bucket_range_start: u32`
   - `bucket_range_count: u32`
3. Cluster PVS bitsets: `u32 * cluster_count * cluster_word_count`. Bit `j` means cluster `j` is a render candidate. `word_index = j / 32`, `bit_mask = 1u32 << (j % 32)`. Each word is little-endian.
4. Flat member-cell indices: `u32 * cluster_cell_index_count`.
5. Flat owned BVH leaf indices: `u32 * cluster_leaf_index_count`.
6. Flat per-cluster bucket slices, `cluster_bucket_range_count` entries, 12 bytes each:
   - `material_bucket_id: u32`
   - `leaf_start: u32`
   - `leaf_count: u32`

The `leaf_start` / `leaf_count` fields in cluster records define the cluster's owned range in the flat cluster BVH leaf index array. The `leaf_start` / `leaf_count` fields in per-cluster bucket slices refer to that flat cluster BVH leaf index array, not the global BVH leaf array. For cluster C, each bucket slice lies within C's `[leaf_start, leaf_start + leaf_count)` owned range. Slices do not overlap. Slices cover exactly that cluster leaf range and are sorted by `material_bucket_id`. Leaf indices inside each bucket slice are sorted by global BVH leaf index. Runtime unions the owned ranges for visible clusters to form camera candidates. This lets one visible cluster issue compact material-local candidate slices even when global BVH leaves are bucket-sorted for the legacy path.

Expected section length:

```text
28
+ 4 * cell_count
+ 24 * cluster_count
+ 4 * cluster_count * cluster_word_count
+ 4 * cluster_cell_index_count
+ 4 * cluster_leaf_index_count
+ 12 * cluster_bucket_range_count
```

Decode rejects overflow while computing this length. Decode rejects trailing bytes.

PVS padding bits for cluster indices `>= cluster_count` in the final word must be zero. Decode rejects set padding bits.

Loader rejection rules:

- Reject unsupported `version`.
- Reject `cluster_word_count != ceil(cluster_count / 32)`.
- Reject `cluster_word_count != 0` when `cluster_count == 0`.
- Reject `cell_count != LevelWorld.leaves.len()` for non-empty maps.
- Reject out-of-bounds starts and counts.
- Reject overflow from checked addition for every `start + count` pair.
- Reject any `cell_to_cluster` cluster id outside `0..cluster_count`, except `u32::MAX`.
- Reject any flat member-cell id outside `LevelWorld.leaves`.
- Reject duplicate flat member-cell ids.
- Reject solid leaves in flat member-cell ranges.
- Reject any member cell listed under cluster C whose `cell_to_cluster[cell] != C`.
- Reject any non-sentinel `cell_to_cluster` entry that does not appear exactly once in that cluster's member-cell range.
- Reject drawable `cell_to_cluster == u32::MAX` cells matching `!is_solid && face_count > 0`.
- Reject any flat BVH leaf index outside `BvhTree.leaves`.
- Reject any per-cluster bucket slice that points outside its cluster's `[leaf_start, leaf_start + leaf_count)` range.
- Reject per-cluster bucket slices that overlap, leave gaps in the cluster leaf range, are not sorted by `material_bucket_id`, or contain leaf indices out of ascending global BVH leaf-index order within a slice.
- Reject any per-cluster bucket slice whose `material_bucket_id` is absent from `BvhTree::derive_bucket_ranges`; `NO_TEXTURE` is valid only when present in derived ranges and follows existing draw fallback regardless of texture-name bounds. Renderer-side texture upload mismatch is draw-time fallback behavior, not section validation.
- Reject duplicate BVH leaf references across the flat cluster leaf index array.
- Reject missing drawable BVH leaves from the flat cluster leaf index array.

Any `VisibilityClusters` loader rejection above disables clustered camera candidates for that level and selects legacy static-candidate mode.

## Decisions

- First clustered camera implementation uses compact leaf-index candidates directly. Cluster-local BVH roots are out of scope for this plan.
- Clustered camera draw submission is CPU-built for the MVP. The renderer uploads a compact indirect buffer and CPU-known active-bucket table before render pass recording. Do not add GPU readback or indirect-count feature work for this plan.
- Cluster PVS does not replace portal traversal. Portal traversal remains the exact runtime truth for visible cells, fog reachability, dynamic-light reachability, diagnostics, and future dynamic-geometry work.
- Cluster PVS is a conservative static-world candidate bound. The renderer still validates candidate leaves with current visible-cell and frustum tests before drawing.
