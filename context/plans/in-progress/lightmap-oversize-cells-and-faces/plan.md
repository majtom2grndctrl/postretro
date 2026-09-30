# lightmap-oversize-cells-and-faces — plan of record

mode: resumable
status: approved
read at: e5eb0807c

## Owner rulings
- 2026-09-30: the oversized-split bound was unachievable in general (a 44 m single-cell cube room charts six 1104² faces that no two share a 2048 block or bake layer: 6 layers where the bound allowed 3; and MaxRects fill < 100% breaks it on very large cells). Owner took the recommended restatement: the Decision now states a fill rule (a chart moves to a later block only when it fits no earlier block's free space; blocks trimmed), and Acceptance [1] row 3 tests that rule by reinsertion, keeping the numeric bound only on a quarter-edge, ≤4-layer synthetic cell. `index.md` and `build_pipeline.md` §Compiler pipeline carry the new wording. Rejected: bake layers larger than the pool edge.
- 2026-09-30: plan approved.

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
| [1] Oversized split trimmed; no later-block chart fits an earlier block (reinsertion); numeric bound on a quarter-edge ≤4-layer synthetic cell | `block_layout` unit tests | achievable as stated |
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
| 1 | Multi-block cell packing (thinnest slice, riskiest premise): fill-then-trim split in `pack_cell_blocks`/`pack_cell_block`, `(cluster, cell, sub-block)` order, limits over every block; worker-count test; bound fixture | integrating executor | — | done — `block_layout::tests::{oversized_cell_packs_into_several_blocks_each_within_the_pool_edge, fitting_cells_pack_one_block_each_exactly_as_the_single_block_packer, oversized_cell_blocks_are_trimmed_and_no_later_chart_fits_an_earlier_block, quarter_edge_oversized_cell_occupies_at_most_one_layer_past_its_texel_area, multi_block_cells_are_contiguous_in_cluster_cell_sub_block_order, multi_block_pack_is_identical_with_one_worker_and_many}`, `lightmap_bake::tests::oversized_cell_vertices_name_their_own_charts_block`, `lightmap_stage::tests::multi_block_cell_section_bytes_are_identical_with_one_worker_and_many`; `--bin prl-build` 1405 pass. Pool edge is an internal packing parameter (`*_within`) so tests split cells with small charts |
| 2 | Loader contiguity rule (`validate_lightmap_block_cells`) and runtime `LevelBlockMap` cell → block range | integrating executor | 1 | done — `lightmap_stream::tests::{both_load_modes_accept_a_cell_owning_several_contiguous_blocks_and_one_owning_none, both_load_modes_reject_a_cell_whose_blocks_interleave_with_another_cells, both_load_modes_reject_a_block_cell_past_the_cells_table}`; `multi_block_tests::block_map_resolves_each_cell_to_its_contiguous_blocks_and_rejects_interleaving`; loader 274 pass |
| 3 | `BlockDemand` and install: all blocks per cell, per-block resident flags, release with in-flight discard, install/preload/settle (P12–P16) | delegated worker (runtime `lightmap_streaming`, `session/lightmap_residency`) | 2 | done (by the integrating executor, not delegated: the seam was two call sites) — `lightmap_streaming::controller::multi_block_tests::{a_cell_with_several_blocks_demands_all_of_them_at_its_class, a_cells_blocks_install_one_by_one_and_only_the_missing_one_is_non_resident, a_cell_leaving_demand_releases_every_block_including_one_in_flight, preload_installs_every_block_of_a_multi_block_mandatory_cell_before_settling}`; runtime `lightmap` filter 74 pass |
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
