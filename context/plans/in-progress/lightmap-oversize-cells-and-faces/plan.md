# lightmap-oversize-cells-and-faces — plan of record

mode: resumable
status: blocked
read at: e5eb0807c

## Block

**Decision "An oversized cell's split is bounded, not shaped" and Acceptance [1] row 3 cannot hold for every input.** Proposed restatement below; owner decides.

Evidence (source at `e5eb0807c`):

- `atlas_pack::choose_layer_dim` sizes every bake layer to the smallest power of two that hosts the largest group alone. Cell blocks are at most the 2048 pool edge, so the bake-layer edge is at most 2048 (`MAX_ATLAS_DIMENSION` = 8192 is only the cap).
- `atlas_pack::pack_groups_into_layers` is next-fit: it never revisits a closed layer.
- Two charts wider and taller than half a pool edge cannot share one 2048² block or one 2048² bake layer.

Counterexample: a convex 44 m cube room is one BSP cell (`research.md` §1). At 0.04 m/texel each of its six faces charts at 1100 + 2·2 padding = 1104² texels. No two fit one block or one layer, so any packer produces six blocks on six bake layers. Their texel area is 6 × 1104² ≈ 7.31 M texels ≈ 1.74 layers, so the bound allows 3 layers.

The bound also fails for small charts on very large cells, because MaxRects fill is below 100%. At 90% fill, 11 full blocks plus a small remainder occupy 12 layers, while their texel area (≈ 9.95 layers) allows 11. The bound holds only when fill approaches 100%, which no rectangle packer guarantees.

The two failing maps probably stay within the bound, estimated from the `research.md` §1 extents and not measured: movement-feel cell 27 has ~630² panels, and kinematic-platform cell 12 has two ~1595 × 1473 charts plus three small ones. The problem is the universal claim, which a test must prove.

### Proposed wording (recommended)

The research's intent (§4) holds: near-edge split blocks each closing a mostly used layer is the waste to avoid, and "full-edge blocks plus one tight remainder" is the fix. The proposal states that fill rule instead of the numeric bound, and keeps the numeric bound only where chart shape allows it.

Decision, replacing its second to fourth sentences:

> An oversized cell fills each block before opening the next: a chart moves to a later block only when it fits no earlier block's free space. Each block is trimmed to its packed content, so shipped texels never pay for empty block area. Bake layers pay only for the waste a block's own shape forces, never for a split that could have filled an earlier block.

Acceptance [1] row 3, replacing it:

> - [ ] [1] An oversized cell's blocks are each trimmed to their packed content, and no chart in a later block of that cell fits the free space left in an earlier one (checked by reinsertion). On a synthetic cell whose charts are each at most a quarter pool edge on both axes and whose texel area fits in 4 layers, the blocks together occupy at most one bake layer more than their texel area needs.

The fixture limits (quarter-edge charts, at most 4 layers of area) keep the numeric bound inside what MaxRects fill actually achieves. The executor will confirm the limits in the first task's test. If they have to change, only the numeric part changes.

### Alternative

Keep the bound and let bake layers grow past the pool edge, for example to 4096 or 8192, so several pool-edge blocks share a bake layer. This does not make the bound universal: rigid rectangles still leave shape-forced gaps, just at a larger scale. It also raises the per-layer bake peak about 4× (warm plane ~912 MiB at 4096², `research.md` §4). Not recommended.

## Corrections
- Brief *Path* "Update with the change" lists `build_pipeline.md` and `rendering_pipeline.md` edits. Promotion (`492162d0a`) already wrote the post-change contracts with *Not built yet* markers, so each merge removes its markers and adjusts wording rather than writing new sections.
- `research.md` §3 says `b9219f301` replaced `MAX_ATLAS_DIMENSION`. The constant still exists at 8192 and caps bake layers; the 2048 pool edge caps blocks and charts. No planning impact beyond the Block above.
- Three commits landed since the brief's `19fb3fc40` read. Only `16c330a68` touches source: it rekeys the SDF atlas and CellResidencySet on density-independent inputs, matching the SDF and id-51 Decisions. Every cited symbol was re-read at `e5eb0807c` and matches the brief.

