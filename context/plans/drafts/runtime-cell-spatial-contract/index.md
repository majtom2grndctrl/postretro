# Runtime Cell Spatial Contract

## Goal

Replace runtime BSP as a loaded spatial contract with explicit cells and a
cell locator. Keep BSP in the compiler pipeline where it discovers solid and
empty space. Runtime systems consume cells, portals, cell draw data, BVH draw
accelerators, and the existing collision trimesh.

This is a follow-on to `context/plans/ready/perf-visible-cell-candidate-cull/`.
It depends on that plan landing first. `CellDrawIndex = 37` is not in current
source on this checkout.

## Scope

### In scope

- Add runtime `Cells` and `CellLocator` PRL sections.
- Stop emitting runtime `BspNodes` / `BspLeaves` sections. BSP remains a
  compiler intermediate only.
- Rename runtime APIs and data flow around cells: `find_leaf` becomes
  `locate_cell`, `VisibleCells` becomes cell-semantic, and callers stop
  treating BSP leaf records as the durable runtime type.
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
- Replacing `CollisionWorld`, `parry3d::TriMesh`, capsule sweeps, or hitscan
  world rays.
- Dynamic moving occluder geometry or per-frame BVH rebuilds.
- Baked PVS.
- Changing `CellDrawIndex` layout from the candidate-cull plan, except where
  this plan validates it against `Cells`.
- Runtime level compilation.
- General-purpose Godot-style scene-node visibility.
- Legacy PRL compatibility. Maps are rebuilt often during development.

## Acceptance criteria

- [ ] New PRLs contain `Cells` and `CellLocator` sections and do not contain
      runtime `BspNodes` or `BspLeaves` sections.
- [ ] PRLs missing `Cells` or `CellLocator` fail to load with a named
      stale-format error. Present but malformed `Cells` or `CellLocator`
      sections fail with named validation errors. The runtime no longer
      synthesizes a fallback leaf for missing BSP data.
- [ ] Modern PRLs with valid `Cells` / `CellLocator` plus legacy runtime
      `BspNodes` or `BspLeaves` fail with a named ambiguous/stale-format error.
- [ ] Runtime camera visibility no longer calls `find_leaf`. It calls
      `locate_cell`, then portal traversal starts from that cell id.
- [ ] Runtime mesh and particle visibility call the same point-to-cell locator
      used by camera visibility. Particles remain culled by each particle's own
      position, not by their emitter's cell.
- [ ] `VisibleCells::Culled` continues to carry cell ids. `DrawAll` fallback
      behavior remains unchanged.
- [ ] Cell ids preserve legacy BSP leaf ids. Solid, exterior, empty, and
      drawable leaves all have cell records. Portal traversal output for current
      fixture maps matches the old BSP-leaf path because ids are unchanged.
- [ ] Candidate cull consumes cell ids from portal traversal and
      `CellDrawIndex`; it does not walk runtime BSP nodes.
- [ ] Runtime BVH data remains loaded and uploaded for world draw, shadow cone
      cull, and diagnostics. The spec does not collapse visibility into BVH
      traversal.
- [ ] `CollisionWorld::populate_from_level`, `cast_capsule`, and `cast_ray`
      keep using PRL static geometry through `parry3d` queries. No collision AC
      depends on BSP or the new cell locator.
- [ ] Cell diagnostics show current cell, portal-reachable drawable cells,
      fog-reachable cells, locator path, and candidate BVH leaf counts.
- [ ] Loader validation rejects malformed `Cells` / `CellLocator` sections as
      named load errors. Missing `Cells` / `CellLocator` is stale-format.
      There is no legacy BSP fallback.
- [ ] Preferred-path validation covers `Cells`, `CellLocator`, `Portals`, BVH
      leaf cell ids, `CellDrawIndex` for non-empty BVH maps, and `FogCellMasks`
      when canonical fog volume count is greater than zero.
