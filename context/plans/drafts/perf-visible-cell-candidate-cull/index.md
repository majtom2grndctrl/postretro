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
- `BvhPrimitive.cell_id` is assigned from the BSP leaf index
  (`bvh_build.rs:83`, `cell_id: face.leaf_index`), then copied into
  `BvhLeaf.cell_id` during flatten (`bvh_build.rs:210`). Runtime
  `geometry.rs` keeps that `BvhLeaf.cell_id` field. This is the same id space
  `VisibleCells::Culled(Vec<u32>)` carries (`crates/postretro/src/visibility.rs`).
  So a `Culled` entry directly indexes a per-cell draw record.
- Portal traversal has negligible measured cost on this map. When it succeeds,
  it produces `Culled`, so no baked visibility is needed; only a baked
  *draw-location* index.

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
  candidate leaf indices from portal-path `VisibleCells::Culled`, and dispatches
  one GPU invocation per candidate leaf to frustum-test and write that leaf's
  existing indirect slot.
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
- Stateful portal/occluder support. This plan follows whatever `VisibleCells`
  already reports for the frame.
- GPU timing as an acceptance gate; it may be captured manually.

## Acceptance criteria

- [ ] `prl-build` emits a `CellDrawIndex` section for maps with non-empty BVH
      leaves. Maps with zero BVH leaves omit it. Emission does not depend on
      portal presence. Candidate eligibility still requires
      `VisibleCells::Culled`, a valid loaded index, and portal visibility path
      provenance (`matches!(visibility.path, VisibilityPath::PrlPortal { .. })`).
- [ ] PRLs without the section load with the index absent and select the legacy
      tree-walk camera path. CPU fixture test; no GPU frame required.
- [ ] For `stress-warren`, `stress-warren-crates`, and `campaign-test` camera
      probes, the candidate set equals all drawable BVH leaves whose `cell_id`
      is in portal-path `VisibleCells::Culled`. Submitted output equals the
      tree-walk submitted leaves after the same frustum predicate. Verified by a
      CPU mirror, not GPU buffer contents. Heavy map-derived probe tests are
      `#[ignore]` / on-demand unless represented by compact checked-in CPU
      fixtures.
- [ ] Output identity: for representative frustum-visible, frustum-rejected,
      and visible-cell-rejected cases, the candidate path's submitted leaf set,
      submitted index counts, unchanged renderer `bucket_ranges`, and per-leaf indirect
      command fields match the tree walk for submitted leaves only (CPU mirror
      oracle). Rejected and non-candidate slots compare normalized CPU mirror
      semantics: `index_count == 0` and `cull_status == 0` for non-candidate or
      visible-cell reject, `cull_status == 1` for frustum reject. Other indirect
      fields are ignored for those slots. `DrawAll` selects the fallback
      tree-walk route and preserves its output.
