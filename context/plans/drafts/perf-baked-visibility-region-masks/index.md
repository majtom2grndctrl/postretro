# Baked Visibility Regions BVH Masks

## Goal

Move static-world visibility work out of the frame loop without returning to a
BSP renderer. The compiler derives coarse visibility regions from BSP cells and
portals, bakes conservative region PVS, and attaches region masks to the global
BVH so the GPU can skip impossible subtrees early.

## Scope

### In scope

- Add an optional PRL visibility-region section.
- Derive coarse regions automatically from non-solid, non-exterior BSP leaves.
- Compute conservative all-open region PVS from compiler portal geometry.
- Bake one region mask per flat BVH node.
- Add a runtime accelerated path that maps camera leaf to region, looks up the
  baked region PVS, and derives both `VisibleCells` and a visible-region mask
  without running the per-frame portal DFS.
- Keep the existing runtime portal traversal as fallback and debug comparison.
- Keep the global BVH, material-bucket leaf ordering, and indirect draw layout.
- Add GPU BVH node-mask early-out for camera cull.
- Clear camera indirect slots before cull so skipped subtrees cannot retain
  stale draw commands.
- Preserve shadow cull correctness with all-regions visible for shadow passes.
- Add CPU-only tests and diagnostics for PVS conservatism, node-mask usefulness,
  and fallback behavior.

### Out of scope

- CPU-built compact draw plans.
- Per-region BVHs.
- Changing the BVH SAH split heuristic. This plan measures node-mask saturation
  so a follow-up can decide whether region-aware BVH building is worthwhile.
- Stateful visibility gates for doors, lowering walls, or destructible
  occluders. The baked PVS assumes revealable blockers are open. Later gate
  masks may only narrow this all-open set while a gate is known closed.
- Kinematic brush authoring or runtime movement.
- Raw Quake-style BSP leaf PVS as the runtime render contract.
- GPU timing as an acceptance gate. GPU timing may be collected manually, but
  automated acceptance stays CPU-visible.

## Acceptance criteria

- [ ] PRLs with a `VisibilityRegions` section load and use the baked-region
      camera path; PRLs without the section load and use the current portal
      traversal path.
- [ ] The baked-region path never produces fewer drawable cells than current
      runtime portal traversal for committed visibility fixtures and
      deterministic `stress-warren` camera points: spawn plus centers of the
      first, median, and last drawable leaves.
- [ ] A connected multi-room fixture proves the compiler PVS is a proper subset
      of all regions. Raw graph reachability for every connected region is not
      accepted as the PVS algorithm.
- [ ] CPU mirror coverage proves BVH node-mask early-out produces the same final
      leaf submissions as leaf-level visible-cell plus frustum tests for the
      same camera inputs.
- [ ] CPU mirror coverage proves at least one fixture skips an internal BVH
      subtree before reaching its leaves.
- [ ] Camera indirect slots are cleared before camera cull. A regression test
      or CPU mirror proves a leaf skipped by an internal-node reject cannot
      retain a previous visible draw command.
- [ ] Shadow cull binds all regions visible and all cells visible, and its CPU
      mirror result matches the current cone-only cull result.
- [ ] Compiler or offline diagnostics report region count, region word count,
      average and max visible-region count, BVH node-mask saturation, and
      estimated masked-traversal node visits for `stress-warren.map` and
      `stress-warren-crates.map` without requiring lightmap or SH bakes.
- [ ] `prl-build --visibility-regions off` omits the section and produces the
      legacy visibility path.
- [ ] `cargo test -p postretro-level-format`,
      `cargo test -p postretro-level-compiler`, and `cargo test -p postretro`
      pass.
- [ ] No new `unsafe`.
- [ ] After implementation, `context/lib/build_pipeline.md` and
      `context/lib/rendering_pipeline.md` describe visibility regions, fallback
      loading, camera cull flow, shadow all-visible behavior, and the all-open
      rule for future stateful occluders.

## Tasks

### Task 1: Split Oversized PRL Seams

Before adding new behavior, make behavior-preserving splits around the files
this plan would otherwise extend heavily. Move BVH conversion and related
loader tests out of `crates/postretro/src/prl.rs` into a focused runtime loader
module. Move PRL section assembly helpers out of the oversized
`crates/level-compiler/src/pack.rs` path enough that adding one optional section
does not deepen the monolith. `load_prl` and `pack_and_write_portals` remain the
public entry points.

### Task 2: Add the VisibilityRegions PRL Section

Add `postretro_level_format::visibility_regions::VisibilityRegionsSection` and
register `SectionId::VisibilityRegions = 37`. The section carries camera
leaf-to-region mapping, baked region PVS masks, and BVH node region masks. It is
optional. Absence means legacy runtime portal traversal. Update format tests for
round trip, malformed sizes, out-of-range ids, zero padding bits, and omitted
section fallback.

### Task 3: Build Coarse Visibility Regions

Add compiler-side region construction in a new visibility-region module.
Eligible leaves are non-solid BSP leaves not in the exterior set. Solid and
exterior leaves map to `u32::MAX`. Regions are deterministic, automatic, and
coarse: merge adjacent leaves through portal graph edges while respecting a
small hard leaf-count cap, an AABB growth cap, and a preference for not crossing
narrow portal bottlenecks. If the compiler cannot produce at most 1024 regions,
it omits the section with a warning and the runtime falls back to portal
traversal.

### Task 4: Compute Conservative Region PVS

