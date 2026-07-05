# Runtime Cell Spatial Contract

## Goal

Replace runtime BSP as a loaded spatial contract with explicit cells and a
cell locator. Keep BSP in the compiler pipeline where it discovers solid and
empty space. Runtime systems consume cells, portals, cell draw data, BVH draw
accelerators, and the existing collision trimesh.

This is a follow-on to `context/plans/done/perf-visible-cell-candidate-cull/`.
It depends on that plan landing first. This checkout already has
`CellDrawIndex = 37`.

## Scope

### In scope

- Add runtime `Cells` and `CellLocator` PRL sections.
- Stop emitting runtime `BspNodes` / `BspLeaves` sections. BSP remains a
  compiler intermediate only.
- Rename runtime APIs and data flow around cells: `find_leaf` is replaced by
  `locate_cell` (`find_leaf` comparison exists only through Task 3 and is
  removed in Task 4), `VisibleCells` becomes cell-semantic, and callers stop
  treating BSP leaf records as the durable runtime type.
- Treat runtime leaf-index payloads as cell-index payloads according to the
  Boundary Inventory rename table. Preserved legacy names must be documented as
  cell ids.
- Keep portal traversal as the normal room-visibility path. Named fallbacks
  handle solid-camera, exterior-camera, and no usable portals.
- Keep runtime BVH as a draw, shadow cone cull, and diagnostics accelerator.
  Compiler BVH use for bake/light work is unchanged. Do not make BVH the
  room-visibility or occlusion-authority model.
- Keep `CollisionWorld` as the physics source of truth. Cells may provide broad
  phase or semantics later, but collision contacts still come from geometry
  queries.
- Add validation and diagnostics that prove cells, portals, `CellDrawIndex`,
  BVH leaves, fog masks, and locator output agree.

### Out of scope

- Removing compiler BSP construction.
- Replacing `CollisionWorld`, `parry3d::shape::TriMesh`, capsule sweeps, or
  hitscan world rays.
- Dynamic moving occluder geometry or per-frame BVH rebuilds.
- Baked PVS.
- Changing `CellDrawIndex` layout from the candidate-cull plan, except where
  this plan validates it against `Cells`.
- Runtime level compilation.
- General-purpose Godot-style scene-node visibility.
- Legacy PRL compatibility. Maps are rebuilt often during development.

## Acceptance criteria

- [ ] Final-state PRLs after Task 6 contain `Cells` and `CellLocator` sections
      and do not contain runtime `BspNodes` or `BspLeaves` sections. Tasks 2-5
      may emit and load dual-section migration fixtures.
- [ ] PRLs missing `Cells` or `CellLocator` fail to load with a named
      stale-format error. Present but malformed `Cells` or `CellLocator`
      sections fail with named validation errors. The runtime no longer
      synthesizes a fallback leaf for missing BSP data.
- [ ] Modern PRLs with valid `Cells` / `CellLocator` plus legacy runtime
      `BspNodes` or `BspLeaves` fail with a named ambiguous/stale-format error.
      Dual-section fixture PRLs are allowed only through Task 5 migration
      tests; rejection starts in Task 6.
- [ ] Runtime camera visibility no longer calls `find_leaf`. It calls
      `locate_cell`, then portal traversal starts from that cell id.
- [ ] Runtime mesh and particle visibility call the same point-to-cell locator
      used by camera visibility. Particles remain culled by each particle's own
      position, not by their emitter's cell.
- [ ] Runtime light records that currently carry `leaf_index` become
      cell-semantic. `u32::MAX` remains the unassigned sentinel for lights that
      cannot be placed in a non-solid cell or are spawned without cell
      assignment.
- [ ] `VisibleCells::Culled` continues to carry cell ids. `DrawAll` fallback
      behavior remains unchanged.
- [ ] Cell ids preserve legacy BSP leaf ids. Solid, exterior, empty, and
      drawable leaves all have cell records. Portal traversal output for current
      fixture maps matches the old BSP-leaf path because ids are unchanged.