- [ ] Camera cull dispatch work is proportional to the candidate leaf count
      (sum of visible cells' leaves), not total leaf or node count. A CPU unit
      test on a synthetic world asserts the gather visits only visible cells'
      spans.
- [ ] Before the candidate pass, both `indirect_draws` and `cull_status` are
      cleared to zero over the camera world ranges only, with offsets and sizes
      derived from total BVH leaves. A leaf dropped this frame cannot retain a
      previous visible command (`index_count = 0` default) or a stale overlay
      status (zero = non-candidate / not-submitted). Candidate frustum rejects
      write `cull_status = 1`; submitted candidates write `2`. A CPU mirror or
      regression proves it.
- [ ] The draw path is unchanged: identical `bucket_ranges`, identical
      `draw_indirect_buckets` per-bucket call count and ordering as today.
      Cleared slots remain in-range as zero-`index_count` draws; the plan adds
      no per-`(cell, bucket)` fragmentation and no new bucket records. The
      full bucket ranges still include cleared slots by design; diagnostics
      expose candidate leaves vs total leaves so future compaction remains a
      measured decision.
- [ ] `VisibleCells::DrawAll`, non-portal `VisibleCells::Culled` fallbacks
      (camera-in-solid / exterior / no-portals), and a missing/invalid section
      all use the legacy tree-walk camera path for that frame.
- [ ] Shadow cull remains on the existing `ShadowCullPipeline` tree-walk path,
      with unchanged source wiring for dynamic spot-shadow passes; existing
      CPU-only shadow tests still pass. Manual GPU behavior checks are allowed
      but non-gating.
- [ ] Spatial-tab diagnostics report candidate leaf count, total leaf count,
      submitted leaves, and which camera path the frame used. CPU-derived; not a
      perf gate.
- [ ] `candidate_cull.wgsl` reuses the `BvhLeaf`, `DrawIndexedIndirect`,
      `FrustumPlane`, and `CullUniforms` definitions from `bvh_cull.wgsl`
      byte-for-byte. It copies or shares `is_aabb_outside_frustum` with
      `bvh_cull.wgsl` byte-for-byte, covered by shader validation tests. The
      exact 16-byte candidate params uniform
      (`candidate_count: u32, _pad0: u32, _pad1: u32, _pad2: u32`) has
      CPU/WGSL serialization or parse/validation coverage, through
      `wgsl_bvh_struct_strides_match_spec` or an equivalent helper.
- [ ] `cargo test -p postretro-level-format`, `-p postretro-level-compiler`,
      `-p postretro`, and `cargo check -p postretro --features dev-tools` pass.
- [ ] No acceptance criterion requires a GPU or `POSTRETRO_GPU_TIMING=1`.
- [ ] No new `unsafe`.

- [ ] `context/lib/rendering_pipeline.md` describes the candidate cull path, and
      `context/lib/build_pipeline.md` records `CellDrawIndex` id 37 plus its
      presence rule.

## Tasks

### Task 1: Split PRL pack/load seams

Both extension points exceed the split-before-extend threshold:
`crates/level-compiler/src/pack.rs` (1316 lines) and
`crates/postretro/src/prl.rs` (2620 lines). Behavior-preserving: add a generic
optional-section append seam in `pack.rs`, for example
`append_optional_section(sections: &mut Vec<SectionBlob>, section_id: u32,
data: Option<Vec<u8>>)`, implemented as the existing `if let Some(bytes) {
sections.push(SectionBlob { section_id, version: 1, data: bytes }) }` pattern.
Also add a generic optional-section read seam in `prl.rs`, for example
`read_optional_section_data<R: Read + Seek>(cursor: &mut R, meta:
&ContainerMeta, section_id: u32) -> Result<Option<Vec<u8>>, PrlLoadError>`,
implemented as the current `prl_format::read_section_data(...)` wrapper. Task 2
turns these generic seams into the CellDrawIndex-specific calls after the
section id and bytes exist. No behavior change in this task.

### Task 2: Add the CellDrawIndex PRL section

Add `SectionId::CellDrawIndex = 37` and map it in `SectionId::from_u32`
(`crates/level-format/src/lib.rs`; current max is `NavMesh = 36`, id 37 is
free). Add a `cell_draw_index` module with the section type, `to_bytes`,
`from_bytes`, round-trip tests, and malformed-byte tests.

Wire layout is little-endian:

```text
u32 version = 1
u32 cell_count
u32 span_count
u32 reserved = 0
u32 cell_span_offset[cell_count + 1]
struct Span { u32 leaf_start; u32 leaf_count; } spans[span_count]
```

`from_bytes` rejects unsupported version, non-zero reserved, mismatched expected
length, trailing bytes, non-monotonic offsets, `offset[0] != 0`,
`offset[cell_count] != span_count`, span checked-add overflow, and
`leaf_count == 0`. Expected length is
`16 + 4*(cell_count + 1) + 8*span_count`. `from_bytes` does not know
`total_leaves`, the BVH leaf array, cell drawability, or material buckets; those
cross-section validations live in Task 4. Use the Task 1 optional-section seams
to append and read the raw `CellDrawIndex` bytes without deepening
`pack_and_write_portals` or `load_prl`.

### Task 3: Compiler bake of the draw index

After the global BVH is flattened (`build_bvh`, `main.rs:420`) and before
`pack_and_write_portals` (`main.rs:1025`), derive the CSR index from the
already-sorted leaf array. The drawable predicate joins each `BvhLeaf.cell_id`
into the compiler's encoded BSP leaf records (`BspLeafRecord`, fields
`face_count: u32`, `is_solid: u8`); since `cell_id == BSP leaf index`, the join
is a direct index. A drawable leaf is `index_count > 0` whose cell satisfies
`is_solid == 0 && face_count > 0`. For each `cell_id`, collect its contiguous
per-bucket leaf ranges into `spans`; fill `cell_span_offset` as a prefix sum
over `cell_count` (BSP leaf count). Serialize cells in ascending `cell_id`.
Within each cell, emit maximal contiguous spans in ascending global `leaf_start`
order. The bake takes the BSP leaf records as an input alongside the flattened
BVH. It runs **uncached**, like the BVH stage it derives from — no cache key or
stage version. Emit through the Task 1 packing seam, gated on non-empty
`bvh.leaves`. Compiler asserts: every drawable leaf appears in exactly one span,
spans are contiguous, maximal, ascending, and within bounds, cells with
`is_solid != 0` or `face_count == 0` have empty ranges.

