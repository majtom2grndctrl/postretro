# Baked Visibility Regions BVH Masks

## Goal

Move static-world visibility rejection earlier in GPU BVH traversal without
replacing runtime portal traversal. Runtime portal traversal remains the
authoritative source of visible cells. The compiler derives coarse visibility
regions from BSP cells and attaches region masks to global BVH nodes. Runtime
converts portal-visible cells into a visible-region mask so the GPU can skip
impossible internal BVH subtrees before reaching leaves.

Recent `stress-warren` diagnostics motivate this plan: a bad view visited about
5033 BVH nodes, tested 2386 leaves, rejected 2383 leaves by visible-cell bit,
and submitted only 3 leaves. The bottleneck is late visible-cell rejection in
global BVH traversal, not CPU portal traversal.

## Scope

### In scope

- Add an optional PRL visibility-region section.
- Derive coarse regions automatically from non-solid, non-exterior BSP leaves.
- Compute conservative all-open region PVS from compiler portal geometry for
  compiler diagnostics, reports, and future baked-vs-runtime comparison.
- Bake one region mask per flat BVH node.
- Keep existing runtime portal traversal as the authoritative camera visibility
  path.
- Derive the frame visible-region mask from runtime portal-visible cells.
- Keep the global BVH, material-bucket leaf ordering, and indirect draw layout.
- Add GPU BVH node-mask early-out for camera cull.
- Clear camera indirect slots before cull so skipped subtrees cannot retain
  stale draw commands.
- Preserve shadow cull correctness with all-regions visible for shadow passes.
- Add CPU-only tests and diagnostics for PVS conservatism, node-mask usefulness,
  and fallback behavior.
- Add CPU-only dev comparison diagnostics between baked region PVS and runtime
  portal traversal without using baked PVS as the core runtime path.

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
- Replacing or removing runtime portal traversal. Baked region PVS may become a
  follow-up path only after separate evidence and design review.
- GPU timing as an acceptance gate. GPU timing may be collected manually, but
  automated acceptance stays CPU-visible.

## Acceptance criteria

- [ ] PRLs with a `VisibilityRegions` section load and use runtime portal
      traversal plus visible-region GPU node-mask culling; PRLs without the
      section load and use runtime portal traversal with all-regions-visible
      masks.
- [ ] A present malformed `VisibilityRegions` section rejects the PRL. Absence
      remains valid fallback.
- [ ] Runtime `VisibleCells` for committed visibility fixtures and deterministic
      `stress-warren` camera points still come from current portal traversal:
      spawn plus centers of the first, median, and last drawable leaves.
- [ ] Dev comparison mode samples baked region PVS against runtime portal
      traversal, proves baked results are a superset, and logs counts. It does
      not route rendering through baked PVS.
- [ ] A connected multi-room fixture proves the compiler PVS is a proper subset
      of all regions. Raw graph reachability for every connected region is not
      accepted as the PVS algorithm.
- [ ] CPU mirror coverage proves BVH node-mask early-out produces the same final
      leaf submissions as leaf-level visible-cell plus frustum tests for the
      same camera inputs.
- [ ] CPU mirror coverage proves at least one fixture skips an internal BVH
      subtree before reaching its leaves.
- [ ] Deterministic `stress-warren` camera probes include the bad-view case
      where diagnostics showed about 5033 BVH node visits, 2386 leaf tests,
      2383 visible-cell rejects, and 3 submitted leaves. CPU mirror diagnostics
      prove node-mask early-out reduces estimated node visits and leaf tests
      while preserving the 3 final submissions.
- [ ] Camera indirect slots are cleared before camera cull. A regression test
      or CPU mirror proves a leaf skipped by an internal-node reject cannot
      retain a previous visible draw command.
- [ ] Shadow cull binds all regions visible and all cells visible, and its CPU
      mirror result matches the current cone-only cull result.
- [ ] `prl-build --visibility-regions-report <input.map>` and
      `prl-build --stop-after visibility-regions <input.map>` parse and build
      through geometry, BVH, and visibility-region analysis, then exit without
      writing a full PRL, lightmap, or SH bake.
- [ ] Compiler diagnostics report region count, region word count, average and
      max visible-region count, BVH node-mask saturation, and estimated
      masked-traversal node visits and leaf tests for `stress-warren.map` and
      `stress-warren-crates.map`.
- [ ] No acceptance criterion requires a GPU, adapter timestamp support, or
      `POSTRETRO_GPU_TIMING=1`; GPU timing remains manual diagnostics only.
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
round trip, version mismatch, malformed sizes, out-of-range ids, zero padding
bits, and omitted section fallback. A present malformed section is a load error,
not silent fallback. Semantic omissions happen at compile time by omitting the
section.

### Task 3: Build Coarse Visibility Regions

Add compiler-side region construction in a new visibility-region module.
Eligible leaves are non-solid BSP leaves not in the exterior set. Solid and
exterior leaves map to `u32::MAX`. Regions are deterministic, automatic, and
coarse. Merge adjacent leaves through portal graph edges with these defaults:
max 16 BSP leaves per region, max merged AABB diagonal 64 m, and narrow portal
barrier when `min(portal area) < 4 m^2` or portal area ratio is below `0.25`.
Process leaves and portals in ascending id order. Break merge ties by smaller
merged AABB volume, then lower region id. If the compiler cannot produce at
most 1024 regions, it omits the section with a warning and the runtime falls
back to portal traversal. Empty or no-portal maps omit the section.

### Task 4: Compute Conservative Region PVS Diagnostics

