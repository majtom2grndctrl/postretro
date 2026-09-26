# SH Probe Streaming — Cell Clustering and Cluster Directory

> Slice 2 shipped in PR #516. Parent: `context/plans/done/sh-probe-streaming/`.
> The spec below records the original implementation contract. Slice 1's GPU premise
> remains `not-yet-evaluable`; this record does not claim a measured streaming benefit.

## Goal

Bake a deterministic, resource-agnostic partition of runtime cells and an inert PRL directory addressing existing SH data. Establish the compiler → format → loader contract before runtime residency consumes it.

## Scope

**In:** adjacent-cell clustering under a primitive budget; a cell-count bound for zero-primitive regions; stable ids; section 49; complete cell membership, bounds, grid-relative resource ranges, fixed delta ownership; strict codecs and cross-section validation; unchanged bake/render output; distribution evidence and threshold selection.

**Out:** runtime planner, partial reads/uploads, streaming, eviction, ownership transfer, miss policy, new GPU bindings, shaders, authored hints, per-cluster files, SH ray/coarsening changes, byte-slice gathering, and streaming another resource. No `unsafe`. No new player option, map KVP, or production CLI switch.

## Shared context

Every task reads `context/lib/index.md`, `development_guide.md`, `testing_guide.md`, `build_pipeline.md`, `rendering_pipeline.md`, this whole brief, and its `research.md`. Read the parent epic and `measurements/sh-probe-streaming/premise.md` once before execution. Use current source over stale parent line anchors.

This is a persistent cross-crate contract; all implementation tasks require **xhigh** reasoning. Task agents receive the full AC, wire, invariants, and failure tables, not an isolated task paragraph. Proposed paths/names are marked here; grounded entry points and callers are in research. Maintain public re-exports during mechanical splits. Do not change existing section epochs or layouts to accommodate the directory.

## Decisions

### Partition

Partition **all** id-38 cells, including solid, exterior, and zero-face cells. Adjacency is an undirected edge from the finalized id-15 portal endpoints; no AABB-proximity shortcut or lighting-weighted edge. Cells with no edges are singletons. A multi-cell cluster must be connected in this graph. Preserve runtime cell ids.

Cell weight is the count of nonempty static BVH primitives owned by that cell, not triangles, SH bytes, lights, material count, or stored-probe density. Greedily grow each cluster through unassigned adjacent cells while both positive limits fit: total primitive count and cell count. An individually over-budget cell becomes a flagged singleton and emits one compiler diagnostic naming the cell and excess; never split a cell or exceed the limit with multiple cells. This is the only primitive-budget exception.

Use one total cell key: `(bounds_min.z, bounds_min.y, bounds_min.x, cell_id)`, ascending finite numeric order, treating negative zero as zero. Pick the smallest unassigned key as seed; repeatedly admit the smallest frontier key that fits, reconsidering the sorted frontier after each admission. Finish when no candidate fits. Final ids are contiguous `0..cluster_count` in seed-key order: x-fastest spatial order with the source cell id as the last tie-break. Member ids sort ascending. Cluster bounds are the exact component-wise union of member bounds, canonicalizing zero. Portal-list order, primitive-list order, hashing, and worker scheduling cannot affect output.

**Threshold decision during implementation, owned by Task 4:** retained Slice 1 evidence contains whole-section disk totals, no cluster histogram or resident floor. Choose and record positive default primitive/cell limits after a dry run on `campaign-test` geometry and a compact disconnected/budget-boundary fixture. Compare at least three candidate limits; record cluster counts, primitive/cell p50/p95/max, singleton exceptions, directory bytes, and per-cluster SH addressed footprint including halo duplication. Keep the rule above fixed. Expose limits to unit/test harness inputs, not new authoring controls. Pin the chosen constants before Task 7; acceptance cannot ship with provisional limits or without a nondegenerate multi-cluster fixture. Slice 3 must reconcile the resulting maximum addressed SH footprint with its eventual residency floor minus always-resident overhead; this slice makes no unsupported resident-memory-cap claim.

### Placement and ownership