### Task 4: Runtime load and validation

Decode `CellDrawIndex` into `LevelWorld` as `Option<_>` via the Task 1 seam.
Decode the raw optional section first; validate it only after the runtime
`LevelWorld.leaves` data has been constructed from `BspLeavesSection`, or move
that construction earlier. Validate against the decoded `BvhTree` and the BSP
leaf count, joining `cell_id` into `LevelWorld.leaves` (`LeafData.is_solid`,
`LeafData.face_count`) for the drawable check: reject unsupported version,
`cell_count != leaves.len()`,
non-zero reserved, non-monotonic `cell_span_offset`, `offset[0] != 0`,
`offset[cell_count] != span_count`, any span outside `[0, total_leaves)`
(checked-add), any span assigned to the wrong cell (`BvhLeaf.cell_id != cell`),
any span covering a non-drawable leaf or non-drawable cell, any span crossing a
material bucket, spans out of ascending `leaf_start` order for a cell, adjacent
same-cell/same-bucket spans that could have been one maximal run, overlapping or
duplicate leaf coverage, any drawable leaf missing from the index, and any
non-drawable cell with non-empty offsets. Any rejection logs once, clears the
index, and selects legacy mode.

Renderer handoff contract: add `cell_draw_index: Option<&CellDrawIndex>` to
`LevelGeometry`, populate it in `level_world_to_geometry` from
`world.cell_draw_index.as_ref()`, clone/store it in
`Renderer::install_level_geometry` alongside `bvh_leaves`, and clear it through
the existing `release_level_resources` empty-geometry install path.

### Task 5: Candidate cull path

Add a renderer-owned camera cull strategy alongside `ComputeCullPipeline`.
Candidate eligibility requires a valid loaded index, `VisibleCells::Culled`, and
portal visibility path provenance
(`matches!(visibility.path, VisibilityPath::PrlPortal { .. })`). Keep
`VisibleCells` semantics unchanged: pass a small renderer input
`CameraCullVisibility { cells: &VisibleCells, path: VisibilityPath }` through
`render_frame_indirect` and `record_pre_scene_compute`; derive candidate
eligibility only from that input.
Each candidate-eligible frame:

