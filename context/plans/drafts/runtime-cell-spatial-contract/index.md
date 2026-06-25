# Runtime Cell Spatial Contract

## Goal

Replace runtime BSP as a loaded spatial contract with explicit cells and a
cell locator. Keep BSP in the compiler pipeline where it discovers solid and
empty space. Runtime systems consume cells, portals, cell draw data, BVH draw
accelerators, and the existing collision trimesh.

This is a follow-on to `context/plans/ready/perf-visible-cell-candidate-cull/`.
It assumes the visible-cell candidate cull path and `CellDrawIndex` section
exist.

## Scope

### In scope

- Add runtime `Cells` and `CellLocator` PRL sections.
- Keep existing `BspNodes` / `BspLeaves` load fallback for legacy PRLs.
- Rename runtime APIs and data flow around cells: `find_leaf` becomes
  `locate_cell`, `VisibleCells` becomes cell-semantic, and callers stop treating
  BSP leaf records as the durable runtime type.
- Keep portal traversal as the sole visibility path.
- Keep BVH as a draw, light, shadow, and bake accelerator. Do not make it the
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

## Acceptance criteria

- [ ] New PRLs contain `Cells` and `CellLocator` sections. `BspNodes` and
      `BspLeaves` may still be emitted for transition builds, but runtime does
      not require them when `Cells + CellLocator` are valid.
- [ ] Legacy PRLs without the new sections still load through the existing
      `BspNodes` / `BspLeaves` path and preserve current visibility, mesh, and
      particle culling behavior.
- [ ] Runtime camera visibility no longer calls `find_leaf` on the preferred
      path. It calls a cell locator, then portal traversal starts from that
      cell id.
- [ ] Runtime mesh and particle visibility call the same point-to-cell locator
      used by camera visibility. Particles remain culled by each particle's own
      position, not by their emitter's cell.
- [ ] `VisibleCells::Culled` continues to carry cell ids. `DrawAll` fallback
      behavior remains unchanged.
- [ ] Portal traversal output for current fixture maps matches the old BSP-leaf
      path when cell ids equal legacy leaf ids.
- [ ] Candidate cull consumes cell ids from portal traversal and
      `CellDrawIndex`; it does not walk runtime BSP nodes.
- [ ] BVH data remains loaded and uploaded for world draw, shadow cone cull,
      diagnostics, and compile-time ray/bake consumers. The spec does not
      collapse visibility into BVH traversal.
- [ ] `CollisionWorld::populate_from_level`, `cast_capsule`, and `cast_ray`
      keep using PRL static geometry through `parry3d` queries. No collision AC
      depends on BSP or the new cell locator.
- [ ] Cell diagnostics show current cell, portal-reachable drawable cells,
      fog/light-reachable cells, locator path, and candidate leaf counts.
- [ ] Loader validation rejects malformed new sections with a warning and falls
      back to legacy BSP sections when present.
- [ ] Loader validation fails the PRL load only when neither the new cell path
      nor the legacy BSP path can provide a point-to-cell locator.
- [ ] `cargo test -p postretro-level-format`, `-p postretro-level-compiler`,
      `-p postretro`, and `cargo check -p postretro --features dev-tools` pass.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Split PRL Load and Pack Seams

`crates/postretro/src/prl.rs` and `crates/level-compiler/src/pack.rs` are both
past the split-before-extend threshold. Split before adding behavior. Extract
PRL runtime BSP/cell decoding helpers from `prl.rs` into a focused loader
module. Extract optional-section packing helpers from `pack.rs` into a focused
packing module. Keep public behavior unchanged. Existing tests for
`LevelWorld`, `load_prl`, and `pack_and_write_portals` must pass unchanged
before later tasks extend these seams.

### Task 2: Add the `Cells` Section

Add a `Cells` PRL section in `postretro-level-format`. A cell is the runtime
visibility unit that currently corresponds one-to-one with an empty or solid
BSP leaf. The section stores enough runtime information to replace
`BspLeavesSection` for visibility and diagnostics: bounds, solidity, exterior
status, drawable face count, and portal adjacency range. It does not store BSP
split planes.

Compiler writes `Cells` from the BSP leaf records after portal generation and
exterior classification. Runtime loads `Cells` into a cell data type owned by
`LevelWorld`. Legacy `LeafData` may remain internally during migration, but the
preferred path must be named and treated as cells.

### Task 3: Add the `CellLocator` Section

