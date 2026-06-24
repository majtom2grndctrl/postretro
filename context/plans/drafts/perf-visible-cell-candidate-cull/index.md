# Visible-Cell Candidate Cull

## Goal

Make camera cull cost scale with *visible* geometry instead of *in-frustum*
geometry. Runtime portal traversal already produces the exact visible-cell set
cheaply; the compiler bakes a small per-cell draw index (each cell's owned BVH
leaf spans) so the runtime gathers only those cells' leaves as cull candidates
instead of walking the whole tree. An occluder then subtracts runtime work —
geometry behind a solid wall is never enumerated — while the per-leaf frustum
gate and the existing indirect-draw layout stay byte-for-byte identical.

## Context

The shipping camera cull (`crates/postretro/src/shaders/bvh_cull.wgsl`,
`cull_main`, `@compute @workgroup_size(1,1,1)`) walks the flat BVH in a single
invocation. A `stress-warren` wall-facing probe visited ~5033 nodes, tested 2386
leaves, rejected 2383 by the visible-cell bit, and submitted 3 leaves: cost
tracks in-frustum-but-occluded geometry, not what is drawn.

Three source facts make a narrower path cheap and correct:

- Leaves are stable-sorted by `(material_bucket_id, cell_id, index_offset)`
  (`crates/level-compiler/src/bvh_build.rs:283`), so each cell's leaves form one
  contiguous range *per bucket*. A cell that touches K buckets owns K disjoint
  spans of the global leaf array — never a single range.
- `BvhLeaf.cell_id` is the BSP leaf index (`bvh_build.rs:83`, `cell_id:
  face.leaf_index`), the same id space `VisibleCells::Culled(Vec<u32>)` carries
  (`crates/postretro/src/visibility.rs`). So a `Culled` entry directly indexes a
  per-cell draw record.
- Portal traversal — which produces `Culled` — has negligible measured cost on
  this map, so no baked visibility is needed; only a baked *draw-location* index.

**Why this design over its siblings, on principle.** `index.md` §2 holds that
"portal traversal is the sole visibility path." This plan honors that: it bakes
only where each cell's geometry lives, never a second visibility source.
`perf-cluster-pvs-static-candidates` and `perf-baked-visibility-region-masks`
both bake PVS *reachability* — a competing, conservative visibility path — and
the cluster plan's compact CPU draw plan fragments the material-bucket ranges
into per-`(cluster, bucket)` slices (its named failure mode). This plan keeps
the global per-leaf indirect slots, so the material-bucket draw path is
unchanged and never fragments. It is also not `perf-flat-leaf-cull`, which tests
every leaf each frame ("plow faster"); this tests only visible cells' leaves
("plow less"), matching the lean-pipeline northstar.

## Scope

### In scope

- One optional PRL section: a per-cell draw index (CSR of leaf spans).
- Compiler bake of that index from the existing sorted BVH leaf array, joined to
  BSP leaf records for the drawable predicate.
- Runtime load/validation into `LevelWorld`, with legacy fallback when absent.
- A camera cull path that clears the indirect and cull-status buffers, gathers
  candidate leaf indices from `VisibleCells::Culled`, and dispatches one GPU
  invocation per candidate leaf to frustum-test and write that leaf's existing
  indirect slot.
- Preserve the per-leaf indirect slot layout, `bucket_ranges`, and
  `draw_indirect_buckets` exactly — no draw-call fragmentation.
- Keep the tree-walk shader as the fallback path (`DrawAll`, camera-in-solid,
  no section) and for shadow cull.
- CPU mirror proving the candidate path submits the same leaves and indirect
  fields as the tree walk.

### Out of scope

- Baked PVS / cluster reachability of any kind. Visibility stays runtime portal
  traversal; this plan only indexes where each cell's geometry lives.
- Changing portal traversal, `VisibleCells`, or `fog_reachable` semantics.
- Compacting the indirect buffer or changing material-bucket draw batching.
- Shadow cone cull migration — shadows keep the tree walk.
- Hi-Z, occlusion queries, software raster, depth readback.
- BVH layout or SAH-split changes.
- Stateful occluders (doors, lowering walls). Not applicable: candidates come
  from live portal visibility, so revealed geometry appears with no rebake.
- GPU timing as an acceptance gate; it may be captured manually.