1. Clear the camera `indirect_draws` and `cull_status` buffers to zero via
   `encoder.clear_buffer`, over only the camera world ranges. Derive the
   indirect offset/size from `total_leaves * size_of::<DrawIndexedIndirect>()`
   and the status offset/size from `total_leaves * size_of::<u32>()`; do not
   clear any future shadow, entity, or packed non-camera ranges if these buffers
   later share storage. The tree walk already clears `cull_status` at
   `compute_cull.rs:324`; the candidate path also clears `indirect_draws`
   because it writes only candidate slots. Output identity is against the CPU
   mirror's normalized semantics: compare all five indirect fields only for
   submitted leaves. For rejected or non-candidate slots, require
   `index_count == 0` and the expected `cull_status`, ignoring stale values in
   other indirect fields. The legacy tree-walk fallback may remain unchanged
   unless separately fixed.
2. On the CPU, expand candidate spans into a flat `candidate_leaves: Vec<u32>`
   of global BVH leaf indices by indexing the CSR with each id in
   `VisibleCells::Culled`. Dedupe visible cell ids before expansion, preserving
   first-seen order, so duplicate cells cannot create duplicate writes to the
   same indirect/status slot. If a visible cell id is outside the loaded
   index's `cell_count`, log once and use the legacy tree-walk path for that
   frame; the renderer must not partially gather a corrupt candidate set. Keep
   span expansion on the CPU rather than adding GPU-side variable-length span
   expansion; candidate counts are expected to be small and this keeps the
   shader branch-light. Upload into a storage buffer sized `total_leaves.max(1)`
   (worst case all leaves); pass the candidate count in a 16-byte uniform:
   `candidate_count: u32, _pad0: u32, _pad1: u32, _pad2: u32`. If
   `candidate_count == 0`, skip the candidate dispatch after clearing buffers.
3. Dispatch `candidate_cull.wgsl` with workgroup size 64 and
   `ceil(candidate_count / 64)` workgroups. Each invocation:
   `if gid.x >= candidate_count { return }`; `leaf =
   leaves[candidate_leaves[gid.x]]`; test the leaf AABB against the frustum; on
   pass write all five `DrawIndexedIndirect` fields into that leaf's existing
   global slot and set its `cull_status` to `2` (submitted), else write
   `cull_status = 1` (frustum-rejected) and leave the indirect slot cleared.
   Non-candidate leaves remain cleared to `0`.

The shader reads BVH leaves, frustum planes (`CullUniforms`), and the candidate
buffer only; it does not read BVH nodes or `skip_index`. Bindings:
`0 = CullUniforms`, `1 = leaves`, `2 = indirect_draws (read_write)`,
`3 = cull_status (read_write)`, `4 = candidate_leaves (read)`,
`5 = CandidateCullParams uniform` with the exact 16-byte layout above. Depth
pre-pass and forward pass draw from the same `indirect_draws` buffer and
`bucket_ranges` as today. `DrawAll`, non-portal fallback visibility, and a
missing index route to the existing tree-walk dispatch unchanged. Shadow cull
keeps the tree walk.

### Task 6: CPU equivalence and diagnostics

Add a CPU mirror helper returning submitted leaf indices, unchanged renderer
`bucket_ranges` over the global indirect slots, and per-leaf indirect commands
for both paths. It must not compare or produce submitted-only bucket compaction.
Assert equality on synthetic worlds and deterministic stress-map probes. Add
checked-in probe fixtures for `stress-warren`,
`stress-warren-crates`, and `campaign-test` with map path or PRL path, camera
origin, yaw/pitch or view matrix, FOV/aspect/near/far, and comparison mode.
Because those maps are large, any tests that compile or load the full maps are
`#[ignore]` / on-demand; routine `cargo test` coverage uses compact synthetic
or checked-in CPU fixtures derived from those probes. Add
an explicit duplicate-visible-cell unit test proving candidate gather dedupes
before expansion. Add the Spatial-tab diagnostics named in the AC, including
candidate leaves vs total leaves so a future optional indirect compaction pass
is based on measured pressure rather than assumption. Update
the naga struct-stride test (`wgsl_bvh_struct_strides_match_spec`) or an
equivalent helper so both `bvh_cull.wgsl` and `candidate_cull.wgsl` are covered,
including `BvhLeaf`, `DrawIndexedIndirect`, `FrustumPlane`, and `CullUniforms`
interface validation, byte-for-byte `is_aabb_outside_frustum` equivalence, and
the candidate params ABI. Update `context/lib/rendering_pipeline.md` to describe
the candidate cull path. Update `context/lib/build_pipeline.md` to list
`CellDrawIndex` id 37 and the presence rule: emitted when BVH leaves are
non-empty, omitted when zero leaves, independent of portal presence.