- [ ] Candidate cull consumes cell ids from portal traversal and `CellDrawIndex`
      (already true, per `perf-visible-cell-candidate-cull`); it never walks
      runtime BSP nodes. The only changes are upstream: `VisibleCells` is seeded
      from `locate_cell` (Task 4), and `CellDrawIndex` `cell_count` is validated
      against `Cells.cell_count` instead of BSP leaf count (Task 5).
- [ ] Runtime BVH data remains loaded and uploaded for world draw, shadow cone
      cull, and diagnostics. The spec does not collapse visibility into BVH
      traversal.
- [ ] `CollisionWorld::populate_from_level` plus the collision-module free
      functions `collision::cast_capsule` / `collision::cast_ray` keep using PRL
      static geometry through `parry3d` queries. No collision AC depends on BSP
      or the new cell locator.
- [ ] After behavior lands, `context/lib/build_pipeline.md`,
      `context/lib/rendering_pipeline.md`, and `context/lib/entity_model.md`
      are updated with the new contracts: BSP is compile-only scaffolding,
      runtime visibility uses cells and portals, and collision uses
      `CollisionWorld`.
- [ ] Spatial diagnostics show labeled fields for current cell,
      portal-reachable drawable cells, fog-reachable cells, locator path, and
      candidate BVH leaf counts.
- [ ] Fog/light reachability keeps the wider portal-reachable cell set:
      non-solid empty cells are included, and an empty set is the all-active
      fallback sentinel (the solid-camera / exterior / no-portals case: all
      canonical fog slots active and all-cells light gating, per
      `rendering_pipeline.md` §7.5), not `VisibleCells::DrawAll`.
- [ ] Fog active-mask selection ORs the current camera cell mask in addition to
      fog-reachable cells.
- [ ] Loader validation rejects malformed `Cells` / `CellLocator` sections as
      named load errors. Missing `Cells` / `CellLocator` is stale-format.
      There is no legacy BSP fallback.
- [ ] Modern PRLs missing `Geometry` or `Bvh` sections fail to load with named
      stale-format errors. Present but malformed or invalid `Geometry` or `Bvh`
      sections fail with named validation errors. The minimum modern load
      contract is `Cells`, `CellLocator`, `Geometry`, and `Bvh` valid;
      `CellDrawIndex` valid when the BVH has leaves; `FogCellMasks` valid when
      canonical fog volume count is greater than zero.
- [ ] After Task 6, `postretro-level-compiler` pack tests assert emitted PRLs
      contain no `BspNodes` (id 12) or `BspLeaves` (id 13) sections and do
      contain `Cells` (id 38) and `CellLocator` (id 39).
- [ ] Preferred-path validation covers `Cells`, `CellLocator`, `Portals`, BVH
      leaf cell ids, `CellDrawIndex` for non-empty BVH maps, and `FogCellMasks`
      when canonical fog volume count is greater than zero.
- [ ] Solid-cell camera positions use the existing solid-camera fallback:
      frustum-cull all drawable cells by bounds and skip portal traversal.
      Exterior-cell camera positions use the existing exterior-camera fallback.
      Missing, empty, decode/schema-failed, or intrinsically invalid `Portals`
      sections mean no usable portal graph and use the no-portals fallback.
      Endpoint or adjacency mismatches are fatal when `Portals` is otherwise
      usable.
- [ ] `cargo test -p postretro-level-format`, `-p postretro-level-compiler`,
      `-p postretro`, and `cargo check -p postretro --features dev-tools` pass.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Split PRL Load and Pack Seams

`crates/postretro/src/prl.rs` and `crates/level-compiler/src/pack.rs` are both
past the split-before-extend threshold. Split before adding behavior. Extract
existing spatial/PRL decoding helpers from `prl.rs` into a focused loader
module. Extract optional-section packing helpers from `pack.rs` into a focused
packing module. Keep public behavior unchanged. Existing tests for
`LevelWorld`, `load_prl`, and `pack_and_write_portals` must pass unchanged
before later tasks extend these seams.

### Task 2: Add the `Cells` Section

