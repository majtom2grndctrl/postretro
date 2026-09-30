# lightmap-oversize-cells-and-faces

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §PRL section IDs, §Build Cache, §Navigation bake · `context/lib/rendering_pipeline.md` §Lightmap cell-block residency · `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §Resource bounds · read at 19fb3fc40 · evidence: `research.md`

## Problem

Developer-observed regression. movement-feel and kinematic-platform compiled until `b9219f301`. That commit replaced the 8192-texel atlas ceiling with the 2048 runtime pool layer, and both maps now fail atlas preparation at the default 0.04 m/texel. The cause: the compiler packs each cell into exactly one block no larger than a pool layer, and the BSP leaves a large open volume as one cell. Behind that limit sits a second one: a single face whose chart exceeds a pool layer also fails. That is about 82 m at 0.04, less at finer density or under a `_lightmap_scale` region. No map hits the second limit yet. The owner wants it lifted before an artist commits to a large surface and gets blocked. When done, any map compiles at any finite positive density. A cell's charts spread over as many blocks as they need. A face too large for one layer is cut with no visible seam. Density edits still leave the cached stages before atlas preparation untouched.

## Decisions

- **A cell owns one or more blocks.** Demand stays per cell (id 51); placement stays per block. This lifts the non-goal in `plans/done/spatial-residency--lightmap-cell-blocks`, keeping its unit. Rejected:
  - Size-driven BSP splitting: cells also serve network relevance, audio and AI perception.
  - A 4096 pool edge: the doubled shadowmask layer sits exactly at the 8192 limit, each layer costs 4×, every map is re-laid out, and the wall only moves 2×.
- **Blocks install independently.** A cell's blocks install one by one, as any blocks do today, and each stays a lightmap/shadowmask pair. Blocks of one cell are not dependent resources. A cut face whose second block is still in flight shows the same transient miss already accepted between neighbouring cells. Rejected: whole-cell atomic install. The drain always admits its first item, so a huge cell would become one unbounded install, the case `large-map-spatial-residency.md` records as setting the worst install.
- **An oversized cell's split is bounded, not shaped.** A cell that fits one layer packs as today. An oversized cell's blocks are each trimmed to their packed content. Together they occupy at most one bake layer more than their texel area needs. Neither shipped texels nor bake layers pay for empty block area.
- **A cell's blocks are contiguous.** Block order is cluster, then cell, then sub-block. The loader rejects interleaved blocks. The id-22 byte layout and version do not change. The load rule narrows from "one block per cell" to "contiguous blocks per cell".
- **Oversize faces are cut at atlas preparation.** Pre-atlas stages consume uncut geometry, so their cache keys and the `build_pipeline.md` density-independence promise stand. Before block packing, every uncached artifact that names faces, index ranges or BVH leaves is rebuilt from the cut geometry. A face at or under the pool edge is never cut. An oversize face is cut on its grid lines into the fewest sub-charts of near-equal extent. Sub-faces stay inside their parent's leaf, at its position. Rejected:
  - Cutting before visibility encoding: density becomes an input to geometry, so tuning a large surface re-bakes the SH family.
  - An early cut on a density-independent world grid: no fit guarantee under arbitrary scale regions.
  - Sub-faces sharing their parent's BVH primitive: a face would own several charts, splitting "chart" from "face" across the lightmap, shadowmask, animated and id-25 paths.
  - A per-face density cap: it quietly bakes a surface coarser than its author asked for.
  - A per-map density override as the regression fix: large open arenas are a product commitment.
- **Pre-atlas face identity is confined to a named rebuild set.** Per-leaf face ranges, geometry, BVH and CellDrawIndex name faces and are rebuilt after the cut. No other stage before atlas preparation emits face identity. This becomes a `build_pipeline.md` §Compiler pipeline invariant: a new face-naming stage joins the rebuild set or moves after the cut.
- **The cell partition follows the cut.** It counts BVH leaves per cell, so a density edit that moves a cut can change cluster membership. It is uncached and rebuilt after the cut, and no cached pre-atlas stage keys on it. `build_pipeline.md` moves "partition inputs are final" from after the BVH stage to after the atlas-preparation cut.
- **The SDF bakes from uncut geometry.** It describes the same surface, so its key and bake stay independent of density, and every density edit hits it. The pre-cut geometry stays alive until the SDF stage.
- **Cuts keep one surface for navmesh and collision.** Navmesh bakes from uncut triangles and collision reads cut ones. The `build_pipeline.md` §Navigation bake contract becomes "the same surface". A cut adds T-junctions against uncut neighbours. If the manual sparkle or snag rows fail, repairing the neighbouring edges is in scope.
- **A sub-chart is a window onto its parent chart's grid.** It carries the parent's frame, pitch and resolved density, plus an integer texel window. Every per-texel quantity for the lightmap, shadowmask and animated weights comes from the parent-grid index. Adjacent sub-charts overlap by at least the bilinear footprint, baked as real surface. Rejected: plain sub-charts that re-derive their own grid. Misaligned grids and dilated padding leave a line mid-surface.
- **One seeding scheme, chart-local.** Every soft-visibility seed, static and animated, keys on the parent chart's world-space frame and parent-grid texel. It never keys on bake-layer coordinates or face indices. Both sides of a cut draw the same samples. Bake noise no longer depends on packing or on unrelated edits. This overturns the non-goal "bake noise that survives repacking" in `plans/done/spatial-residency--lightmap-cell-blocks`. Every map's lightmap bytes change once; the owner accepts the rebake. Rejected: re-seeding only cut sub-charts to keep other maps byte-identical. That keeps two schemes forever to protect a guard the owner chose to give up.
- **Oversize compiles fail early, by name.** Sub-face, block, bake-layer and animated-block counts are computed at atlas preparation, before any bake buffer is allocated. The animated count is conservative: faces and sub-faces whose chart overlaps an animated light's influence. Exceeding a limit fails on its named error. The block-too-large and chart-too-large errors become unreachable and are removed. `--verbose` logs the predicted peak memory of the lightmap stage (`research.md` §4).
- **A map with no static lights cuts nothing.** Its charts skip packing as today, so an oversize face there changes no geometry.
- **Determinism is by construction.** Output is identical at any worker count. Block ids, block splits and cut order never depend on completion order.
- **kinematic-platform is the first consumer of face cuts.** A gable ceiling lifts one wall past the chart limit at 0.04, with the cut in the lit fade band. One added light crosses the cut with a soft shadow edge and an animation, so a seam in any baked term would show.
- **Non-goals.**
  - The default density stays 0.04 (owner; survey in `research.md` §6).
  - A back-filling layer packer: chart-local seeding makes it noise-free, but next-fit waste is bounded by the split Decision.
  - Parallel bake layers: `drafts/bake-parallelism-large-maps` keeps the layer loop serial.
  - Brush-side-anchored grids for coplanar BSP fragments. The window mechanism could fix those seams, but nobody has shown they are visible; that is a follow-up.
  - Tier validity at fine density: a huge cell may push one camera cell past the Low budget. The epic's lever for that is half resolution.
  - Finer residency within a cell: id 51 demands whole cells, and changing its unit is a format decision of its own.
- **Coordination.**
  - `drafts/bake-parallelism-large-maps` holds output bytes unchanged; its baselines must be taken after this lands, or retaken.
  - `drafts/bvh-leaf-clustering` changes BVH leaves from one per face; whichever lands second re-checks the post-cut rebuild and the partition's leaf count.

## Acceptance

Tags name the merge each row gates: [1] multi-block cells, [2] chart-local seeding, [3] face cuts.

### Automated
Multi-block cells:
- [ ] [1] A cell whose charts exceed one pool layer compiles into two or more blocks, each within the pool edge. Every chart lands in exactly one block, and every vertex names its own chart's block.
- [ ] [1] A cell whose charts fit one pool layer compiles to exactly one block with the pre-change extent and chart placements.
- [ ] [1] An oversized cell's blocks are each trimmed to their packed content, and together occupy at most one bake layer more than their texel area needs.
- [ ] [1] Block ids, placements and section bytes are identical with one packing worker and with many.
- [ ] [1] The loader accepts a cell owning several contiguous blocks, and a cell owning none. It rejects a cell whose blocks interleave with another cell's, and still rejects an out-of-range cell. The runtime resolves each cell to exactly its contiguous blocks (pin P10).
- [ ] [1] Block demand:
  - A mandatory or visible cell with several blocks demands all of them.
  - Each block's table entry turns resident as its own pair installs. While one block of a cell is resident and another is not, only the missing block's entry lacks the resident flag (pins P12, P16).
  - A cell leaving demand releases all of its blocks, including one whose read is still in flight. That read's completion is discarded and its buffers released (pin P13).
  - Level install and capture preload install every block of a multi-block mandatory cell before the first frame. Lightmap residency reports settled only once all of those blocks are resident (pin P15).
- [ ] [1] The residency dry run counts every block of a multi-block cell on a synthetic fixture. The walk measurement does the same on movement-feel, run on demand.
- [ ] [1] movement-feel and kinematic-platform compile at the default density, each chart at the density its face resolves to (an on-demand, ignored check).
- [ ] [1] For campaign-test and stress-warren-hallway-inspection, block extents and chart placements are unchanged from before (on demand).

Face cuts:
- [ ] [3] A face whose chart, padding included, is exactly at the pool edge is not cut. One texel over on one axis, it is cut into exactly two sub-charts along that axis and none along the other. Each fits, and their extents differ by at most one texel (pin P5).
- [ ] [3] Across every cut, overlap texels are bit-identical on both sides before encoding: irradiance, direction, shadowmask visibility and animated weights.
- [ ] [3] A sub-face whose own first vertex lies in a different scale region from its parent's uses the parent's density.
- [ ] [3] After cuts:
  - per-leaf face ranges, BVH, CellDrawIndex, the cell partition and the animated chunk ranges on BVH leaves agree with the emitted geometry (pins P1, P2);
  - the loader's partition rebuild validates;
  - sub-faces take their parent's position in its leaf, in the window order pin P6 fixes.
- [ ] [3] Face cuts and the rebuilds are identical with one worker and with many.
- [ ] [3] On a small fixture with no face past the pool edge, the rebuilt per-leaf face ranges, BVH, CellDrawIndex and cell partition equal the ones built before atlas preparation (pin P4).
- [ ] [3] A cut face lit by an animated light compiles. No vertex, including those on the cut, is shared between its sub-faces, and every sub-face's animated block keeps its bilinear footprint inside its placement (pin P8).
- [ ] [3] A vertex on a cut maps to the same parent-grid texel position from both sub-faces, within vertex UV quantization (pin P20).
- [ ] [3] A density edit that changes which faces are cut hits the warm caches of every stage that runs before atlas preparation, and the SDF and cell residency set caches. The warm build equals a warm build at the new density from an empty cache, including any change in cluster membership. Reverting the edit reproduces the first build byte for byte (pins P17, P18).
- [ ] [3] On a map with no static lights, an oversize face is not cut; its chart skips packing and the compile succeeds (pin P19).
- [ ] [3] A scale region extreme enough to need more bake layers than the compiler allows fails at atlas preparation with the named layer-count error, before the lightmap bake starts. One that would exceed the animated block cap fails there too, on the animated cap's named error. The block-count limit keeps its synthetic-count check, now over every block of a multi-block cell (pin P11).
- [ ] [3] The reshaped kinematic-platform compiles at the default density (on demand). Its tall wall is the only face cut, and every resulting chart fits.
- [ ] [3] The compiler no longer defines the chart-too-large or block-too-large error (grep gate). `build_pipeline.md` §Compiler pipeline names the pre-atlas face-identity rebuild set (review gate).

Seeding:
- [ ] [2] Moving a chart to different bake-layer coordinates leaves its baked texels unchanged. So does an edit elsewhere in the map that renumbers faces but leaves that chart's surface and lights unchanged.
- [ ] [2] For campaign-test and stress-warren-hallway-inspection, each compiled `--release` at `19fb3fc40` and after the change (an on-demand, ignored check):
  - block extents and chart placements are unchanged;
  - every section is byte-identical except the lightmap, shadowmask and animated weight-map sections. The exception extends, only where re-seeding flips a chunk between lit and unlit, to the animated chunk section and the chunk ranges and block ids it stamps into the BVH and geometry;
  - lightmap irradiance differs only within a per-texel tolerance recorded with the baseline.

### Manual
- [ ] [1] movement-feel and kinematic-platform render correctly in both lightmap streaming modes. kinematic-platform's walls still fade to dark above the lit band.
- [ ] [1] With one block of kinematic-platform's multi-block cell withheld, only faces on that block lose lightmap terms.
- [ ] [3] On kinematic-platform, no seam or sparkle shows where the fade gradient, the added light's penumbra or its animation cross the cut, in both streaming modes.
- [ ] [3] Walking across and sliding along a cut floor and a cut wall does not snag at the cut.
- [ ] Recorded runs (protocol in `research.md` §4):
  - [1] peak `prl-build` RSS for both maps against the `--verbose` prediction, and the hallway's peak RSS unchanged;
  - [1] pool layer count and the lightmap byte meter on movement-feel;
  - [1] visible-miss counts while walking into kinematic-platform's multi-block cell.

## Path

- Seams:
  - Packing: `lightmap_bake/block_layout::pack_cell_blocks`, `cell_blocks::pack_cell_block` (`atlas_pack::MaxRects` is the multi-bin candidate), `BlockOrdering::sort_key`. One shape that meets the bound: fill blocks toward the pool edge while the remaining charts do not fit one layer, trim each, then pack the rest tightly.
  - Loader: `prl_lightmap::validate_lightmap_block_cells`.
  - Runtime: `lightmap_streaming/block_map::LevelBlockMap`, `demand::BlockDemand`.
  - Measurement: `walk_measurement`, and the dry run's `cell_blocks::CellBlocks` and `layouts::stored_repack_matches`.
  - Cut: `pipeline/lightmap_stage` prepare; `pipeline/cell_partition::plan_cell_partition` moves inside it, after the cut; `bvh_build`; `cell_draw_index_bake`; `geometry_utils::split_polygon`; `lightmap_bake/charts::plan_charts`; `chart_raster::chart_texel_world_position`; the SDF stage key and inputs.
  - Seeds: `lightmap_bake::texel_seed`, `animated_light_weight_maps::soft_visibility_texel_seed`.
  - Animated cap: `animated_light_chunks::build_animated_light_chunks` overlap test, `animated_block_ids` cap check.
- The fused bake and every stage after it take the rebuilt BVH (`research.md` §3, §7).
- First slice: multi-block cells end to end, on movement-feel. It unblocks both maps, gives the visual checkpoint that face cuts need, and merges to main on its own once its [1] rows pass. Seeding merges next, then face cuts on the reshaped kinematic-platform, the riskiest: BC6H across a cut and T-junction sparkle are unverified.
- Split first, each in its own behavior-preserving commit: `pipeline.rs`, `geometry.rs` and `lightmap_bake.rs` are past 800 lines.
- Update with the change:
  - `build_pipeline.md`: §Compiler pipeline atlas preparation (cut and rebuild order, partition after the cut, the face-identity rebuild set), §Navigation bake ("the same surface"), §Build Cache (SDF from uncut geometry), and the id-22 load-reject list;
  - `rendering_pipeline.md` §4 Static direct and §Lightmap cell-block residency: a cell's blocks.

## Open questions

- How wide the sub-chart overlap is (at least the bilinear footprint) — **delegated**
- kinematic-platform's gable geometry and the added light, with the wall height chosen so the even cut lands in the lit band — **delegated**, within its Decision