## Sequencing

**Phase 1 (sequential):** Task 1 — split-before-extend for the pack/load seams.
**Phase 2 (sequential):** Task 2 — establishes the wire contract.
**Phase 3 (concurrent):** Task 3, Task 4 — bake and decode against the Task 2 contract.
**Phase 4 (sequential):** Task 5 — consumes the loaded index and the candidate gather.
**Phase 5 (sequential):** Task 6 — verifies equivalence and reports diagnostics.

## Rough sketch

Camera path (candidate-eligible frame):

```text
portal visibility path -> VisibleCells::Culled(cells)   (unchanged, authoritative)
-> candidate_leaves = concat over deduped valid c in cells of spans[ offset[c] .. offset[c+1] ]   (CSR expand, CPU)
-> clear indirect_draws and cull_status
-> candidate_cull dispatch (one invocation per candidate leaf, wg 64):
     i = gid.x; if i >= candidate_count: return
     leaf = leaves[candidate_leaves[i]]
     if leaf AABB outside frustum: leave indirect cleared, status = 1
     else write all 5 DrawIndexedIndirect fields, status = 2
     non-candidate leaves remain status = 0
-> draw_indirect_buckets per material bucket   (unchanged; cleared slots are zero-index_count no-ops)
```

Fallback path (`DrawAll`, solid/exterior/no-portals, missing/invalid section):

```text
-> existing ComputeCullPipeline tree walk over the global BVH
```

Output identity holds because the spans of the visible cells are exactly the
leaves whose `cell_id` is portal-visible. The candidate set is pre-frustum;
submitted output matches the tree walk after both paths apply the same frustum
predicate.

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
> `perf-cluster-pvs-static-candidates`, `perf-baked-visibility-region-masks`,
> and `E17--kinematic-platform-foundation`. This draft is the active candidate
> and owns `37` if promoted first; if an alternative promotes instead, reconcile
> the losing drafts before implementation.

## Wire format

`CellDrawIndex` is optional; little-endian. Absence means legacy tree-walk
camera cull. Mirrors the CSR shape of an offset table plus a flat payload, like
existing index sections.

```text
u32 version = 1
u32 cell_count            // == BSP leaf count for non-empty maps
u32 span_count            // total leaf spans
u32 reserved = 0          // decode rejects non-zero
u32 cell_span_offset[cell_count + 1]   // prefix sums; cell c owns spans[offset[c]..offset[c+1])
struct Span { u32 leaf_start; u32 leaf_count; } spans[span_count]
```

`cell_span_offset` is non-decreasing, `cell_span_offset[0] == 0`, and
`cell_span_offset[cell_count] == span_count`. Each span is a contiguous range of
the global `BvhSection::leaves` array lying within a single material bucket;
`leaf_count > 0`, and `leaf_start + leaf_count <= total_leaves` (checked-add,
no overflow). Cells are serialized in ascending `cell_id`. Each cell's spans are
maximal contiguous runs emitted in ascending global `leaf_start` order; runtime
validation rejects empty spans, out-of-order spans, and adjacent
same-cell/same-bucket spans that are not maximal. Every drawable leaf is covered
by exactly one span; cells with
`is_solid != 0` or `face_count == 0` have empty ranges
(`offset[c] == offset[c+1]`). Decode rejects unsupported version, non-zero
reserved, count mismatches, non-monotonic offsets, overflow, and trailing bytes.

Expected length: `16 + 4*(cell_count + 1) + 8*span_count`.