Add a `CellLocator` PRL section in `postretro-level-format`. It answers
point-to-cell lookup for world-space positions. Version 1 may encode the same
plane decision tree as `BspNodesSection`, but it is a locator, not the runtime
BSP contract. Its children point to cell ids, not BSP leaf records.

Compiler writes the locator from the final BSP tree. Runtime loads it as the
preferred point-to-cell query source. Implement `LevelWorld::locate_cell` and
make `find_leaf` a legacy wrapper or remove it once call sites are migrated.
The implementation must preserve current on-plane behavior: points on a split
plane choose the front child.

### Task 4: Migrate Runtime Visibility Callers

Change `determine_visible_cells` to seed visibility from `locate_cell`. Change
portal traversal inputs and diagnostics to use cell terminology. Migrate mesh
and particle culling call sites that currently descend BSP for object positions.
Keep `VisibleCells` behavior stable for the renderer and scripts: it carries
drawable cell ids or `DrawAll`.

This task owns callers in visibility, portal traversal, mesh render collection,
particle render collection, and diagnostics. It does not change portal clipping
or frustum math.

### Task 5: Cross-Section Validation

Validate loaded `Cells`, `CellLocator`, `Portals`, `CellDrawIndex`, BVH leaves,
and fog masks together. Requirements:

- portal endpoints reference valid non-solid cell ids;
- portal adjacency ranges match the portal list;
- every BVH leaf `cell_id` references a valid drawable cell;
- `CellDrawIndex` spans cover exactly drawable BVH leaves, as defined by cell
  metadata;
- fog cell masks, when present, have one entry per cell;
- locator leaves reference valid cells;
- legacy BSP fallback produces the same cell ids on fixture probes while both
  paths are present.

Invalid new cell sections warn and fall back to legacy BSP sections if those
sections exist. Missing or invalid locator data is fatal only when no legacy
fallback exists.

### Task 6: Remove Runtime BSP Requirement

Once preferred-path tests pass, make `BspNodes` and `BspLeaves` optional for
runtime load. Keep compiler emission during the transition unless a separate
format-cleanup plan removes it. Update tests so modern PRLs load with
`Cells + CellLocator` and no BSP sections.

This task must not change compiler BSP construction. It only changes what the
runtime requires from the PRL.

### Task 7: Diagnostics and Documentation

Update dev-tool overlays to label cells, portals, and BVH leaves without BSP
terminology on the preferred path. Keep legacy labels only where the fallback is
actually active. Add diagnostics that show whether the current frame used
`CellLocator` or legacy BSP lookup.

Update `context/lib/build_pipeline.md`, `context/lib/rendering_pipeline.md`,
and `context/lib/entity_model.md` at promotion time, not during the draft. The
durable docs should state: BSP is compile-only scaffolding; runtime visibility
uses cells and portals; collision uses `CollisionWorld`.

## Sequencing

**Phase 1 (sequential):** Task 1 — creates safe extension seams in oversized
files.

**Phase 2 (sequential):** Task 2, then Task 3 — `CellLocator` references cell
ids from `Cells`.

**Phase 3 (sequential):** Task 4 — migrates runtime callers to the new cell
locator and terminology.

**Phase 4 (sequential):** Task 5 — validates all loaded spatial sections after
the preferred runtime path exists.

**Phase 5 (sequential):** Task 6 — removes runtime dependence on BSP sections.

**Phase 6 (sequential):** Task 7 — updates diagnostics and durable docs after
behavior is stable.

## Rough Sketch

Current runtime shape:

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

The first `CellLocator` can be structurally identical to the current BSP node
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
| Legacy locator | `LevelWorld::find_leaf` | `BspNodes` / `BspLeaves` | n/a | n/a | n/a |

## Wire Format

All new PRL section fields are little-endian. `Cells` uses section id 38.
`CellLocator` uses section id 39. This follows `CellDrawIndex = 37` from
`perf-visible-cell-candidate-cull`.

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

`drawable` must equal `!solid && face_count > 0` for v1. Store the bit anyway
so future non-face drawable cell classes can be additive.

Empty lists use zero counts and no sentinel. `portal_ref_start +
portal_ref_count` must be checked for overflow and bounds.

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
selects front.

## Open Questions

- Should `BspNodes` / `BspLeaves` stop being emitted in the same implementation,
  or should this plan only make runtime no longer require them? The draft
  chooses runtime independence first.
- Should `find_leaf` remain as a deprecated compatibility wrapper for one
  release, or should the migration remove it outright after all call sites move?
- Should dynamic portal policy bits live in this plan or a follow-up? This
  draft leaves policy state for a later feature; it only makes cells the runtime
  contract.