Add a `Cells` PRL section in `postretro-level-format`. Register
`SectionId::Cells = 38` and its `SectionId::from_u32` mapping in
`crates/level-format/src/lib.rs` (current max `NavMesh = 36`,
`CellDrawIndex = 37`; ids 38/39 are free). A cell is the runtime visibility
unit and preserves the compiler BSP leaf id space one-to-one:
solid, exterior, empty, and drawable leaves all become cells. This avoids a
remap for portals, BVH leaf `cell_id`, fog masks, diagnostics, and tests. The
section stores enough runtime information to replace `BspLeavesSection` for
visibility and diagnostics: bounds, solidity, exterior status, geometry face
range summary, and portal adjacency range. It does not store BSP split planes.
Per-cell portal adjacency (`portal_ref_start` / `portal_ref_count` into
`portal_refs`) is net-new compiler output — it is not serialized today. The
compiler builds transient in-memory `front_leaf` / `back_leaf` adjacency maps
for SH/light reach baking (`affinity_grid.rs`, `chunk_light_list_bake.rs`) and
the exterior flood (`crates/level-compiler/src/visibility/mod.rs:79`, `:82`,
`:83`); the runtime builds its own `leaf_portals` at load (`prl.rs`). Emit
`portal_refs` as sorted, duplicate-free per-cell ranges derived from the portal
list.

Compiler writes `Cells` from BSP leaf records plus an explicit per-leaf
`is_exterior` vector or field from the compiler's exterior-leaf classification
data after portal generation. The classification is produced by
`find_exterior_leaves(tree, portals)`
(`crates/level-compiler/src/visibility/mod.rs`). `exterior_leaves` is already
retained in the main compile pipeline and threaded into geometry and bake
inputs, but it is not passed to `pack_and_write_portals` or serialized. Extend
the pack input from `crates/level-compiler/src/main.rs:395`, `:405`, and `:737`
so `Cells` can write it. `BspLeafRecord` does not carry exterior status; the
current exterior cull zeroes face counts. Do not derive exterior status from
`face_count == 0`. Runtime loads `Cells` into a cell data type owned by
`LevelWorld`. Legacy `LeafData` may remain internally during migration, but the
preferred path must be named and treated as cells.

Cell bounds are copied from compiler BSP leaf bounds currently emitted as
`BspLeafRecord.bounds_min/max`. Those bounds are already finite because BSP
construction starts from the brush-derived world AABB with slack and tightens
regions. Compiler validation rejects non-finite or inverted leaf bounds before
writing `Cells`. Do not introduce unbounded runtime cells.

Cell flag combinations are fixed:

- `solid` and `exterior` are mutually exclusive.
- Solid cells have `face_count == 0`, `drawable == false`, and may carry bounds.
- Exterior cells are non-solid, have `face_count == 0`, and
  `drawable == false`.
- Drawable cells are non-solid, non-exterior, and have `face_count > 0`.
- Empty interior cells are non-solid, non-exterior, have `face_count == 0`, and
  `drawable == false`.

Task-owned wire contract:

```text
u32 version = 1
u32 cell_count
u32 portal_ref_total
u32 reserved = 0
CellRecord cells[cell_count]
u32 portal_refs[portal_ref_total]
```

`cell_count` must be greater than zero. Each `CellRecord` stores
`bounds_min[3]`, `bounds_max[3]`, `flags`, `face_start`, `face_count`,
`portal_ref_start`, and `portal_ref_count`. Flag bits are: bit 0 `solid`, bit 1
`exterior`, bit 2 `drawable`. Unknown bits are invalid. Section byte length
must exactly match the declared counts. Reject trailing bytes, truncated records,
and count multiplication overflow as named `Cells` validation errors. Normalize
`face_start` to `0` when `face_count == 0`. Normalize `portal_ref_start` to `0`
when `portal_ref_count == 0`.

### Task 3: Add the `CellLocator` Section