## Delegated answers
- **Sub-chart overlap width.** Each sub-chart extends 2 parent-grid texels past its cut line, so the overlap is 4 texels. The bilinear footprint needs 1 texel; the second covers the 1/256 fragment snap and vertex UV quantization (P20). BC6H blocks straddling the cut are still encoded separately on each side. The encoded-seam check (task 12) widens the overlap to `lcm(4, direction texel scale)` and aligns sub-chart placement to the parent grid mod that value if the check or the manual seam row shows a step.
- **kinematic-platform gable and added light.** Answered in task 13 with the map edit, within its Decision: a gable ceiling lifts one wall past the chart limit at 0.04; the wall height is chosen so the even cut lands in the lit fade band; one added animated light with a soft shadow edge crosses the cut. The chosen dimensions are recorded here when made.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| [1] Oversized cell → ≥2 blocks within pool edge; each chart in exactly one block; each vertex names its chart's block | `block_layout` unit test on a synthetic oversized cell; `atlas_layout` vertex-block assertion | achievable as stated |
| [1] Cell fitting one layer → one block with pre-change extent and placements | `block_layout` test comparing against `pack_cell_block` output | achievable as stated |
| [1] Oversized split trimmed and bounded | — | **needs restatement** (see Block) |
| [1] Block ids, placements and section bytes identical at 1 and many workers | `block_layout` worker-count test (new; `research.md` §5 notes none exists) | achievable as stated |
| [1] Loader accepts contiguous multi-block and zero-block cells, rejects interleaved and out-of-range cells; runtime resolves each cell to its contiguous blocks (P10) | `prl_lightmap` tests; `block_map` tests | achievable as stated |
| [1] Demand: multi-block mandatory/visible cell demands all blocks | `demand` unit test | achievable as stated |
| [1] Demand: per-block resident flag, only the missing block lacks it (P12, P16) | `demand` / install drain test | achievable as stated |
| [1] Demand: leaving cell releases all blocks, in-flight completion discarded (P13) | residency controller test | achievable as stated |
| [1] Demand: level install and capture preload install every block of a multi-block mandatory cell; settled only once all are resident (P15) | install + settle-query test | achievable as stated |
| [1] Dry run counts every block of a multi-block cell (synthetic); walk measurement on movement-feel on demand | `lightmap_residency_dry_run` test; `#[ignore]` walk measurement | achievable as stated |
| [1] movement-feel and kinematic-platform compile at default density, charts at their resolved density | `#[ignore]` compile check | achievable as stated |
| [1] campaign-test and hallway block extents and placements unchanged | `#[ignore]` baseline comparison | achievable as stated |
| [3] Chart at pool edge not cut; edge+1 → two sub-charts on that axis only, extents within 1 texel (P5) | cut-selection unit test | achievable as stated |
| [3] Overlap texels bit-identical across a cut (irradiance, direction, shadowmask, animated weights) | cut fixture bake test, pre-encode buffers | achievable as stated |
| [3] Sub-face in another scale region uses parent density | charts unit test | achievable as stated |
| [3] Post-cut leaf ranges, BVH, CellDrawIndex, partition, animated chunk ranges agree with geometry; loader partition rebuild validates; sub-faces at parent's slot in P6 order (P1, P2) | cut pipeline fixture test + loader round trip | achievable as stated |
| [3] Cuts and rebuilds identical at 1 and many workers | worker-count test | achievable as stated |
| [3] No-cut fixture: rebuilt leaf ranges, BVH, CellDrawIndex, partition equal pre-atlas (P4) | fixture test | achievable as stated |
| [3] Animated-lit cut face compiles; no shared vertex between sub-faces; footprints inside placements (P8) | animated weight-map fixture test | achievable as stated |
| [3] Cut vertex maps to same parent-grid texel from both sides within UV quantization (P20) | `atlas_layout` test | achievable as stated |
| [3] Cut-moving density edit hits pre-atlas, SDF and id-51 caches; warm equals fresh warm; revert byte-identical (P17, P18) | warm-cache integration test with log capture | achievable as stated |
| [3] No static lights: oversize face not cut, skips packing, compiles (P19) | `atlas_layout` test | achievable as stated |
| [3] Layer-count and animated-cap overflow fail by name at atlas preparation before bake; block-count synthetic check over sub-blocks (P11) | error-path tests | achievable as stated |
| [3] Reshaped kinematic-platform compiles; tall wall the only cut; every chart fits | `#[ignore]` compile check | achievable as stated |
| [3] ChartTooLarge / BlockTooLarge gone (grep); `build_pipeline.md` names the rebuild set (review) | grep gate; review | achievable as stated |
| [2] Chart moved in bake-layer coords, or faces renumbered elsewhere → texels unchanged | lightmap / weight-map seed tests | achievable as stated |
| [2] campaign-test and hallway `--release` at `19fb3fc40` vs after: placements unchanged, sections identical except the listed families, irradiance within recorded tolerance | `#[ignore]` baseline comparison | achievable as stated |
| [1] Both maps render correctly in both streaming modes; kinematic walls fade | owner, in-engine | manual |
| [1] One withheld block of the multi-block cell drops only its faces' terms | owner, in-engine | manual |
| [3] No seam or sparkle across the cut (fade, penumbra, animation), both streaming modes | owner, in-engine | manual |
| [3] No snag walking or sliding across cut floor and wall | owner, in-engine | manual |
| [1] Peak `prl-build` RSS vs `--verbose` prediction; hallway RSS unchanged | executor-recorded run (`research.md` §4 protocol) | manual |
| [1] Pool layer count and lightmap byte meter on movement-feel | executor-recorded in-engine run | manual |
| [1] Visible-miss counts walking into kinematic-platform's multi-block cell | owner, in-engine (dev-tools Streaming tab) | manual |