Compute all-open region PVS from compiler portal geometry. Use a conservative
portal-window propagation algorithm over the BSP portal graph. A path that
survives clipping marks the target region visible. Numerical uncertainty,
depth-limit truncation, or unsupported geometry keeps the path open rather than
dropping visibility. The PVS includes the source region. No kinematic or
revealable blocker is allowed to reduce this base PVS.

### Task 5: Bake BVH Node Region Masks

After the existing flat BVH is built, compute one region mask per flat BVH node.
Leaf-node masks come from the region of the BVH leaf's `cell_id`. Internal-node
masks are the union of child masks. Keep the current `BvhSection` bytes stable;
store masks in `VisibilityRegions`. Add compiler diagnostics for mask
saturation, including the share of internal nodes whose mask is all regions or
above a high-popcount threshold.

### Task 6: Load Regions and Select Baked Visibility

Load the optional section into `LevelWorld` without changing old PRL behavior.
When the camera leaf maps to a valid visibility region, derive the frame's
drawable `VisibleCells` from the baked region PVS instead of calling
`portal_vis::portal_traverse`. Derive `fog_reachable` from the same PVS without
the `face_count > 0` filter, matching the existing distinction. Add a new
visibility path statistic for baked region PVS. Fall back to the current path
when the section is absent, camera leaf has no region, portals are absent, or
validation fails.

### Task 7: Add GPU Node-Mask Early-Out

Extend the camera cull resources with a node-region-mask storage buffer and a
visible-region-mask storage buffer. Clear the camera indirect buffer before
dispatch. In `bvh_cull.wgsl`, test the current node's region mask before AABB
tests and jump to `skip_index` when it has no visible region. Keep the existing
leaf-level visible-cell test as the exact final gate. Update `ShadowCullPipeline`
to share the new shader layout while binding all regions visible and all cells
visible.

### Task 8: Verify and Document

Add CPU-only tests for section decode, region generation, PVS conservatism,
runtime fallback, baked-path leaf derivation, node-mask traversal, camera
indirect clearing, and shadow all-visible behavior. Run stress-map diagnostics
that stop after visibility/BVH analysis and record the output in
`context/plans/in-progress/perf-baked-visibility-region-masks/research.md` when
orchestrated. Update context library docs after implementation, not during
drafting.

## Sequencing

**Phase 1 (sequential):** Task 1 — split-before-extend for oversized PRL seams.
**Phase 2 (sequential):** Task 2 — establishes the wire contract.
**Phase 3 (sequential):** Task 3, Task 4 — region IDs feed PVS computation.
**Phase 4 (sequential):** Task 5 — consumes region IDs and flat BVH nodes.
**Phase 5 (sequential):** Task 6 — consumes the section and defines frame masks.
**Phase 6 (sequential):** Task 7 — consumes runtime masks and changes shader layout.
**Phase 7 (sequential):** Task 8 — verifies all behavior and updates context.

## Rough sketch

Proposed modules:

- `crates/level-format/src/visibility_regions.rs`
- `crates/level-compiler/src/visibility_regions.rs`
- `crates/postretro/src/visibility_regions.rs`

Runtime path:

```text
camera position
-> LevelWorld::find_leaf
-> leaf_to_region[camera_leaf]
-> region_pvs[region]
-> drawable leaves in visible regions
-> visible-region mask upload
-> GPU BVH node-mask early-out
-> leaf frustum + exact cell test
-> indirect slots
```

Fallback path:

```text
missing/invalid section, no camera region, no regions, or explicit compiler off
-> current portal traversal / frustum fallback
-> visible-region mask is all ones
-> GPU traversal behaves like legacy after indirect clear
```

The all-open rule is the future-kinematics contract. Permanent structural
brushes may reduce baked PVS. Anything that can move, lower, break, open, or be
destroyed must be modeled as open for base PVS. Later stateful visibility gates
can narrow the mask only while closed.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Visibility regions section | `SectionId::VisibilityRegions` | section id `37` | n/a | n/a | n/a |
| Compiler toggle | `visibility_regions` arg field | CLI `--visibility-regions auto|off` | n/a | n/a | n/a |
| No region sentinel | `NO_VISIBILITY_REGION` | `u32::MAX` | n/a | n/a | n/a |

## Wire format

`VisibilityRegions` is an optional PRL section. All fields are little-endian.
The body version is `1`.

```text
u32 region_count
u32 bsp_leaf_count
u32 bvh_node_count
u32 region_word_count
u32 reserved0 = 0
u32 reserved1 = 0
u32 reserved2 = 0
u32 reserved3 = 0
u32 leaf_to_region[bsp_leaf_count]
u32 region_pvs[region_count * region_word_count]
u32 bvh_node_region_masks[bvh_node_count * region_word_count]
```

`region_word_count = ceil(region_count / 32)`. `region_count` must be
`1..=1024` when the section is present. Padding bits above `region_count` are
zero on disk. `leaf_to_region` entries are either a region id less than
`region_count` or `u32::MAX` for solid, exterior, or otherwise unmapped leaves.
`bsp_leaf_count` must match the loaded BSP leaf count. `bvh_node_count` must
match the loaded BVH node count. Empty or no-portal maps omit the section.

## Open questions

None requiring user decision before draft review. The main implementation risk
is node-mask saturation in the current global BVH; this plan measures that risk
but does not change the BVH builder unless a follow-up spec calls for it.