Add a `CellLocator` PRL section in `postretro-level-format`. Register
`SectionId::CellLocator = 39` and its `SectionId::from_u32` mapping in
`crates/level-format/src/lib.rs`. It answers point-to-cell lookup for
world-space positions. Version 1 may encode the same
plane decision tree as `BspNodesSection`, but it is a locator, not the runtime
BSP contract. Its children point to cell ids, not BSP leaf records.

Compiler writes the locator from the final BSP tree. Implement
`LevelWorld::locate_cell` and validate it against the old BSP leaf path. Keep
`LevelWorld::find_leaf` only as a temporary test comparison helper through Task
3. Task 4 moves production runtime callers to `locate_cell`. The implementation
must preserve current on-plane behavior: points on a split plane choose the
front child.

Add fixture probes here while both implementations exist. Probe positions must
produce the same cell ids as the old BSP leaf path. This protects Task 4, which
removes `find_leaf`. Include at least one probe positioned exactly on a split
plane to lock the front-child tie-break.

Dual-section fixture PRLs are allowed for `find_leaf` comparison only through
Task 3. They may remain load fixtures through Task 5 without `find_leaf`
comparison. They become invalid in Task 6, when ambiguous legacy-section
rejection starts.

Task-owned wire contract:

```text
u32 version = 1
u32 node_count
u32 root_kind        // 0 = cell, 1 = node
u32 root_index
CellLocatorNode nodes[node_count]
```

Each `CellLocatorNode` stores `plane_normal[3]`, `plane_distance`,
`front_kind`, `front_index`, `back_kind`, and `back_index`. Kind values are
only `0` for cell and `1` for node. Plane values must be finite, and normals
must be nonzero. Node references must be in range. Cell references must be in
range. Section byte length must exactly match `node_count`. Reject trailing
bytes, truncated records, and count multiplication overflow as named
`CellLocator` validation errors. Reject cycles and unreachable nodes.

### Task 4: Migrate Runtime Visibility Callers

Change `determine_visible_cells` to seed visibility from `locate_cell`. Change
portal traversal inputs and diagnostics to use cell terminology. Migrate mesh
and particle culling call sites that currently descend BSP for object positions
(non-visibility `find_leaf` callers:
`crates/postretro/src/render/mesh_pass.rs`,
`crates/postretro/src/scripting/systems/particle_render.rs`,
`crates/postretro/src/render/sh_diagnostics.rs`; plus
`determine_visible_cells` in `crates/postretro/src/visibility.rs`).
Keep `VisibleCells` behavior stable for the renderer: it carries drawable cell
ids or `DrawAll`. Preserve the wider fog/light reachability set as cell ids. It
includes empty non-solid cells for volume and dynamic-light gating. An empty
wider set is the all-active fallback sentinel (all canonical fog slots active;
not `VisibleCells::DrawAll`, which fog never returns).

This task owns callers in visibility, portal traversal, mesh render collection,
particle render collection, dynamic light reachability, SH diagnostics, fog
masking, spatial diagnostics, animated-lightmap visibility, candidate-cull
mirrors, and shadow-pass diagnostics. Audit with `rg` for `world.leaves`,
`LeafData`, `leaf_portals`, `leaf_index`, `VisibleCells`, and `fog_reachable`;
expected runtime files include `crates/postretro/src/render/renderer_diagnostics.rs`,
`crates/postretro/src/render/debug_ui/mod.rs`,
`crates/postretro/src/candidate_cull_mirror.rs`,
`crates/postretro/src/render/animated_lightmap.rs`, and
`crates/postretro/src/render/renderer_shadow_passes.rs`. Remove
`LevelWorld::find_leaf` after those callers migrate. It does not change portal
clipping or frustum math. While both lookup paths exist, freeze fixture expected
`VisibleCells::Culled` outputs for fixture cameras. After migration, assert the
cell-locator path still produces those frozen outputs — not just `locate_cell`
vs `find_leaf` point parity — to protect the 'portal traversal output matches
the old path' AC.

Mesh and particle object visibility remains membership-based against
`VisibleCells`. If `locate_cell(pos)` returns a cell id not in
`VisibleCells::Culled`, the object is culled. `DrawAll` remains visible. Do not
add object-specific frustum-only or fallback behavior for objects in solid,
exterior, or empty cells.