Compute partition and resource addressing after all bakes and final section-presence decisions, immediately before section serialization. Run uncached in warm, `--no-cache`, and compiler `--release` builds. Existing stages, input hashes, epochs, global SH rays, coarsening, atlas geometry, and payload order do not change. The directory reads immutable finalized cells, portals, BVH primitive ownership, id-39 locator, and finalized SH metadata/CSR; it never feeds a bake. A compiler-owned borrowed emission view supplies exactly the sections the packer will write after direct-selection and scatter-cap filtering. Build that view once, then use it for directory construction and serialization. Do not discover presence separately in two places.

Append section 49 after all existing sections, preserving their relative table/payload order and container-entry versions. Existing offsets necessarily move by one 22-byte table entry; their section-relative layouts and payloads do not. PRL header remains version 4. Encode only the current payload during pack, retaining the existing one-payload-at-a-time lifetime; directory generation must not clone or re-encode all lighting payloads.

### Spatial addressing

The directory is an index, not an SH byte-offset table. Resource domains are dense probe indices for ids 34/35/47 and 4×4×4 affinity-cell indices for ids 27/41/45/48. Flatten both x-fastest: `x + dim_x * (y + dim_y * z)`. Ranges use `start,count`, with exclusive checked end. No atlas slot, compressed block offset, kept rank, CSR-entry index, light slot, or descriptor index is serialized in section 49.

Define id-34 probe-center positions from the serialized base grid as `grid_origin + [probe_x, probe_y, probe_z] * cell_size`, where `cell_size` is the id-34 probe spacing vector in engine meters. For an affinity brick `(brick_x, brick_y, brick_z)`, clipped probe-index bounds are:

- `min_index = [brick_x, brick_y, brick_z] * 4`
- `max_index = min(min_index + [3,3,3], grid_dim - [1,1,1])`

The brick is invalid/missing when any grid dimension is zero; do not subtract one from a zero axis. For a valid scale-0 brick, including a partial edge brick, its probe-support AABB is the closed half-interval around the outer clipped probe centers: `support_min = center(min_index) - 0.5 * cell_size`, `support_max = center(max_index) + 0.5 * cell_size`. This half-cell-expanded AABB is a new section-49-specific helper: current compiler helpers use the outer probe centers without the half-cell expansion, and must not be cited as already providing this exact support box. A non-solid, non-exterior member cell contributes its cell bounds expanded component-wise by exactly `cell_size`; closed AABB intersections count. These formulas distinguish probe centers, affinity-brick support, and cluster/member-cell bounds.

Build the initial common coverage set of affinity bricks containing a valid id-34 probe **or** any CSR entry in an emitted delta companion. Initial cell-to-brick contact is the only place that uses closed AABB intersection: a brick touches a cluster when its probe-support AABB intersects any expanded member-cell bounds above. Also include the cluster containing every valid probe center, located through the serialized id-39 locator semantics (front on plane). For an entry-bearing brick with no valid probe and no spatial cover, compute the affinity brick origin probe index as `[brick_x * 4, brick_y * 4, brick_z * 4]`, component-wise clamp it to `[0, grid_dim - 1]` only when every grid dimension is nonzero, convert that clamped id-34 probe center to world space using the formula above, then locate it through serialized id-39 front-on-plane semantics. A zero grid is an invalid/missing base-resource condition, not an underflowing fallback. These rules guarantee an owner even for canonical zero-contribution delta entries. Locator failure is a compiler error, not guessed membership.