Compute all-open region PVS from compiler portal geometry for reporting and
future comparison support. It is not the runtime visibility source in this
plan. Use a conservative portal-window propagation algorithm over the BSP
portal graph. Propagate a convex portal polygon window through portals. Clip
with Sutherland-Hodgman against accumulated portal edge planes from source
eye-region bounds. A path that survives clipping marks the target region
visible. Cap chain depth at 256. Numerical uncertainty keeps the current edge
open and logs degradation. If a source degrades, the compiler may mark all
regions visible for that source, but must report it. The PVS includes the
source region. No kinematic or revealable blocker is allowed to reduce this
base PVS. Runtime rendering continues to use portal traversal.

### Task 5: Bake BVH Node Region Masks

After the existing flat BVH is built, compute one region mask per flat BVH node.
Leaf-node masks come from the region of the BVH leaf's `cell_id`. Internal-node
masks are the union of child masks. Keep the current `BvhSection` bytes stable;
store masks in `VisibilityRegions`. Add compiler diagnostics for mask
saturation, including the share of internal nodes whose mask is all regions or
above a high-popcount threshold. Runtime `cell_id` is the BSP leaf index. Every
drawable BVH leaf cell must map to a valid region when the section is present.
Solid, exterior, and empty leaves may use `u32::MAX`. If drawable geometry is
unmapped, the compiler omits the section or format validation rejects it. Do not
emit empty masks for drawable geometry.

### Task 6: Load Regions and Derive Runtime Masks

Load the optional section into `LevelWorld` without changing old PRL behavior.
Runtime `cell_id` equals BSP leaf index. `VisibleCells` contains BSP leaf ids
and still comes from `portal_vis::portal_traverse` or the existing frustum
fallback path. When a valid `VisibilityRegions` section exists, convert the
portal-visible drawable cells into a visible-region mask by OR-ing
`leaf_to_region[cell_id]` for visible drawable cells. Derive `fog_reachable`
exactly as the current portal path does; this plan does not route fog through
baked region PVS. Add a visibility-path statistic that reports portal traversal
plus region-mask acceleration. If the section is absent, explicitly omitted by
compiler option, or unusable for the camera leaf, synthesize an all-regions
visible mask. A malformed present section rejects load.

### Task 7: Add GPU Node-Mask Early-Out

Extend the camera cull resources with a node-region-mask storage buffer and a
visible-region-mask storage buffer. Clear the camera indirect buffer before
dispatch. In `bvh_cull.wgsl`, test the current node's region mask before AABB
tests and jump to `skip_index` when it has no visible region. Keep the existing
leaf-level visible-cell test as the exact final gate. Update `ShadowCullPipeline`
to share the new shader layout while binding all regions visible and all cells
visible. Account for the binding/layout coupling this creates: the shadow cull
path must bind the same node-mask and visible-region-mask resources even though
its effective region mask is all visible. For PRLs without `VisibilityRegions`,
synthesize one region word, every BVH node mask as all ones, and the
visible-region mask as all ones.

### Task 8: Verify and Document

Add CPU-only tests for section decode, region generation, PVS conservatism,
runtime fallback, portal-derived visible-region masks, dev comparison,
node-mask traversal, camera indirect clearing, and shadow all-visible behavior.
Run `prl-build --visibility-regions-report <input.map>` or
`prl-build --stop-after visibility-regions <input.map>` diagnostics and record
the output in
`context/plans/in-progress/perf-baked-visibility-region-masks/research.md` when
orchestrated. Required verification must run on CPU-only machines. GPU timing
may be collected manually but cannot gate completion. Update context library
docs after implementation, not during drafting.

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
-> runtime portal traversal / existing frustum fallback
-> portal-visible drawable cells
-> OR leaf_to_region[cell] into visible-region mask
-> visible-region mask upload
-> GPU BVH node-mask early-out
-> leaf frustum + exact cell test
-> indirect slots
```

Fallback path:

```text
missing section, no camera region, no regions, or explicit compiler off
-> current portal traversal / frustum fallback
-> visible-region mask is all ones
-> GPU traversal behaves like legacy after indirect clear
```

Malformed present section:

```text
decode or validation error
-> reject PRL load
```

The all-open rule is the future-kinematics contract for compiler diagnostics and
any later baked-PVS runtime experiment. Permanent structural brushes may reduce
baked PVS. Anything that can move, lower, break, open, or be destroyed must be
modeled as open for base PVS. Later stateful visibility gates can narrow the
mask only while closed.

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
u32 version = 1
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
`1..=1024` when the section is present. `version` must equal `1`; any other
value rejects the section. Padding bits above `region_count` are zero on disk.
`leaf_to_region` entries are either a region id less than `region_count` or
`u32::MAX` for solid, exterior, or empty leaves. `bsp_leaf_count` must match the
loaded BSP leaf count. `bvh_node_count` must match the loaded BVH node count.
Every drawable BVH leaf's `cell_id` must be a BSP leaf id whose
`leaf_to_region` entry is a valid region. Empty or no-portal maps omit the
section.

## Open questions

None requiring user decision before draft review.

Known risks:

- Node-mask saturation in the current global BVH may leave few internal nodes
  rejectable. This plan measures saturation and node-visit reduction, but does
  not change the BVH builder unless a follow-up spec calls for it.
- Shader mask bandwidth scales with `region_word_count`. The compiler cap and
  diagnostics must keep the word count visible before runtime cost grows.
- Internal-node early-out requires clearing camera indirect slots before cull;
  skipped subtrees otherwise risk stale draw commands.
- Shadow cull shares the camera cull shader layout but must bind all-visible
  region and cell masks. That binding/layout coupling needs tests so shadow
  cull stays cone-only.