Fog masking keeps the current camera-cell union. Runtime ORs the mask for every
fog-reachable cell, then ORs the current camera cell mask. This prevents
single-frame flicker when portal traversal briefly omits the camera cell.

Map lights and runtime light bridge records become cell-semantic. Rename
`leaf_index` to `cell_index` as listed in the Boundary Inventory. All
`MapLight.leaf_index` call sites must be updated; examples include `prl.rs`,
`renderer_light_slots.rs`, and the `convert_alpha_lights` copy site. Also update
`MapLightShape.leaf_index` and the `component_to_map_light(... leaf_index)`
parameter in `crates/postretro/src/scripting/systems/light_bridge.rs`. The
compiler/wire-side `AlphaLightRecord.leaf_index` and the
`ALPHA_LIGHT_LEAF_UNASSIGNED` (= `u32::MAX`) constant (`alpha_lights.rs`) may
keep legacy names with cell-id comments — they are wire/compiler types, not
scripting typedefs. Preserve `u32::MAX` as the unassigned sentinel.
Script-spawned dynamic lights keep that sentinel until runtime cell assignment
exists for them.

### Task 5: Cross-Section Validation

Validate loaded `Cells`, `CellLocator`, `Portals`, `CellDrawIndex`, BVH leaves,
and fog masks together. Requirements:

- portal endpoints reference valid non-solid cell ids;
- portal endpoints are distinct; same-endpoint portals are fatal endpoint
  validation errors once `Portals` is decoded, nonempty, and otherwise usable;
- portal adjacency ranges match the portal list: `portal_refs` are portal
  indices into `Portals`, sorted ascending, duplicate-free per cell; each
  portal appears exactly once in each endpoint cell's adjacency list and never
  in unrelated cells;
- cell bounds are finite and `bounds_min <= bounds_max`;
- `Cells.reserved == 0`;
- cell flags contain no unknown bits;
- `solid` and `exterior` flags are mutually exclusive;
- solid cells have no faces and are not drawable;
- exterior cells are non-solid, have no faces, and are not drawable;
- drawable cells are non-solid, non-exterior, and have faces;
- empty interior cells are non-solid, non-exterior, have no faces, and are not
  drawable;
- the stored `drawable` bit equals `!solid && !exterior && face_count > 0`;
  mismatch is a named `Cells` validation error;
- `face_start + face_count` is checked for overflow and stays within
  `GeometrySection::faces` / `GeometrySection.faces.len()`;
- every BVH leaf `cell_id` references a valid drawable cell;
- `CellDrawIndex` uses CSR layout: `cell_span_offset[cell_count + 1]` plus
  spans `{ leaf_start, leaf_count }`;
- `CellDrawIndex` is emitted for non-empty BVH and omitted for empty BVH;
  present `CellDrawIndex` on a zero-leaf BVH is rejected as a named validation
  error;
- `CellDrawIndex` spans are ascending, non-overlapping, non-empty, and within
  BVH leaf bounds;
- span leaves for each cell exactly cover drawable BVH leaves whose
  `BvhLeaf.cell_id == cell_id`;
- cells with no drawable leaves have empty `CellDrawIndex` ranges;
- light spatial ids either reference valid non-solid cells or use the
  unassigned sentinel;
- fog cell masks, when present, have `cell_count` entries (one `u32` per cell,
  indexed by cell id including solid and exterior cells, matching the legacy
  per-leaf layout since cell ids preserve leaf ids one-to-one); bit `i` is
  canonical fog volume `i`. `FogCellMasks` is required when canonical fog volume
  count is greater than zero, including `fog_volume`, `fog_lamp`, and
  `fog_tube`; bits outside `all_slots_mask` are rejected as named
  `FogCellMasks` validation errors;
- locator planes are finite and have nonzero normals;
- locator roots and children use valid kind values, node indices are in range,
  cell indices are in range, traversal cannot cycle, and unused or unreachable
  nodes are rejected.