For each initially covered brick, choose the lowest covering cluster id as owner; all other covering clusters are halo. Then expand coverage to include complete support of every id-34 adaptive node actually referenced by covered probe metadata, including its derived origin brick, and recompute owners from the final covering sets. Id 34 stores each probe's `node_scale`, not a node origin. The adaptive-node closure is exact brick-index membership, not AABB growth: for each dense probe index inside a covered brick, read that probe's `node_scale`; compute its affinity brick as `floor(probe_index / 4)`; derive `node_origin_brick = floor(brick / 2^scale) * 2^scale`; derive `node_origin_probe = node_origin_brick * 4`. Scale 0 adds only the probe's own affinity brick. For scale > 0, the current format accepts only a full aligned `4 * 2^scale` probe cube entirely inside the id-34 grid; if `node_origin_probe + [4 * 2^scale - 1]` exceeds any grid axis, the id-34 section is invalid before directory validation. Scale > 0 adds exactly the node's constituent `2^scale × 2^scale × 2^scale` affinity-brick index cube and no neighboring bricks merely because support boxes touch. Iterate only when newly added bricks expose previously unvisited `(node_origin_brick, node_scale)` records through their own covered probe metadata; keep a finite visited set so closure cannot degenerate into Cartesian expansion. This affects only resource ranges, never the cell partition. It preserves scale-0/1 production and accepted scale-2/3 measurement data without splitting stored nodes. Id 34/35 dense ranges contain every clipped probe index of covered bricks; id 47 follows the same dense grid coverage. Coalesce maximal consecutive indices per cluster/resource. Sparse companions address the final common covered brick set, including empty CSR rows; all entries in one `(section, affinity cell)` inherit the same owned/halo status. Id 48 inherits exactly id 45's coverage and ownership. Cross-resource owner agreement follows the common covering set; halo grants reconstruction access, never another accumulation right. Dense ranges claim no stored-slot ownership: Slice 3 resolves their adaptive-node representation.

## Acceptance criteria

- [ ] **AC1 — Partition:** every emitted runtime cell occurs once, including solid/exterior/empty cells; bounds equal the member union; ids/order follow the pinned rule; multi-cell clusters are portal-connected and within both limits. Only flagged indivisible singletons may exceed the primitive limit. Evidence selects and records final positive defaults.
- [ ] **AC2 — Determinism:** two independent `--no-cache` bakes of the same multi-cluster fixture produce byte-identical section 49, including ranges/ownership, with different worker counts. Permuting portal and primitive inputs preserves directory bytes. A warm rebuild keeps the same cell partition and produces a directory valid for its own finalized SH metadata.
- [ ] **AC3 — Address completeness:** every emitted/present SH-family section has one resource descriptor, including valid-empty sections; absent or withheld sections have none. Ranges cover required probe/node reconstruction support and all entry-bearing affinity cells, including empty-validity zero-contribution entries and partial scale-0 edge bricks. Valid-empty sparse companions over a nonzero id-34 grid retain normal affinity dimensions and range over the final common covered brick set, including empty rows; they have zero ranges only when that common covered set is empty. The same logical directory addresses BC6H and uncompressed versions of identical grid/metadata/CSR inputs without byte-offset dependence.
- [ ] **AC4 — Ownership:** a cross-cluster affinity cell has exactly one owned reference and explicit halo references in all other covering clusters, stable across cold bakes. Every CSR entry inherits that owner; ids 45/48 agree. Scaled-node support closure is complete for full-grid-contained scale>0 nodes, uses exact constituent brick membership rather than support-AABB adjacency, and does not repartition cells.
- [ ] **AC5 — Format/load:** section 49 round-trips through compiler publication and runtime loading. Version mismatch, out-of-range cell/probe/affinity reference, overflow, malformed ordering/flags, missing target section, valid-empty sparse companion range/dimension disagreement, and invalid ownership fail with the named errors below. Missing section 49 leaves full-load behavior unchanged. Explicit empty and single-cluster encodings follow the wire rules.
- [ ] **AC6 — Companions:** validation uses the emitted/parsed section inventory, including valid-empty animated-direct data with zero CSR entries and normal affinity dimensions, and checks all ids 27/34/35/41/45/47/48 plus their selection/descriptor dependencies. Existing optional-section rejection and size-floor behavior remains unchanged; unavailable companions disable the inert directory as specified, without bypassing existing validation. Compiler and runtime terminology must consistently separate on-wire/emitted presence, parsed validity, policy availability, and exposed runtime `Option` fields.
- [ ] **AC7 — Inert output:** section 49 is never read by rendering, visibility, gameplay, content-parity hashing, or frame scheduling. Given identical renderer-visible lighting inputs, directory-present and directory-removed files have identical loaded lighting payloads, SH/GPU allocations, per-frame allocations/work, and same-adapter frame output. The only permitted difference is one-time CPU storage for the parsed inert directory metadata. Manual windowed verification also covers animated billboard lighting, excluded by the static capture harness.
- [ ] **AC8 — Bake/cache preservation:** adding the directory changes no pre-existing uncompressed section payload, layout, epoch, relative section order, or cold global-SH result. Existing compressed-output tolerance rules remain; directory bytes have no lossy exception. Warm caches are reused; no clustering cache is created; stage reporting includes the new metadata stage. Existing SH cold determinism and warm-tolerance gates pass.
- [ ] **AC9 — Bounded lifecycle:** no full-family payload clone/encode is introduced for clustering, validation, or diagnostics. Overflow fails before allocation. Record directory construction/serialization peak metadata bytes and elapsed time on the selected fixture, including covering-set/range overlap, with worker/cache/cleanup details. Source splits and all downstream constructors/callers compile in supported feature configurations.