## Acceptance criteria

- [ ] `prl-build` emits a `CellDrawIndex` section for maps with non-empty BVH
      leaves. Maps with zero BVH leaves omit it. (Portals are always present per
      `build_pipeline.md`, so leaf count is the gate, not portal presence.)
- [ ] PRLs without the section load with the index absent and select the legacy
      tree-walk camera path. CPU fixture test; no GPU frame required.
- [ ] For `stress-warren`, `stress-warren-crates`, and `campaign-test` camera
      probes, the candidate set built from `VisibleCells::Culled` equals exactly
      the leaf set the tree walk would pass the visible-cell gate on — verified
      by a CPU mirror, not GPU buffer contents.
- [ ] Output identity: for representative frustum-visible, frustum-rejected,
      visible-cell-rejected, and `DrawAll` cases, the candidate path's submitted
      leaf set, submitted index counts, material-bucket spans, and per-leaf
      indirect command fields match the tree walk for the same camera inputs
      (CPU mirror oracle).
- [ ] Camera cull dispatch work is proportional to the candidate leaf count
      (sum of visible cells' leaves), not total leaf or node count. A CPU unit
      test on a synthetic world asserts the gather visits only visible cells'
      spans.
- [ ] Before the candidate pass, both `indirect_draws` and `cull_status` are
      cleared to zero, so a leaf dropped this frame cannot retain a previous
      visible command (`index_count = 0` default) or a stale overlay status
      (zero = not-submitted). A CPU mirror or regression proves it.
- [ ] The draw path is unchanged: identical `bucket_ranges`, identical
      `draw_indirect_buckets` per-bucket call count and ordering as today.
      Cleared slots remain in-range as zero-`index_count` draws; the plan adds
      no per-`(cell, bucket)` fragmentation and no new bucket records.
- [ ] `VisibleCells::DrawAll`, camera-in-solid / exterior / no-portals
      fallbacks, and a missing/invalid section all use the legacy tree-walk
      camera path for that frame.
- [ ] Shadow cull output and dynamic spot-shadow behavior are unchanged.
- [ ] Spatial-tab diagnostics report candidate leaf count, total leaf count,
      submitted leaves, and which camera path the frame used. CPU-derived; not a
      perf gate.
- [ ] `candidate_cull.wgsl` reuses the `BvhLeaf`, `DrawIndexedIndirect`, and
      `CullUniforms` definitions from `bvh_cull.wgsl` byte-for-byte and is
      covered by the existing naga struct-stride test
      (`wgsl_bvh_struct_strides_match_spec`) plus CPU-only parse + validate.
- [ ] `cargo test -p postretro-level-format`, `-p postretro-level-compiler`,
      `-p postretro`, and `cargo check -p postretro --features dev-tools` pass.
- [ ] No acceptance criterion requires a GPU or `POSTRETRO_GPU_TIMING=1`.
- [ ] No new `unsafe`.

> The post-implementation update to `context/lib/rendering_pipeline.md` is a
> deliberate non-AC deliverable, landed at promotion (see Task 6).

## Tasks

### Task 1: Split PRL pack/load seams

Both extension points exceed the split-before-extend threshold:
`crates/level-compiler/src/pack.rs` (1316 lines) and
`crates/postretro/src/prl.rs` (2620 lines). Behavior-preserving: add
`append_cell_draw_index(...)` in `pack.rs` so the section does not deepen
`pack_and_write_portals` (line 336), and a `decode_cell_draw_index(...)` helper
seam in `prl.rs` so the decoder is not inlined into `load_prl` (line 515). No
behavior change in this task.

### Task 2: Add the CellDrawIndex PRL section

Add `SectionId::CellDrawIndex = 37` and map it in `SectionId::from_u32`
(`crates/level-format/src/lib.rs`; current max is `NavMesh = 36`, id 37 is
free). Add a `cell_draw_index` module with the section type, `to_bytes`,
`from_bytes`, round-trip tests, and malformed-byte tests (bad version,
mismatched counts, length overflow, trailing bytes, out-of-range leaf ranges,
non-monotonic offsets). See Wire format.

### Task 3: Compiler bake of the draw index

After the global BVH is flattened (`build_bvh`, `main.rs:420`) and before
`pack_and_write_portals` (`main.rs:1025`), derive the CSR index from the
already-sorted leaf array. The drawable predicate joins each `BvhLeaf.cell_id`
into the compiler's BSP leaf records (`BspLeafRecord`, fields `is_solid: bool`,
`face_count: u32`); since `cell_id == BSP leaf index`, the join is a direct
index. A drawable leaf is `index_count > 0` whose cell satisfies
`!is_solid && face_count > 0`. For each `cell_id`, collect its contiguous
per-bucket leaf ranges into `spans`; fill `cell_span_offset` as a prefix sum
over `cell_count` (BSP leaf count). The bake takes the BSP leaf records as an
input alongside the flattened BVH. It runs **uncached**, like the BVH stage it
derives from — no cache key or stage version. Emit through the Task 1 packing
seam, gated on non-empty `bvh.leaves`. Compiler asserts: every drawable leaf
appears in exactly one span, spans are contiguous and within bounds,
solid/exterior cells have empty ranges.

