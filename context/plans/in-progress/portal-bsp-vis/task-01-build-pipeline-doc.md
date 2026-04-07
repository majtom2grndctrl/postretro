# Task 01: Build Pipeline Doc Update

**Files:** `context/lib/build_pipeline.md` · `context/lib/index.md`
**Depends on:** Task 00

---

## Context

`context/lib/build_pipeline.md` describes the PRL compiler pipeline as voxel-based:
> parse .map → voxelize brushes → spatial grid → PVS (ray-cast) → geometry → pack .prl

This is no longer accurate. The pipeline is being replaced with portal-based BSP vis. Agents working on Tasks 02–06 will load `build_pipeline.md` for architectural guidance — it must reflect the new pipeline before those tasks begin.

`context/lib/index.md` §3 also describes the PRL path as "no BSP tree" and "voxel grid for spatial queries" — both are now wrong.

---

## What to Change

### `context/lib/build_pipeline.md` — §PRL Compilation

Replace the entire §PRL Compilation section with a description of the new pipeline:

**New pipeline:**
```
parse .map → BSP compilation → portal generation → portal vis → geometry → pack .prl
```

**Stage descriptions to write:**

1. **Parse.** Shambler extracts brush volumes, faces, and entities. Coordinate transform (Quake Z-up → engine Y-up) applied immediately at parse boundary. All downstream stages receive engine-native coordinates.

2. **BSP compilation.** Build BSP tree from world faces. Produces interior nodes (splitting planes) and leaves (convex regions). Leaves classified solid or empty via brush half-plane test. Solid leaves represent brush interiors. Empty leaves represent navigable space.

3. **Portal generation.** For each BSP internal node, clip the splitting-plane polygon against ancestor splitting planes to produce the portal polygon bounding that node's partition. Each portal is a convex polygon connecting two adjacent empty leaves. Portals are compile-time only — not stored in .prl.

4. **Portal vis.** Per empty leaf, flood through the portal graph. A leaf L' is potentially visible from L if any sequence of portals connects them. Output: per-leaf PVS bitsets, RLE-compressed. Computed in parallel (one task per leaf).

5. **Geometry.** Fan-triangulate faces into vertex/index buffers. Faces grouped by leaf index for efficient per-leaf draw calls.

6. **Pack.** Write BSP tree nodes, BSP leaves (face ranges, bounds, PVS references), leaf PVS bitsets, and geometry to the .prl binary format.

**Key differences from former voxel approach** (include this in the doc):
- No voxel grid. Solid/empty classification uses brush half-plane geometry directly.
- Leaf-based PVS replaces cluster-based PVS. BSP leaves are the visibility units.
- BSP tree stored in .prl — enables O(log n) point-in-leaf at runtime.
- Portals are compile-time intermediate data; they are not stored in .prl.

Remove the §PRL Compilation "Compiler pipeline" diagram and replace with the new one above. Remove all references to voxels, spatial grid, clusters, and the `voxel_grid.rs` / `spatial_grid.rs` files.

Also remove the line referencing `plans/prl-spec-draft.md` — that draft is superseded by this plan and the implemented format.

### `context/lib/index.md` — §3 Baked Data Strategy

Update the paragraph describing the PRL path. Currently:
> The PRL format replaces per-leaf BSP visibility with cluster-based PVS, stores geometry in engine-native coordinates, and is designed to subsume the baked data currently provided by BSPX lumps. The compiler uses a voxel grid for solid/empty classification and ray-cast visibility — no BSP tree.

Replace with accurate description:
> The PRL format stores BSP tree, per-leaf PVS, and geometry in engine-native coordinates. The compiler builds a BSP tree from brush geometry, extracts portals at each splitting plane, and computes PVS by flooding through the portal graph. Designed to subsume baked data currently provided by BSPX lumps.

Also update the Agent Router entry for PRL:

Old:
> **PRL format / level compiler / clusters / PVS** → `plans/prl-spec-draft.md` · `build_pipeline.md` §PRL

New:
> **PRL format / level compiler / BSP / PVS** → `build_pipeline.md` §PRL

---

## Acceptance Criteria

- `context/lib/build_pipeline.md` §PRL Compilation describes the new portal-based pipeline with correct stage names and contracts.
- No mention of voxel grid, spatial grid, or clusters in the PRL section.
- `context/lib/index.md` §3 accurately describes the PRL path.
- Agent Router entry for PRL points to `build_pipeline.md` only (no `prl-spec-draft.md` reference).
- Both files remain in the style defined by `context/lib/context_style_guide.md`: active voice, short sentences, no function names or struct fields as load-bearing detail.
- `cargo check` passes (doc task, but verify compiler still builds).