## Tasks

The Path merges three times: [1], then [2], then [3]. Each merge runs its own preflight, review loop and report, then waits for "land the plane" before the next milestone branches from the merged `main`.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Multi-block cell packing (thinnest slice, riskiest premise): fill-then-trim split in `pack_cell_blocks`/`pack_cell_block`, `(cluster, cell, sub-block)` order, limits over every block; worker-count test; bound fixture | integrating executor | Block cleared | |
| 2 | Loader contiguity rule (`validate_lightmap_block_cells`) and runtime `LevelBlockMap` cell → block range | integrating executor | 1 | |
| 3 | `BlockDemand` and install: all blocks per cell, per-block resident flags, release with in-flight discard, install/preload/settle (P12–P16) | delegated worker (runtime `lightmap_streaming`, `session/lightmap_residency`) | 2 | |
| 4 | Dry run `CellBlocks` / `stored_repack_matches` and walk measurement per block | delegated worker (`lightmap_residency_dry_run`, `walk_measurement`) | 2 | |
| 5 | On-demand checks: movement-feel and kinematic-platform compile; campaign-test and hallway placements unchanged; recorded RSS and meter runs; remove [1] *Not built yet* markers | integrating executor | 3, 4 | |
| — | Merge [1]: preflight, review loop, report, land | integrating executor | 5 | |
| 6 | Chart-local seeds: `texel_seed` and `soft_visibility_texel_seed` keyed on chart frame and chart-grid texel; stage epoch bumps; seed-invariance tests | integrating executor | merge [1] | |
| 7 | `19fb3fc40` vs post-change `--release` comparison for campaign-test and hallway; record the irradiance tolerance; remove [2] markers | integrating executor | 6 | |
| — | Merge [2]: preflight, review loop, report, land | integrating executor | 7 | |
| 8 | Behavior-preserving splits of `pipeline.rs`, `geometry.rs`, `lightmap_bake.rs`, one commit each | integrating executor | merge [2] | |
| 9 | Sub-chart window model: parent frame, pitch, density and texel window on `Chart`; `chart_texel_world_position` and seeds from the parent-grid index; cut selection (P5, P7) | integrating executor | 8 | |
| 10 | Geometry cut and rebuild set in atlas preparation: clip and re-fan, leaf-range recount, BVH, CellDrawIndex, Cells and partition after the cut (P1, P2, P4, P6); SDF from pre-cut geometry; no-static-lights path (P19) | integrating executor | 9 | |
| 11 | Early limits at atlas preparation: block, layer and conservative animated counts; remove ChartTooLarge and BlockTooLarge; `--verbose` peak-memory prediction (P11) | integrating executor | 10 | |
| 12 | Cut proof tests: overlap bit-identity, animated guards (P8), P20, worker count, cache P17/P18; encoded-seam check that settles the overlap width | integrating executor | 11 | |
| 13 | Reshape kinematic-platform (gable + light); on-demand compile check; remove [3] markers; `build_pipeline.md` rebuild-set invariant | integrating executor | 12 | |
| — | Merge [3]: preflight, review loop, report, land | integrating executor | 13 | |