Exterior cells are valid light placements — the rule is `non-solid || sentinel`,
deliberately not `non-solid && non-exterior`. A light may legitimately sit in
exterior space: e.g. a future kinematic platform whose light starts outside the
playable area and moves in on a scripted event. It is inert there, not wrong —
its interior portal-reachable set is empty until it moves, so it cannot
over-brighten anything. This matches the runtime cell-assignment invariant for
movers (`locate_cell`, deferred to the mover work), so kinematics needs no
relaxation. Do not tighten to exclude exterior cells.

The existing `validate_cell_draw_index` (`crates/postretro/src/prl.rs:557-574`)
checks `cell_count` against `LeafData` / `leaves.len()` derived from BSP leaves;
retarget it to `Cells.cell_count`. The existing `FogCellMasks` length check
(`prl.rs`, masks length vs leaf count) likewise retargets to
`Cells.cell_count`.

Missing `Cells` / `CellLocator` sections are stale-format. Present but malformed
or invalid `Cells` / `CellLocator` sections are named validation errors.
Present `Cells` or `CellLocator` sections with unsupported `version` values are
named unsupported-version validation errors for that section. All are fatal and
name the section or sections. Exact-length, trailing-byte, truncated-record, and
count-overflow checks belong to section parsing. Cross-section mismatches belong
to loader validation. Malformed `Geometry`, `Bvh`, required `CellDrawIndex`, and
required `FogCellMasks` also fail load instead of warning and dropping data.

Portal usability is an explicit behavior change. Current code treats any
present `Portals` section as `has_portals`. Target behavior treats missing,
empty, intrinsic decode/schema failures, or intrinsically invalid `Portals` as
no usable portal graph and takes the no-portals fallback. Cross-section endpoint
or adjacency mismatches are fatal when `Portals` is otherwise usable. `Cells`
and `CellLocator` remain fatal. `Cells.portal_refs` range and overflow
validation is fatal, but portal endpoint and adjacency resolution is skipped
when `Portals` is unusable.

Portal validation matrix:

| `Portals` state | `Cells.portal_refs` validation | Endpoint/adjacency validation | Runtime state |
|---|---|---|---|
| usable | `portal_ref_start + portal_ref_count` overflow/bounds; each ref < `Portals.len()`; sorted, duplicate-free | required and fatal on mismatch, including same-endpoint portals | usable portal graph |
| missing, empty, decode/schema-failed, or intrinsically invalid | `portal_ref_start + portal_ref_count` overflow/bounds; sorted, duplicate-free; ref values accepted but unresolved | skipped | no usable portal graph fallback |

This task does not reject coexisting legacy `BspNodes` / `BspLeaves` plus
modern sections; ambiguous-section rejection lands in Task 6. Task 5 fixtures
may carry both section sets.

The minimum modern load contract is: `Cells`, `CellLocator`, `Geometry`, and
`Bvh` must be valid; `CellDrawIndex` must be valid when the BVH has leaves;
`FogCellMasks` must be valid when canonical fog volume count is greater than
zero. Missing `Geometry` or `Bvh` is stale-format. Present but malformed or
invalid `Geometry` or `Bvh` is a named section validation error.

### Task 6: Stop Emitting Runtime BSP Sections

Once preferred-path tests pass, stop packing `BspNodes` and `BspLeaves` into
PRL output. Remove runtime tests that expect modern PRLs to decode those
sections, and add stale-format tests for PRLs missing `Cells` or `CellLocator`.
Modern PRLs load with `Cells + CellLocator` and no runtime BSP sections. Modern
PRLs that also contain `BspNodes` or `BspLeaves` are rejected with a named
ambiguous/stale-format error.

This task must not change compiler BSP construction. It only changes what the
compiler writes into PRL and what the runtime accepts from PRL. Remove runtime
`BspNodes` / `BspLeaves` decoding and storage from `LevelWorld`; any remaining
BSP section helpers must be compiler-side or migration-test-only.

### Task 7: Diagnostics and Documentation

