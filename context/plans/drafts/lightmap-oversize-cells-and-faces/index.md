# lightmap-oversize-cells-and-faces

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §PRL section IDs, §Build Cache · `context/lib/rendering_pipeline.md` §Lightmap cell-block residency · `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §Resource bounds · read at 19fb3fc40 · evidence: `research.md`

## Problem

Developer-observed regression. movement-feel and kinematic-platform compiled until `b9219f301`. That commit replaced the 8192-texel atlas ceiling with the 2048 runtime pool layer, and both maps now fail atlas preparation at the default 0.04 m/texel. The cause: the compiler packs each cell into exactly one block no larger than a pool layer, and the BSP leaves a large open volume as one cell. Behind that limit sits a second one: a single face whose chart exceeds a pool layer also fails. That is about 82 m at 0.04, less at finer density or under a `_lightmap_scale` region. No map hits the second limit yet. The owner wants it lifted before an artist commits to a large surface and gets blocked. When done, any map compiles at any finite positive density. A cell's charts spread over as many blocks as they need. A face too large for one layer is cut with no visible seam. Density edits still leave the cached stages before atlas preparation untouched.

## Decisions

- **A cell owns one or more blocks.** Demand stays per cell (id 51); placement stays per block. This lifts the non-goal in `plans/done/spatial-residency--lightmap-cell-blocks`, keeping its unit. Two alternatives are rejected:
  - Size-driven BSP splitting: cells also serve network relevance, audio and AI perception.
  - A 4096 pool edge: the doubled shadowmask layer sits exactly at the 8192 limit, each layer costs 4×, every map is re-laid out, and the wall only moves 2×.
- **Blocks install independently.** A cell's blocks install one by one, as any blocks do today, and each stays a lightmap/shadowmask pair. Blocks of one cell are not dependent resources. A cut face whose second block is still in flight shows the same transient miss already accepted between neighbouring cells. Whole-cell atomic install is rejected: the drain always admits its first item, so a huge cell would become one unbounded install, the case `large-map-spatial-residency.md` records as setting the worst install.
- **Oversized cells pack as full-edge blocks plus one tight remainder.** A cell that fits one layer packs as today. Equal-sized splits are rejected: the next-fit layer packer would close a mostly empty layer per block.
- **A cell's blocks are contiguous.** Block order is cluster, then cell, then sub-block index. The loader rejects interleaved blocks, so the runtime derives a cell → block range. The id-22 byte layout and version do not change. The load rule narrows from "one block per cell" to "contiguous blocks per cell".
- **Oversize faces are cut at atlas preparation.** Pre-atlas stages consume uncut geometry, and their cache keys and the `build_pipeline.md` density-independence promise stand. After the cut, the compiler rebuilds from the cut geometry every uncached artifact that names faces, index ranges or BVH leaves: per-leaf face ranges, BVH, CellDrawIndex and the cell partition. It does this before block packing. A face at or under the pool edge is never cut. An oversize face is cut on its grid lines into the fewest sub-charts of near-equal extent. Sub-faces stay inside their parent's leaf and take its place in cut order.
  - Rejected: cutting before visibility encoding. It makes density an input to geometry, so tuning a large surface re-bakes the SH family.
  - Rejected: an early cut on a density-independent world grid. It cannot guarantee a fit under arbitrary scale regions.
  - Rejected: sub-faces sharing their parent's BVH primitive. A face would then own several charts, splitting "chart" from "face" across the lightmap, shadowmask, animated and id-25 paths.
  - Rejected: a per-face density cap. It quietly bakes a surface coarser than its author asked for.
  - Rejected: a per-map density override as the regression fix. Large open arenas are a product commitment; a map-by-map workaround is not.
- **Stages before atlas preparation emit no face identity.** Their outputs never name a face, an index range or a BVH leaf, so a late cut leaves them valid. This becomes a `build_pipeline.md` §Compiler pipeline invariant. A future pre-atlas stage that needs face identity must move after the cut.
- **The cell partition follows the cut.** It counts BVH leaves per cell, so a density edit that moves a cut can change cluster membership. It is uncached and rebuilt after the cut, and no cached pre-atlas stage keys on it. `build_pipeline.md` moves "partition inputs are final" from after the BVH stage to after the atlas-preparation cut.
- **A sub-chart is a window onto its parent chart's grid.** It carries the parent's origin, axes, pitch and resolved density, plus an integer texel offset and extent. Every per-texel quantity is computed from the parent-grid index: world position for the lightmap, shadowmask and animated weights. Adjacent sub-charts overlap by at least the bilinear footprint, baked as real surface. Rejected: plain sub-charts that re-derive their own grid. Grids that don't line up and dilated padding leave a line mid-surface, and a seamless cut is part of the outcome.
- **One seeding scheme, chart-local.** Every soft-visibility seed, static and animated, keys on the parent chart's world-space frame (origin and axes) and parent-grid texel coordinates. Seeds never use bake-layer coordinates or face indices. Both sides of a cut draw the same samples. Bake noise no longer depends on packing, and an unrelated edit elsewhere leaves a chart's noise alone. This overturns the non-goal "bake noise that survives repacking" in `plans/done/spatial-residency--lightmap-cell-blocks`.
  - This changes every map's lightmap bytes once; the owner accepts the one-time rebake.
  - Placement and non-lightmap output of maps that fit today do not change.
- **Oversize compiles fail early, by name.** Sub-face, block and bake-layer counts are computed at planning, before any bake buffer is allocated. Exceeding a format limit fails on the existing named block-count or layer-count errors. The block-too-large and chart-too-large errors become unreachable and are removed. `--verbose` logs the predicted peak memory of the lightmap stage (`research.md` §4).
- **Determinism is by construction.** Block packing stays parallel per cell, with one governor entry per cell. The split is a pure function of chart extents, with ties broken by index. Ids are assigned in one serial pass. The face cut and the rebuilds after it run in a fixed order. Output is identical at any worker count.
- **kinematic-platform is the first consumer of face cuts.** Its ceiling becomes a gable, so one wall peaks comfortably past the chart limit at 0.04, and the cut lands in the lit fade band. One added light crosses the cut with a soft shadow edge and an animation, so a seam in any baked term would show. The README is updated to match.
- **Non-goals.**
  - The default density stays 0.04 (owner; survey in `research.md` §6).
  - A back-filling layer packer: chart-local seeding makes it noise-free, but next-fit waste is not measured as a problem.
  - Parallel bake layers: `drafts/bake-parallelism-large-maps` keeps the layer loop serial (owner ruling).
  - Brush-side-anchored grids for coplanar BSP fragments. The window mechanism could fix those seams, but nobody has shown they are visible; that is a follow-up.
  - Tier validity at fine density: a huge cell may push one camera cell past the Low budget. The epic's lever for that is half resolution.
  - Finer residency within a cell.
- **Coordination.** `drafts/bake-parallelism-large-maps` holds output bytes unchanged. Its baselines must be taken after this lands, or retaken.

## Acceptance

### Automated
Multi-block cells:
- [ ] A cell whose charts exceed one pool layer compiles into two or more blocks, each within the pool edge. Every chart lands in exactly one block, and every vertex names its own chart's block.
- [ ] A cell whose charts fit one pool layer compiles to exactly one block with the pre-change extent and chart placements.
- [ ] Every block of an oversized cell but one spans the pool edge on both axes. The split adds at most one partly filled bake layer for that cell.
- [ ] Block ids, placements and section bytes are identical with one packing worker and with many.
- [ ] The loader accepts a cell owning several contiguous blocks, and a cell owning none. It rejects a cell whose blocks interleave with another cell's, and still rejects an out-of-range cell. The runtime resolves each cell to exactly its contiguous blocks (pin P10).
- [ ] Block demand:
  - A mandatory or visible cell with several blocks demands all of them.
  - Each block's table entry turns resident as its own pair installs. While one block of a cell is resident and another is not, only the missing block's entry lacks the resident flag (pins P12, P16).
  - A cell leaving demand releases all of its blocks, including one whose read is still in flight. That read's completion is discarded and its buffers released (pin P13).
  - Level install and capture preload install every block of a multi-block mandatory cell before the first frame. Lightmap residency reports settled only once all of those blocks are resident (pin P15).
- [ ] The residency dry run counts every block of a multi-block cell on a synthetic fixture. The walk measurement does the same on movement-feel, run on demand.
- [ ] movement-feel and kinematic-platform compile at the default density, each chart at the density its face resolves to (an on-demand, ignored check).

Face cuts:
- [ ] A face whose chart, padding included, is exactly at the pool edge is not cut. One texel over on one axis, it is cut into exactly two sub-charts along that axis and none along the other. Each fits, and their extents differ by at most one texel (pin P5).
- [ ] Across every cut, overlap texels are bit-identical on both sides before encoding: irradiance, direction, shadowmask visibility and animated weights.
- [ ] A sub-face whose own first vertex lies in a different scale region from its parent's uses the parent's density.
- [ ] After cuts:
  - per-leaf face ranges, BVH, CellDrawIndex, the cell partition and the animated chunk ranges on BVH leaves agree with the emitted geometry (pins P1, P2);
  - the loader's partition rebuild validates;
  - sub-faces take their parent's position in its leaf, in the window order pin P6 fixes.
- [ ] Face cuts and the rebuilds are identical with one worker and with many.
- [ ] On a small fixture with no face past the pool edge, the rebuilt per-leaf face ranges, BVH, CellDrawIndex and cell partition equal the ones built before atlas preparation (pin P4).
- [ ] A cut face lit by an animated light compiles. No vertex, including those on the cut, is shared between its sub-faces, and every sub-face's animated block keeps its bilinear footprint inside its placement (pin P8).
- [ ] A vertex on a cut maps to the same parent-grid texel position from both sub-faces, within vertex UV quantization (pin P20).
- [ ] A density edit that changes which faces are cut hits the warm caches of every stage that runs before atlas preparation. The warm build equals a cold build at the new density, including any change in cluster membership.
- [ ] A scale region extreme enough to need more bake layers than the compiler allows fails at atlas preparation with the named layer-count error, before the lightmap bake starts. The block-count limit keeps its synthetic-count check, now over every block of a multi-block cell (pin P11).
- [ ] The reshaped kinematic-platform compiles at the default density (on demand). Its tall wall is the only face cut, and every resulting chart fits.

Seeding and regression guard:
- [ ] Moving a chart to different bake-layer coordinates leaves its baked texels unchanged. So does an edit elsewhere in the map that renumbers faces but leaves that chart's surface and lights unchanged.
- [ ] The compiler no longer defines the chart-too-large or block-too-large error (grep gate). `build_pipeline.md` §Compiler pipeline states that no stage before atlas preparation emits face identity (review gate).
- [ ] For campaign-test and stress-warren-hallway-inspection:
  - block extents and chart placements are unchanged from before;
  - every section except the lightmap-family sections is byte-identical;
  - lightmap irradiance differs only within the soft-shadow noise tolerance.

### Manual
- [ ] movement-feel and kinematic-platform render correctly in both lightmap streaming modes. kinematic-platform's walls still fade to dark above the lit band.
- [ ] With one block of kinematic-platform's multi-block cell withheld, only faces on that block lose lightmap terms.
- [ ] On kinematic-platform, no seam or sparkle shows where the fade gradient, the added light's penumbra or its animation cross the cut, in both streaming modes.
- [ ] Recorded runs (protocol in `research.md` §4):
  - peak `prl-build` RSS for both maps against the `--verbose` prediction;
  - the hallway's peak RSS, unchanged;
  - pool layer count and the lightmap byte meter on movement-feel;
  - visible-miss counts while walking into kinematic-platform's multi-block cell.

## Path

- Seams:
  - Packing: `lightmap_bake/block_layout::pack_cell_blocks`, `cell_blocks::pack_cell_block` (`atlas_pack::MaxRects` is the multi-bin candidate), `BlockOrdering::sort_key`.
  - Loader: `prl_lightmap::validate_lightmap_block_cells`.
  - Runtime: `lightmap_streaming/block_map::LevelBlockMap`, `demand::BlockDemand`.
  - Measurement: `walk_measurement`, and the dry run's `cell_blocks::CellBlocks` and `layouts::stored_repack_matches`.
  - Cut: `pipeline/lightmap_stage` prepare; `pipeline/cell_partition::plan_cell_partition` moves inside it, after the cut; `bvh_build`; `cell_draw_index_bake`; `geometry_utils::split_polygon`; `lightmap_bake/charts::plan_charts`; `chart_raster::chart_texel_world_position`.
  - Seeds: `lightmap_bake::texel_seed`, `animated_light_weight_maps::soft_visibility_texel_seed`.
- Stages the cut leaves alone: ChunkLightList chunks are a world grid and name no faces; the SH family, entity-shadow selection, navmesh and cell visibility output no face identity (`research.md` §3). The fused bake and every stage after it take the rebuilt BVH.
- Shape: pre-atlas stages on uncut geometry, a cut and rebuild at atlas preparation, sub-charts as parent-grid windows. The strongest rival is cutting before visibility encoding. It is smaller, but it re-keys the SH family on every density edit that moves a cut.
- First slice: multi-block cells end to end, on movement-feel. It unblocks both maps, gives the visual checkpoint that face cuts need, and merges to main on its own once its rows pass. Later slices follow in their own merges. The second slice is chart-local seeding, then face cuts on the reshaped kinematic-platform. That is the riskiest: BC6H across a cut and T-junction sparkle are unverified.
- Split first, each in its own behavior-preserving commit: `pipeline.rs`, `geometry.rs` and `lightmap_bake.rs` are past 800 lines.
- Update with the change:
  - `build_pipeline.md`: §Compiler pipeline atlas preparation (cut and rebuild order, partition after the cut, the no-face-identity invariant for pre-atlas stages) and the id-22 load-reject list;
  - `rendering_pipeline.md` §Lightmap cell-block residency: a cell's blocks.

## Open questions

- How wide the sub-chart overlap is (at least the bilinear footprint) — **delegated**
- kinematic-platform's gable geometry and the added light, with the wall height chosen so the even cut lands in the lit band — **delegated**, within its Decision