## Boundary inventory

All names in this table are **proposed**. No JS, TS, Luau, FGD, network, or renderer boundary is added.

| Meaning | Rust owner | Wire |
|---|---|---|
| Directory | `level-format::cluster_directory::ClusterDirectorySection`; `SectionId::ClusterDirectory` | id 49; container entry version 1; section epoch `u32 = 1` |
| Runtime storage | `LevelWorld.cluster_directory` under `load-prl` | `Option<ClusterDirectorySection>`; absent/unavailable = `None` |
| Resource domain | shared directory codec | `u32`: 0 dense probe, 1 affinity cell |
| Range role | shared directory codec | `u32`: 0 dense, 1 owned, 2 halo; no bit combinations |
| Indivisible oversize | cluster record | flags bit 0; all other bits zero |
| Named failures | `ClusterDirectoryError`, wrapped at compiler/loader boundary | names in Failure table; not new on-wire strings |

## Wire format

Section 49 mirrors `Cells`' version-first, count-framed, reserved-zero discipline and `CellDrawIndex`'s flat indexed tables. It does **not** copy an existing whole layout. All integers are unsigned little-endian; bounds are finite IEEE f32 engine meters. No implicit alignment, platform padding, or trailing bytes. Counts are u32; checked size arithmetic uses u64/usize before allocation. The total length must equal `40 + 48*C + 24*R + 4*M + 24*N` exactly.

| Block | Fixed offsets / order |
|---|---|
| Header, 40 bytes | 0 epoch=1; 4 runtime_cell_count; 8 cluster_count C; 12 resource_count R; 16 member_count M; 20 range_count N; 24 primitive_limit; 28 cell_limit; 32 reserved=0; 36 reserved=0 (all u32) |
| Cluster records, C × 48 | 0 bounds_min f32×3; 12 bounds_max f32×3; 24 member_start; 28 member_count; 32 range_start; 36 range_count; 40 primitive_count; 44 flags (six trailing u32) |
| Resource records, R × 24 | 0 section_id; 4 domain; 8 dimensions u32×3; 20 reserved=0 |
| Members, M × 4 | Flat runtime cell ids, cluster-major, ascending per cluster; M=runtime_cell_count |
| Ranges, N × 24 | 0 resource_index; 4 start; 8 count; 12 owner_cluster_id; 16 role; 20 reserved=0 (u32) |

Cluster id is implicit array position. Cluster table is seed-key order. Resource table is ascending numeric section id, unique; epoch 1 permits only ids 27/34/35/41/45/47/48 with the domains above. Dense dimensions equal id 34's grid; affinity dimensions equal component-wise ceiling of that grid divided by four. A resource record uses `[0,0,0]` dimensions only when the underlying section/grid dimensions are zero. A valid-empty CSR companion over a nonzero id-34 grid has zero CSR entries, carries normal affinity dimensions, and emits ranges over the final common covered brick set including empty rows; its range count is zero only when the final common covered set is empty. Mixed-zero axes reject. Empty lists encode count zero and no bytes; zero-count cluster ranges use start zero. Nonempty per-cluster member/range slices are contiguous in cluster order and consume their entire tables. Each cluster has at least one member. Ranges sort by `(resource_index,start)`; no overlaps inside a cluster/resource; adjacent equal-role/equal-owner runs must coalesce. Counts are positive for every range. Dense role=0 and owner=`0xffffffff` (sole sentinel); affinity role is 1 or 2, owner is a real cluster id, and role=1 iff the containing cluster equals that owner. There is no null affinity owner.