### Task 4: Runtime load and validation

Decode `CellDrawIndex` into `LevelWorld` as `Option<_>` via the Task 1 seam.
Validate against the decoded `BvhTree` and the BSP leaf count, joining
`cell_id` into `LevelWorld.leaves` (`LeafData.is_solid`, `LeafData.face_count`)
for the drawable check: reject unsupported version, `cell_count != leaves.len()`,
non-monotonic `cell_span_offset`, `offset[0] != 0`,
`offset[cell_count] != span_count`, any span outside `[0, total_leaves)`
(checked-add), overlapping or duplicate leaf coverage, and any drawable leaf
missing from the index. Any rejection logs once, clears the index, and selects
legacy mode.

### Task 5: Candidate cull path

Add a renderer-owned camera cull strategy alongside `ComputeCullPipeline`. Each
candidate-eligible frame:

1. Clear the camera `indirect_draws` and `cull_status` buffers to zero via
   `encoder.clear_buffer` (the tree walk already clears `cull_status` at
   `compute_cull.rs:284`; the candidate path must also clear `indirect_draws`
   because it no longer visits every slot).
2. On the CPU, expand candidate spans into a flat `candidate_leaves: Vec<u32>`
   of global BVH leaf indices by indexing the CSR with each id in
   `VisibleCells::Culled`. Upload into a storage buffer sized
   `total_leaves.max(1)` (worst case all leaves); pass the candidate count in a
   16-byte uniform (`candidate_count: u32` + padding).
3. Dispatch `candidate_cull.wgsl` with workgroup size 64 and
   `ceil(candidate_count / 64)` workgroups. Each invocation:
   `if gid.x >= candidate_count { return }`; `leaf =
   leaves[candidate_leaves[gid.x]]`; test the leaf AABB against the frustum; on
   pass write all five `DrawIndexedIndirect` fields into that leaf's existing
   global slot and set its `cull_status` to rendered, else leave both cleared.

The shader reads BVH leaves, frustum planes (`CullUniforms`), and the candidate
buffer only; it does not read BVH nodes or `skip_index`. Bindings:
`0 = CullUniforms`, `1 = leaves`, `2 = indirect_draws (read_write)`,
`3 = cull_status (read_write)`, `4 = candidate_leaves (read)`,
`5 = FlatCullParams uniform`. Depth pre-pass and forward pass draw from the same
`indirect_draws` buffer and `bucket_ranges` as today. `DrawAll`, fallback
visibility, and a missing index route to the existing tree-walk dispatch
unchanged. Shadow cull keeps the tree walk.

### Task 6: CPU equivalence and diagnostics

Add a CPU mirror helper returning submitted leaf indices, bucket spans
(`(material_bucket_id, first_leaf, leaf_count)`), and per-leaf indirect commands
for both paths, and assert equality on synthetic worlds and deterministic
stress-map probes. Add the Spatial-tab diagnostics named in the AC. Update
`context/lib/rendering_pipeline.md` to describe the candidate cull path at
promotion (not during drafting).

## Sequencing

**Phase 1 (sequential):** Task 1 — split-before-extend for the pack/load seams.
**Phase 2 (sequential):** Task 2 — establishes the wire contract.
**Phase 3 (concurrent):** Task 3, Task 4 — bake and decode against the Task 2 contract.
**Phase 4 (sequential):** Task 5 — consumes the loaded index and the candidate gather.
**Phase 5 (sequential):** Task 6 — verifies equivalence and reports diagnostics.