- [ ] Solid-cell camera positions use the existing solid-camera fallback:
      frustum-cull all drawable cells by bounds and skip portal traversal.
      Exterior-cell camera positions use the existing exterior-camera fallback.
      Missing, empty, decode-failed, or validation-rejected `Portals` sections
      mean no usable portal graph and use the no-portals fallback.
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

Add a `Cells` PRL section in `postretro-level-format`. A cell is the runtime
visibility unit and preserves the compiler BSP leaf id space one-to-one:
solid, exterior, empty, and drawable leaves all become cells. This avoids a
remap for portals, BVH leaf `cell_id`, fog masks, diagnostics, and tests. The
section stores enough runtime information to replace `BspLeavesSection` for
visibility and diagnostics: bounds, solidity, exterior status, geometry face
range summary, and portal adjacency range. It does not store BSP split planes.

Compiler writes `Cells` from BSP leaf records plus an explicit per-leaf
`is_exterior` vector or field from the compiler's exterior-leaf classification
data after portal generation. `BspLeafRecord` does not carry exterior status;
the current exterior cull zeroes face counts. Do not derive exterior status
from `face_count == 0`. Runtime loads `Cells` into a cell data type owned by
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

### Task 3: Add the `CellLocator` Section

Add a `CellLocator` PRL section in `postretro-level-format`. It answers
point-to-cell lookup for world-space positions. Version 1 may encode the same
plane decision tree as `BspNodesSection`, but it is a locator, not the runtime
BSP contract. Its children point to cell ids, not BSP leaf records.

Compiler writes the locator from the final BSP tree. Runtime loads it as the
only point-to-cell query source. Implement `LevelWorld::locate_cell`. Keep
`LevelWorld::find_leaf` only as a temporary comparison/test helper until
callers migrate. The implementation must preserve current on-plane behavior:
points on a split plane choose the front child.

Add fixture probes here while both implementations exist. Probe positions must
produce the same cell ids as the old BSP leaf path. This protects Task 4, which
removes `find_leaf`.

### Task 4: Migrate Runtime Visibility Callers

Change `determine_visible_cells` to seed visibility from `locate_cell`. Change
portal traversal inputs and diagnostics to use cell terminology. Migrate mesh
and particle culling call sites that currently descend BSP for object positions.
Keep `VisibleCells` behavior stable for the renderer and scripts: it carries
drawable cell ids or `DrawAll`.

This task owns callers in visibility, portal traversal, mesh render collection,
particle render collection, SH diagnostics, fog masking, and spatial
diagnostics. Remove `LevelWorld::find_leaf` after those callers migrate. It
does not change portal clipping or frustum math.

Mesh and particle object visibility remains membership-based against
`VisibleCells`. If `locate_cell(pos)` returns a cell id not in
`VisibleCells::Culled`, the object is culled. `DrawAll` remains visible. Do not
add object-specific frustum-only or fallback behavior for objects in solid,
exterior, or empty cells.

### Task 5: Cross-Section Validation

Validate loaded `Cells`, `CellLocator`, `Portals`, `CellDrawIndex`, BVH leaves,
and fog masks together. Requirements:

- portal endpoints reference valid non-solid cell ids;
- portal endpoints are distinct;
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
- `face_start + face_count` is checked for overflow and stays within
  `Geometry.face_meta`;
- every BVH leaf `cell_id` references a valid drawable cell;
- `CellDrawIndex` uses CSR layout: `cell_span_offset[cell_count + 1]` plus
  spans `{ leaf_start, leaf_count }`;
- `CellDrawIndex` is emitted for non-empty BVH and omitted for empty BVH;
- `CellDrawIndex` spans are ascending, non-overlapping, non-empty, and within
  BVH leaf bounds;
- span leaves for each cell exactly cover drawable BVH leaves whose
  `BvhLeaf.cell_id == cell_id`;
- cells with no drawable leaves have empty `CellDrawIndex` ranges;
- fog cell masks, when present, have one entry per cell; `FogCellMasks` is
  required when canonical fog volume count is greater than zero, including
  `fog_volume`, `fog_lamp`, and `fog_tube`;