Standalone all-empty encoding: 40-byte header with epoch 1, positive limits, and all counts/reserved zero. Decoder accepts it; current level loading rejects it when id 38 contains cells or SH carries a nonempty grid. Current id 38 disallows zero cells, so ordinary empty-geometry maps still partition their real cells and emit id 34's zero-grid resource descriptor with no ranges. A one-cell map encodes one cluster, one member, zero or more normal resources/ranges; no special implicit whole-map sentinel.

## Validation and failure behavior

Use the shared codec/semantic validator from compiler pre-publication checks and runtime load. Structural section-49 errors always reject before optional-companion handling. Validate lengths before allocation, checked range ends/products, finite ordered bounds, partition completeness, canonical ordering, owner uniqueness, role consistency, resource-domain/dimension agreement, and missing/extra resources. Validate section 49 against id 38/15 connectivity, BVH primitive ownership/counts and id 39 where spatial coverage is checked. Do not construct an unbounded cell×probe Cartesian matrix; use sparse covering sets and range sweeps.

The section-49 validator tracks each optional SH companion with this inventory vocabulary. In compiler prose, "emitted" means the finalized pack view will write the old section; in runtime prose, "on-wire present" means the PRL container contains it. Use the same state table on both sides:

| Inventory state | Meaning | Directory effect |
|---|---|---|
| On-wire present / emitted | The section is in the finalized section set, including a valid-empty payload. | Section 49 must contain exactly one resource row for it, and no row for absent/withheld sections. A valid-empty sparse companion over a nonzero id-34 grid still has normal affinity dimensions. |
| Parsed-valid | The section decoded and passed its own old validation, even if later policy filtering exposes `None`. | Participates in section-49 dependency checks, dimensions, CSR shape, descriptor rules, ownership validation, and empty-row range checks. |
| Policy-available | Existing size floor/cap and companion policy allow semantic use. | Required for full semantic directory availability; over-cap or policy-rejected sections make the directory unavailable per the table below. |
| Exposed-runtime | The legacy runtime field is `Some` after all old filtering. | Not sufficient for section-49 validation; valid-empty id 45 may be parsed-valid and exposed as `None`. |

| Condition | Exact behavior |
|---|---|
| No id 49 | Silent `None`; full level load/render unchanged |
| Duplicate id 49; wrong container-entry or internal epoch | Fatal `ClusterDirectoryVersionMismatch` or `ClusterDirectoryInvalidData` for duplication |
| Runtime cell out of range; grid range overrun | Fatal `ClusterDirectoryCellOutOfRange` / `ClusterDirectoryGridRangeOutOfRange`, naming cluster/resource/start/count/limit |
| Count/end/size overflow or allocation refusal | Fatal `ClusterDirectorySizeOverflow` / `ClusterDirectoryAllocationFailed`; no panic |
| Gap/duplicate membership; disconnected cluster; bad union/flags/budget/order/owner/coverage | Fatal `ClusterDirectoryInvalidData`, naming the offending record and invariant |
| Resource row names absent on-wire section, or emitted SH section has no row | Fatal `ClusterDirectoryMissingResource` |
| Present valid companion disagrees with dimensions/CSR/selection/descriptor rules, including valid-empty sparse companion dimensions/ranges | Preserve its existing reject/fallback behavior first; a directory disagreement with otherwise valid companion data is fatal `ClusterDirectoryResourceMismatch` |
| Existing optional companion rejected/over policy cap, preventing complete semantic validation | Preserve its existing lighting fallback; keep directory structurally checked, expose `None`, warn once `ClusterDirectoryUnavailableCompanion` with ids. Never decode an over-cap payload just to validate directory metadata |
| Zero clusters with nonzero cells or nonempty SH grid | Fatal `ClusterDirectoryInvalidData` |