Update dev-tool overlays to label cells, portals, and BVH leaves without BSP
terminology. Update the Spatial diagnostics tab and backing renderer diagnostics
data in `crates/postretro/src/render/debug_ui/mod.rs`,
`crates/postretro/src/render/renderer_debug_ui.rs`, and
`crates/postretro/src/render/renderer_diagnostics.rs`. Show labeled fields for
all five items the Cell-diagnostics AC requires: current cell,
portal-reachable drawable cells, fog-reachable cells, locator status (descent
path/result), and candidate BVH leaf counts. Task 3 or Task 4 must expose a
traceable locator API or companion diagnostic call so this task can display the
descent path without reimplementing locator traversal.

Task 7 updates `context/lib/build_pipeline.md`,
`context/lib/rendering_pipeline.md`, and `context/lib/entity_model.md` after
behavior lands. Do not edit durable docs while this plan is still being
drafted/reviewed. The durable docs should state: BSP is compile-only
scaffolding; runtime visibility uses cells and portals; collision uses
`CollisionWorld`. The `build_pipeline.md` PRL section-id registry gains rows for
`Cells` (38) and `CellLocator` (39) and drops `BspNodes`/`BspLeaves` from the
runtime-emitted set.

## Sequencing

**Phase 1 (sequential):** Task 1 — creates safe extension seams in oversized
files.

**Phase 2 (sequential):** Task 2, then Task 3 — `CellLocator` references cell
ids from `Cells`.

**Phase 3 (sequential):** Task 4 — migrates runtime callers to the new cell
locator and terminology.

**Phase 4 (sequential):** Task 5 — validates all loaded spatial sections after
the preferred runtime path exists.

**Phase 5 (sequential):** Task 6 — stops emitting runtime BSP sections and
rejects stale PRLs.

**Phase 6 (sequential):** Task 7 — updates diagnostics and durable docs after
behavior is stable.

## Rough Sketch

Old runtime shape:

```text
LevelWorld::find_leaf(position)
  -> BspNodesSection / BspLeavesSection-derived data
  -> portal traversal
  -> VisibleCells
  -> BVH / candidate cull / mesh / particle consumers
```

Target runtime shape:

```text
LevelWorld::locate_cell(position)
  -> CellLocator
  -> portal traversal over Cells
  -> VisibleCells
  -> CellDrawIndex + BVH candidate cull
```

Collision remains separate:

```text
PRL vertices + indices
  -> CollisionWorld::populate_from_level
  -> parry3d::shape::TriMesh
  -> cast_capsule / cast_ray
```

The first `CellLocator` can be structurally identical to the compiler BSP node
descent. That makes the migration low risk. The important change is ownership:
runtime systems ask for a cell id. They do not depend on BSP as the semantic
world model. Later plans can replace the locator with a grid, cached-neighbor
walk, or hybrid without changing visibility callers.

Do not use the BVH as a replacement for portal visibility. BVH stays an
accelerator for draw leaves and ray-like workloads. Godot's occlusion model is
the caution here: mutable occluder geometry plus BVH rebuild cost is the wrong
shape for this engine. Prefer dynamic portal state over mutable occluder BVHs.

## Boundary Inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `Cells` section | `SectionId::Cells` | PRL section id 38 | n/a | n/a | n/a |
| `CellLocator` section | `SectionId::CellLocator` | PRL section id 39 | n/a | n/a | n/a |
| Visible cells | `VisibleCells` | n/a | n/a | n/a | n/a |
| Runtime locator | `LevelWorld::locate_cell` | n/a | n/a | n/a | n/a |
| Portal endpoints | rename runtime locals and helpers to `front_cell` / `back_cell`; keep compiler `Portal.front_leaf` / `back_leaf` while BSP owns portal generation, with comments that emitted runtime records carry cell ids | runtime `PortalRecord` keeps its already-present `front_leaf` / `back_leaf` fields, documented as cell ids; rename only runtime locals and helpers to `front_cell` / `back_cell` | n/a | n/a | n/a |
| Light spatial id | rename runtime bridge/storage fields from `leaf_index` to `cell_index`; keep generated or catalog-facing legacy names only when type churn crosses the scripting typedef boundary, with comments that values are cell ids or `u32::MAX` | light cell id or `u32::MAX` sentinel | n/a | n/a | n/a |