## Rough sketch

Camera path (candidate-eligible frame):

```text
portal traversal -> VisibleCells::Culled(cells)   (unchanged, authoritative)
-> candidate_leaves = concat over c in cells of spans[ offset[c] .. offset[c+1] ]   (CSR expand, CPU)
-> clear indirect_draws and cull_status
-> candidate_cull dispatch (one invocation per candidate leaf, wg 64):
     i = gid.x; if i >= candidate_count: return
     leaf = leaves[candidate_leaves[i]]
     if leaf AABB outside frustum: leave slot cleared, status stays 0
     else write all 5 DrawIndexedIndirect fields, status = rendered
-> draw_indirect_buckets per material bucket   (unchanged; cleared slots are zero-index_count no-ops)
```

Fallback path (`DrawAll`, solid/exterior/no-portals, missing/invalid section):

```text
-> existing ComputeCullPipeline tree walk over the global BVH
```

Output identity holds because the spans of the visible cells are exactly the
leaves whose `cell_id` is visible, so `(in-frustum) ∧ (cell visible)` — the tree
walk's gate — equals `(candidate) ∧ (in-frustum)`.

Likely touch points: `crates/level-format/src/{lib.rs,cell_draw_index.rs}`,
`crates/level-compiler/src/{bvh_build.rs,pack.rs,main.rs}`,
`crates/postretro/src/{prl.rs,compute_cull.rs,visibility.rs}`, new
`crates/postretro/src/shaders/candidate_cull.wgsl`, Spatial diagnostics.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Cell draw index section | `SectionId::CellDrawIndex` | section id `37` | n/a | n/a | n/a |
| Cell id | `u32` | little-endian `u32`, runtime BSP leaf index | n/a | n/a | n/a |
| Leaf span | `(leaf_start, leaf_count)` | two little-endian `u32` | n/a | n/a | n/a |

> Section id `37` is also claimed by the alternative drafts
> `perf-cluster-pvs-static-candidates` and `perf-baked-visibility-region-masks`.
> Only one of these mutually-exclusive approaches ships; the survivor takes `37`.

## Wire format

`CellDrawIndex` is optional; little-endian. Absence means legacy tree-walk
camera cull. Mirrors the CSR shape of an offset table plus a flat payload, like
existing index sections.

```text
u32 version = 1
u32 cell_count            // == BSP leaf count for non-empty maps
u32 span_count            // total leaf spans
u32 reserved = 0
u32 cell_span_offset[cell_count + 1]   // prefix sums; cell c owns spans[offset[c]..offset[c+1])
struct Span { u32 leaf_start; u32 leaf_count; } spans[span_count]
```

`cell_span_offset` is non-decreasing, `cell_span_offset[0] == 0`, and
`cell_span_offset[cell_count] == span_count`. Each span is a contiguous range of
the global `BvhSection::leaves` array lying within a single material bucket;
`leaf_start + leaf_count <= total_leaves` (checked-add, no overflow). Every
drawable leaf is covered by exactly one span; solid/exterior/non-drawable cells
have empty ranges (`offset[c] == offset[c+1]`). Decode rejects unsupported
version, count mismatches, non-monotonic offsets, overflow, and trailing bytes.

Expected length: `16 + 4*(cell_count + 1) + 8*span_count`.

## Decisions

Resolved during draft review by zooming out to `index.md` §2 (baked over
computed; portal traversal is the sole visibility path) and the lean-pipeline
northstar:

- **Candidate dispatch granularity → one invocation per candidate leaf, CPU
  expands spans.** The simpler shader (no GPU-side variable-length span
  expansion); candidate counts are small, so CPU expansion is cheap. Workgroup
  64, `ceil(candidate_count / 64)` dispatch. (Was an open question; Task 5
  commits to this.)
- **Cleared-slot draw cost → accepted.** `draw_indirect_buckets` still iterates
  full bucket ranges including cleared (`index_count = 0`) slots. The expensive
  plowing was the cull traversal, not near-free zero-`index_count` indirect
  commands, and keeping global slots is what avoids fragmentation. Diagnostics
  record candidate vs total leaves so a future optional compaction stays a
  measured decision, not a guess.