For accepted inputs, validate id 35 against id 34 node geometry/format/slots; ids 27/41/45 against id 34 grid and kept-probe rules; id 41 against id 40 selection; id 47 grid/validity against id 34; id 48 grid/descriptor/CSR against id 45 and its required id 47 base. When id 49 is present, add a new directory-specific validation for id 27/id 45 descriptor-index arrays: each `animation_descriptor_indices` value must be `u32::MAX` or `< id34.animation_descriptors.len()`. This is not claimed as an existing legacy loader check, and missing id 49 leaves legacy no-49 load behavior unchanged. A descriptor-index violation in otherwise accepted old companion data is `ClusterDirectoryResourceMismatch`; if the existing failure table first makes that companion unavailable because of rejection or policy cap, follow the unavailable-directory row instead. Retain parsed-valid empty id 45 through these checks even when the exposed runtime delta is subsequently `None`. Derive availability from on-wire presence plus decode/policy outcomes, not only the final runtime Options. Directory failure cannot silently enable a rejected lighting section. Compiler emitted sections must all validate; the runtime-only unavailable-companion warning is never a compiler-success escape.

## Invariants

| Invariant | Established by | Preserved / threatened at | Proof |
|---|---|---|---|
| One source cell, one cluster; connected bounded growth | T4 | T2 codec, T6 load | AC1/5 |
| Stable spatial ids and canonical bytes | T2/T4 | packing, input order, worker scheduling | AC2/5 |
| Grid indices never imply storage offsets or spatial flood-fill | T2/T4 | adaptive nodes, compression, delta kept rank, closure locality | AC3/4/8 |
| One delta owner; all other coverage is halo | T4 | coalescing and companion decoding | AC4/6 |
| Directory derives from the actual final emission set | T3/T4 | pack suppression, empty id45, optional runtime fallback | AC3/6 |
| Global bake/cache and all preexisting wire contracts unchanged | T1/T3/T5 splits, T4 integration | side-table placement and signature changes | AC7/8/9 |
| Inert CPU data; no runtime residency or GPU consumer | T6 | loader construction and renderer install | AC7/9 |
| No whole-lighting-family temporary copy | T2/T4/T6 | validation/readback/serialization | AC9 |

## Tasks

### Task 1: Split shared container code before registry extension

Behavior-preserving extraction in `crates/level-format/src/lib.rs` (1,167 lines at drafting): separate section registry and container framing/I/O from module declarations, retaining existing root exports and exact container bytes. Keep tests with their responsibility; do not alter codecs. Capture the existing registry/container test baseline and run dependent checks. This prepares T2; AC8/9.

### Task 2: Define the directory codec and shared semantic contract

Add proposed `crates/level-format/src/cluster_directory.rs` with the exact Boundary/Wire/Failure contract and checked serializer/parser. Add id 49 through the extracted registry. Implement borrowed semantic validation inputs for cell/portal/BVH and SH metadata/CSR, so pack and loader share the contract without full payload copies. Existing section decoders remain authoritative for their own layouts. Freeze all section-relative offsets and epochs recorded in research; do not reversion or normalize old sections. Tests cover every structural/semantic negative row, empty/single encodings, valid-empty sparse companions with normal nonzero affinity dimensions, and closure-locality cases using production codecs. AC1–6/8/9.

### Task 3: Split compiler assembly before directory integration

Behavior-preserving extraction of `pipeline.rs` (3,002 lines), `pack.rs` (2,905), `main.rs` (3,092), and `pack_output.rs` (1,179) along grounded seams in research. Separate stage registry from orchestration; runtime spatial encoding and finalized-section planning from pack entry wrappers; output readback from publication; CLI data/parsing from the binary root. Preserve existing signatures/re-exports, callers, planned-stage order, all section presence filters, and publication locking. Keep this commit free of directory behavior. Before T4, retain the small uncompressed cold baseline and its exact build recipe/hashes for T7's old-section comparison; keep binaries temporary. This prepares T4 and its small module declarations/stage insertion; AC8/9.