## Wire Format

All new PRL section fields are little-endian. `Cells` uses section id 38.
`CellLocator` uses section id 39. These are follow-on ids after
`CellDrawIndex = 37` from `perf-visible-cell-candidate-cull`.

### `Cells`

```text
u32 version = 1
u32 cell_count
u32 portal_ref_total
u32 reserved = 0
CellRecord cells[cell_count]
u32 portal_refs[portal_ref_total]
```

Modern PRLs require `cell_count > 0`.

`CellRecord` fields:

```text
f32 bounds_min[3]
f32 bounds_max[3]
u32 flags
u32 face_start
u32 face_count
u32 portal_ref_start
u32 portal_ref_count
```

Flags:

- bit 0: solid;
- bit 1: exterior;
- bit 2: drawable.

`face_start` and `face_count` reference the `GeometrySection::faces` index
space, checked against `GeometrySection.faces.len()`, matching the legacy leaf
face range. They are diagnostic and visibility-fallback metadata; draw
submission uses `CellDrawIndex`. `drawable` must equal
`!solid && !exterior && face_count > 0` for v1. Store the bit anyway so future
non-face drawable cell classes can be additive. Solid and exterior cells are
never drawable. The exterior bit comes from explicit exterior classification
data, not from zero `face_count`. When copying legacy `BspLeafRecord` face
ranges into `Cells`, normalize `face_start` to `0` for zero-face cells instead
of preserving a nonzero legacy `BspLeafRecord.face_start`.

The section byte length must exactly match `cell_count` and `portal_ref_total`.
Reject trailing bytes, truncated records, and count multiplication overflow as
named `Cells` validation errors.

Empty lists use zero counts and no sentinel. Per-cell `portal_ref_start +
portal_ref_count` must be checked for overflow and bounds against
`portal_ref_total` (the `portal_refs` array length); each value stored in
`portal_refs` is bounds-checked against `Portals.len()` when `Portals` is
usable. When `face_count` is
zero, `face_start` must be `0`. When `portal_ref_count` is zero,
`portal_ref_start` must be `0`. `portal_refs` are portal indices into the
`Portals` section. They are sorted ascending and duplicate-free for each cell.
Each portal index appears exactly once in each of its two endpoint cells and in
no other cell when `Portals` is usable. If `Portals` is unusable, endpoint and
adjacency resolution is skipped and the loader records no usable portal graph.

Validation rejects nonzero `reserved`, unknown flag bits, non-finite bounds,
`bounds_min > bounds_max`, invalid flag combinations, `cell_count == 0`, and
overflowing or out-of-range `face_start + face_count`. Missing section is
stale-format. Present section with unsupported `version` is a named `Cells`
validation error.

### `CellLocator`

```text
u32 version = 1
u32 node_count
u32 root_kind        // 0 = cell, 1 = node
u32 root_index
CellLocatorNode nodes[node_count]
```

`CellLocatorNode` fields:

```text
f32 plane_normal[3]
f32 plane_distance
u32 front_kind       // 0 = cell, 1 = node
u32 front_index
u32 back_kind        // 0 = cell, 1 = node
u32 back_index
```

Node traversal uses the existing rule: `dot(position, normal) - distance >= 0`
selects front. Locator planes must be finite and have nonzero normals.
`root_kind`, `front_kind`, and `back_kind` accept only `0` or `1`. Node
references must be in range. Cell references must be in range. The loader
rejects cycles and unreachable nodes. Missing section is stale-format. Present
section with unsupported `version` is a named `CellLocator` validation error.
The section byte length must exactly match `node_count`; reject trailing bytes,
truncated records, and count multiplication overflow as named `CellLocator`
validation errors.

## Open Questions

None. Dynamic portal policy bits are a follow-up feature; this plan only makes
cells the runtime contract.