- locator planes are finite and have nonzero normals;
- locator roots and children use valid kind values, node indices are in range,
  cell indices are in range, child ranges are checked, traversal cannot cycle,
  and unused or unreachable nodes are rejected.

Missing `Cells` / `CellLocator` sections are stale-format. Present but malformed
or invalid `Cells` / `CellLocator` sections are named validation errors. Both
are fatal and name the section or sections.

Portal usability is an explicit behavior change. Current code treats any
present `Portals` section as `has_portals`. Target behavior treats missing,
empty, decode-failed, or validation-rejected `Portals` as no usable portal
graph and takes the no-portals fallback. `Cells` and `CellLocator` remain
fatal. `Cells.portal_refs` range and overflow validation is fatal, but portal
endpoint and adjacency resolution is skipped when `Portals` is unusable.

The minimum modern load contract is: `Cells`, `CellLocator`, `Geometry`, and
`Bvh` must be valid; `CellDrawIndex` must be valid when the BVH has leaves;
`FogCellMasks` must be valid when canonical fog volume count is greater than
zero.

### Task 6: Stop Emitting Runtime BSP Sections

Once preferred-path tests pass, stop packing `BspNodes` and `BspLeaves` into
PRL output. Remove runtime tests that expect modern PRLs to decode those
sections, and add stale-format tests for PRLs missing `Cells` or `CellLocator`.
Modern PRLs load with `Cells + CellLocator` and no runtime BSP sections. Modern
PRLs that also contain `BspNodes` or `BspLeaves` are rejected with a named
ambiguous/stale-format error.

This task must not change compiler BSP construction. It only changes what the
compiler writes into PRL and what the runtime accepts from PRL.

### Task 7: Diagnostics and Documentation

Update dev-tool overlays to label cells, portals, and BVH leaves without BSP
terminology. Add diagnostics that show current locator status, plus
fog-reachable and portal-reachable cells.

Task 7 updates `context/lib/build_pipeline.md`,
`context/lib/rendering_pipeline.md`, and `context/lib/entity_model.md` after
behavior lands. Do not edit durable docs while this plan is still being
drafted/reviewed. The durable docs should state: BSP is compile-only
scaffolding; runtime visibility uses cells and portals; collision uses
`CollisionWorld`.

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
  -> parry3d TriMesh
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

## Wire Format

All new PRL section fields are little-endian. `Cells` uses section id 38.
`CellLocator` uses section id 39. These are follow-on ids after
`CellDrawIndex = 37` from `perf-visible-cell-candidate-cull`, which must land
first.

### `Cells`

```text
u32 version = 1
u32 cell_count
u32 portal_ref_count
u32 reserved = 0
CellRecord cells[cell_count]
u32 portal_refs[portal_ref_count]
```

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

`face_start` and `face_count` reference the `Geometry` section's `face_meta`
index space, matching the legacy leaf face range. They are diagnostic and
visibility-fallback metadata; draw submission uses `CellDrawIndex`. `drawable`
must equal `!solid && !exterior && face_count > 0` for v1. Store the bit anyway
so future non-face drawable cell classes can be additive. Solid and exterior
cells are never drawable. The exterior bit comes from explicit exterior
classification data, not from zero `face_count`.

Empty lists use zero counts and no sentinel. `portal_ref_start +
portal_ref_count` must be checked for overflow and bounds. `portal_refs` are
portal indices into the `Portals` section. They are sorted ascending and
duplicate-free for each cell. Each portal index appears exactly once in each of
its two endpoint cells and in no other cell.

Validation rejects nonzero `reserved`, unknown flag bits, non-finite bounds,
`bounds_min > bounds_max`, invalid flag combinations, and overflowing or
out-of-range `face_start + face_count`.

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
rejects cycles and unreachable nodes.

## Open Questions

None. Dynamic portal policy bits are a follow-up feature; this plan only makes
cells the runtime contract.