### Task 4: Implement clustering, coverage, and final-emission integration

Create proposed `crates/level-compiler/src/cluster_directory_bake.rs`. It borrows final cells/portals/BVH/locator plus the single finalized SH emission view, returns owned directory metadata, and never mutates its inputs. Implement Partition/Spatial addressing exactly, including sparse coverage and exact brick-index node closure; decide defaults under the threshold protocol. Invoke it through a new reported metadata stage after existing bakes and presence filtering, before serializers run; update the extracted `StageId` registry and its order tests. Route the result through both pack entry wrappers and their local tests, with only `pipeline::run_after_parsing` as the production caller. Append its planned section and validate directory/companions before publication, then parse/check id 49 during same-handle readback. Preserve the no-reencoding footprint report, bounded closure visited-set metadata report, warm cache reuse, and publication semantics. Add small synthetic graph/grid tests, including the closure-locality regression and valid-empty sparse companion fixture, plus cold integration coverage. AC1–6/8/9.

### Task 5: Split loader data and decoding before extension

Behavior-preserving extraction of `prl.rs` (7,310 lines) and `prl_loader.rs` (4,447): separate `LevelWorld` data/construction from spatial queries and lighting decode/validation from core spatial load assembly, preserving crate exports and optional-section policies. Keep typed parsed-valid-empty companion state available at its current validation seam. Update existing test modules/imports without changing fixtures or adding directory state yet. This prepares T6; AC6/8/9.

### Task 6: Load the directory as validated inert data

Add proposed optional directory storage to `LevelWorld` under `load-prl`; write it only in the final successful loader assembly, defaulting to `None` in `new_visibility_only` and every test/helper constructor. Parse id 49 through the shared codec; collect on-wire presence and accepted metadata during existing decode operations; run shared semantic validation before post-policy filtering loses valid-empty id45. Apply Failure behavior, with typed loader wrapping and named diagnostics. Update all struct-literal callers found workspace-wide; no renderer plumbing. Tests use generated containers and production section codecs to exercise directory+all companions, missing directory, absent target, corruption, policy caps, and a shared compiler/loader fixture with valid base plus valid-empty id45 and valid-empty id48. AC3–7/9.

### Task 7: Prove determinism, cache preservation, and inert rendering

Complete the proof matrix in research using production compiler → container → loader paths, not twin handwritten fixtures. Run two no-cache multi-cluster bakes with 1 and 8 workers; compare exact directory bytes and old uncompressed section payloads against the pre-T4 baseline. Keep BC6H tolerance rules for old sections. Run the existing cold/warm SH gates once, the closure-locality regression, and the targeted optional-companion regressions. Record budget/distribution/resource-lifetime results, bounded closure metadata, commands, source hashes, cache mode, timings, and cleanup. On an adapter-equipped host compare a current PRL with a test-created copy that omits only id49: same scene/time/adapter/resolution, exact captured RGBA and equal SH allocation reports; windowed animated billboard check covers the omitted capture consumer. A missing adapter records `not-yet-evaluable` and leaves AC7 open, never substitutes CPU evidence. Finish required preflight/review checks and document remaining manual evidence honestly. AC1–9.

## Sequencing

**Phase 1 (concurrent):** T1, T3, T5 — independent behavior-preserving splits in separate crates.
**Phase 2 (sequential):** T2 — consumes T1's registry seam; freezes shared contract before consumers.
**Phase 3 (concurrent):** T4, T6 — consume T2 and their respective split; compiler and loader own separate files.
**Phase 4 (sequential):** T7 — integrates compiler and loader and closes the proof matrix.

## Open questions

- No unresolved owner choice changes this slice's scope. Primitive/cell constants are an explicit T4 implementation decision with a required record, not permission to loosen AC1.
- Slice 1 has not established a performance win. Promotion/execution follows the owner's decision about that parent prerequisite; this draft does not rewrite it.
- Visual acceptance needs a working GPU adapter. Retained Slice 1 evidence reports none exposed to automation. AC7 stays open until measured; Slice 3 remains separate work.
