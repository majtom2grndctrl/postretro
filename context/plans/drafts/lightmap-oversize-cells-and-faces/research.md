# Research: lightmap oversize cells and faces

Derivation companion to `index.md`. Read at `19fb3fc40`. Scratch outputs (compile logs, PRLs, a section-dump tool `prldump/`) are in the drafting session's scratchpad and are not durable.

## 1. Basis: the two failing maps

Default density 0.04 m/texel. Error class is `BlockTooLarge` on both. Neither hits `ChartTooLarge`.

| Map | Cell | Extent (m) | Faces | Block at 0.04 | Largest chart | Compiles at |
|---|---|---|---|---|---|---|
| movement-feel | 27 (dome air above wall tops) | 36.6 × 12.2 × 76.2 | 27 dome panels, ~20 m each | 2548 × 1664 | ~630² | 0.045 (1744 × 1960) |
| movement-feel | 170 (other dome half) | 29.5 × 11.9 × 76.2 | 26 | fits at 0.04 | — | — |
| kinematic-platform | 12 | 19.5 × 63.8 × 58.9 | 5 | 1600 × 3076 | ~1595 × 1473 | 0.06 (2048 × 1400) |

- kinematic-platform's 63.8 m ceiling is authored: walls fade into darkness above (owner). Its cell holds ~7,300 m² of surface, about 4.56 M texels at 0.04, more than one 2048² layer (4.19 M) before packing loss. Its 64 m wall exceeds the chart limit at 0.02.
- campaign-test (passes): 198 blocks, worst edge 1316 (cell 319, 248 × 1316), largest area 696 × 1044. About 1.5× linear margin.
- Cells are raw BSP empty leaves. `partition::brush_bsp::select_splitter` picks only brush-side planes spanning the region; a region with no candidates becomes a leaf. No size-driven splitting. A convex open volume is one cell by construction.
- `plans/done/spatial-residency--lightmap-cell-blocks/index.md` lists "splitting a cell whose block exceeds a pool layer" as a non-goal; its margin research measured only hallway and campaign-test.

## 2. One-block-per-cell assumptions

| Site | Assumption | Class |
|---|---|---|
| `level-loader` `prl_lightmap::validate_lightmap_block_cells` | rejects a cell named by two blocks; runs in both load modes (also via `lightmap_stream/load::read_lightmap_index_prefix`); a `lightmap_stream/tests.rs` test pins the message | rule |
| `postretro` `lightmap_streaming/block_map::LevelBlockMap::build` | rejects "cell owns more than one block"; `cell_to_block: Vec<u32>`, `block_of_cell -> Option<u32>` | rule + shape |
| `lightmap_streaming/demand::BlockDemand::recompute`, `drawn_blocks_of` | one block per demanded cell; slots already per block | shape |
| `session/lightmap_residency/walk_measurement::run`, `mandatory_blocks` | `block_of_cell` vec; a second block overwrites the first | shape |
| `lightmap_residency_dry_run/cell_blocks::CellBlocks` | per-cell `dims`, `block_bytes`; sims consume them | shape |
| `lightmap_residency_dry_run/layouts::stored_repack_matches` | last block wins; repacks one block per cell | shape |
| `lightmap_bake/block_layout::pack_cell_blocks` | one `pack_cell_block` per cell; id = position under `BlockOrdering::sort_key` (cluster, cell) | shape |
| `level-format` `LightmapBlockIndex::from_prefix` / `validate_record` | no uniqueness check | none |

Already block-keyed, no change: id 42 (`shadowmask_atlas.rs`, pairs by block index), id 25 and the animated path (`static_atlas_frame::rebase_to_cell_block`, `prl_animated_atlas::check_blocks_inside_cell_blocks`), render-cpu `LightmapPoolModel`/`BlockPool`, renderer pool, id 51 (per cell, never names blocks), animated chunk → cell visibility map.

A face never spans blocks: `atlas_layout::assign_lightmap_uvs` gives every vertex of a face `chart_blocks[face] + 1`.

## 3. Face split premises

- Faces: `partition/face_extract::extract_faces` emits one face per brush-side hull per empty leaf. No existing world-face subdivision; `partition/manifold::check_watertight` tolerates T-junctions.
- Clipper: `geometry_utils::split_polygon` (Sutherland–Hodgman, DVec3, convex).
- Order in `pipeline::run_after_parsing`: partition → check_watertight → generate_portals → find_exterior_leaves → `visibility::encode_vis` → `extract_geometry` → build_bvh → cell_draw_index → navmesh → SH family → entity-shadow selection → billboard scatter → ChunkLightList → `pipeline/cell_partition::plan_cell_partition` (reads the BVH section) → `pipeline/lightmap_stage` prepare (`split_shared_vertices`, `plan_charts`, pack, `assign_lightmap_uvs`) → fused bake (live `bvh`/`bvh_primitives`) → animated chunks → weight maps → SDF → `stamp_animated_blocks` → pack (`write_finalized_prl`; ids 17 and 19 written here).
- Chart grid (`lightmap_bake/charts::plan_charts`): origin = face first vertex; u = normalize(p1 − p0); v = normal × u; `uv_min` = min projection; `width_texels = ceil(extent/density) + 2·pad`. Pitch is stretched: `uv_extent / ceil(uv_extent/density)`. Density from `resolved_chart_density(origin)`, last matching `MapLightmapScaleRegion` wins. Re-deriving a chart from a sub-polygon changes origin, basis, `uv_min`, pitch and possibly density.
- Coverage: `lightmap_layer::for_each_light_layer_chart_texel` bakes every interior texel of the chart rect at `chart_raster::chart_texel_world_position`. No triangle test. Shadowmask (`shadowmask_bake`), animated weights (`animated_light_weight_maps::bake_one_chunk`) and `lightmap_bake/reference` use the same bound. `lightmap_bake::dilate_edges` fills only the padding ring (`CHART_PADDING_TEXELS` = 2). Overlap texels inside a sub-chart's interior are therefore baked as real surface.
- **Seeds are bake-layer-coordinate keyed.** `lightmap_bake::texel_seed(atlas_x, atlas_y)` (lightmap_layer, reference) and `animated_light_weight_maps::soft_visibility_texel_seed(ax, ay)`. Two sub-charts covering the same parent texel sit at different atlas coordinates, so they get different soft-visibility samples. World position computed from each sub-chart's own `uv_min` also differs by rounding. Bit-identical overlap needs both computed from the parent-grid index.
- Vertex UVs: `atlas_layout::chart_texel_position` projects onto the chart basis. `split_shared_vertices` already gives each face its own vertices.
- Consumers that see a split as N faces: BVH (`bvh_build::collect_primitives`, one primitive per face), `cell_draw_index_bake` (merges contiguous same-cell spans), charts 1:1 with faces, `animated_light_chunks`, `animated_block_ids`, ChunkLightList, SDF, SH, entity-shadow selection. Collision (`physics` `populate_from_level`) and navmesh (`navmesh_bake::bake_navmesh`) read render triangles; a split adds coplanar triangles only. No u16 face-count cap found.
- One-block-per-cell statements beyond the Path list: `rendering_pipeline.md` §4 Static direct ("packed as one block per cell"), and `large-map-spatial-residency.md` §Decisions still open › Packing granularity ("Each cell's charts pack into one BC-aligned block").
- Parent-grid window math is exact per texel only below about 2^23 texels per parent axis: `chart_texel_world_position` indexes in f32. `ChartDimensionOverflow` stays reachable, alongside the block-count and layer-count errors. Global density is clamped to at least 1e-4 m/texel.
- **b9219f301** (2026-09-28) replaced the 8192 per-chart atlas ceiling (`MAX_ATLAS_DIMENSION`) with the 2048 pool edge. Both failing maps compiled before it.

Cut placement, early vs late:

| Artifact | Cached | Under a late cut (chosen) |
|---|---|---|
| `encode_vis` leaf face ranges (copied into Cells by `pack::encode_cells`) | no | recount per leaf; sub-faces stay in the parent's leaf |
| Geometry / id 17 | no | rewrite in place: faces own their vertices and are fans; clip, re-fan, rebuild `face_index_ranges`; texture UVs interpolate exactly (affine) |
| BVH / id 19 | no | rebuild; primitives are faces and point into the index buffer; the fused bake, shadowmask and weight maps use the rebuilt one |
| CellDrawIndex / id 37 | no | rebuild |
| Cell partition / ClusterDirectory | no | rerun after the cut; `canonical_cell_partition` counts BVH leaves per cell, and the loader rebuilds it from the emitted BVH to validate |
| ChunkLightList / id 23 | yes | keep; chunks are an 8 m world grid, output names no faces |
| SH family, billboard scatter | yes | keep; outputs are probe grids; keys hash `sh_group::geometry_content_hash` over the whole `GeometryResult`, so an early cut re-keys all of them |
| Entity-shadow selection, navmesh, cell visibility | yes / pre-atlas | keep; no face identity in output |
| SDF, id 51 | yes, post-atlas | Once the position-and-index SDF key lands, the SDF atlas re-keys only on density edits that move a cut: the cut rewrites positions and indices. CellResidencySet leaves face ranges out of its key and still hits. `build_pipeline.md`'s post-atlas density-independence sentence must carve out cut-moving edits. |

Early cut: about 150–250 lines, but every density or scale edit that moves a cut re-keys the SH family, ChunkLightList and navmesh. Late cut: roughly 2–3× the code, and pre-atlas caches hold. Both add T-junctions against uncut neighbours.

Alternatives to cutting:

| Option | Fixes compile | Cost |
|---|---|---|
| Keep the named chart-too-large error, better message | no | blocks an artist at the moment of commitment |
| Per-face density cap with a warning | yes | bakes coarser than authored; amends the `_lightmap_scale` contract |
| Cut on the parent grid (chosen) | yes, at full density | BC6H and T-junction behaviour at cuts unverified until kinematic-platform proves them |

Seeding: chart-local seeds (chart identity + parent-grid texel) replace bake-layer-coordinate seeds everywhere. One-time lightmap-family byte change on every map; SH caches are unaffected. Bake noise then no longer depends on block or layer placement.

No current `content/` map uses a `lightmap_scale_region` or hits the chart limit at 0.04. kinematic-platform, reshaped with a gable, is the consumer that proves the cut.

## 4. Bake memory

Per `development_guide.md` §1.4. Bake-layer edge = smallest power of two ≥ the largest block (`choose_layer_dim`), capped by the 2048 block limit; `MAX_ATLAS_LAYERS` = 256.

| Buffer | Scope | Size | Co-resident |
|---|---|---|---|
| Cold `bake_atlas_layer_controlled` plane / warm `IncrementalLayerAccumulator` | one bake layer | 29 B/texel cold, ~57 B/texel warm (~116 / ~228 MiB at 2048²) | one layer at a time |
| `reference::bake_face_chart` scratch | one chart per worker | 29 B × chart area × workers | up to `-j` |
| `LightmapLayer.texels` partitions | (light, layer) | 8 B per reached texel | warm one; cold shadowmask window of 4 |
| Shadowmask raw fill (`allocate_shadowmask_raw_fill`) | whole atlas | 4 B × edge² × layers, empty space included | whole lightmap layer loop |
| `BlockSectionBuilder` blocks → section → `to_bytes` for cache | whole atlas | ~1.5 B/texel, twice | end of stage |
| Animated `per_chunk` weights | whole map | per animated texel × lights | whole stage; checked against `ANIMATED_ATLAS_VRAM_BUDGET_BYTES` |

- Layer packer (`pack_groups_into_layers`) is next-fit: it never revisits a closed layer. Near-edge split blocks would each close a mostly used layer, inflating the whole-atlas fill, per-layer planes, partitions, `layer_input_hashes`, and distance to the layer cap. Full-edge blocks plus one tight remainder avoid it.
- Predicted stage peak ≈ 4 × edge² × layers + ~57 × edge² + 2 × section bytes.
- Face-split texel overhead ≈ 6 × cut length texels (~0.15% on a 4096-long chart). The risk is cut count under an extreme scale region, not overlap bytes.
- `compiler-implausible-allocation-guard` bounds lengths at format ceilings, not host memory; it does not cover this.
- Memory note `sh-delta-compile-ram-constraints`: lightmap bake partitions cleanly by layer; not a constraint here.

Measurement protocol for the manual RSS rows: fixture movement-feel, kinematic-platform, stress-warren-hallway-inspection; `--release`; default `-j`; cold (`--no-cache`) and warm; metric peak RSS of `prl-build`; baseline main at `19fb3fc40` (hallway); clean the scratch cache dir after.

## 5. Parallelism and determinism

- `pack_cell_blocks` already packs per cell in parallel: sorted `cells.par_iter()` with one governor `enter()` per cell, indexed `collect`; limits, `pack_groups_into_layers` and id assignment run serially after.
- Fused walk (`pipeline/lightmap_stage::bake_fused_prepared`): serial layers × lights; parallel per chart inside (`lightmap_layer::bake_light_layer_for_faces`).
- Rules: `build_pipeline.md` §Build Cache determinism invariant (no order-feeding `HashMap` iteration, order-preserving reductions; BC6H exempt, BC5 not). Governor: one `enter()` at the outermost item boundary.
- Worker-count tests exist for the layer bake, monolithic atlas, shadowmask graph and fixture, fused outputs, and cluster SH spool. `block_layout` has none.
- `drafts/bake-parallelism-large-maps` (owner rulings at `a017567fb`): output bytes unchanged; layer loop stays serial; atlas prep after the SH family. It does not touch packing. Chart-local re-seeding changes every map's lightmap-family bytes once, so that brief's byte baselines must be taken after this lands, or retaken. A late cut leaves its SH timing baselines alone.

## 6. Density survey

Converted to m/texel. Quake unit ≈ 2.54 cm (±25%), Source unit 1.905 cm, Unreal 1 cm, Unity/Godot 1 m.

| Era | Engine / game | Default | m/texel | Source | Confidence |
|---|---|---|---|---|---|
| Classic | Quake 1/2/3 (qbsp, q3map2, ericw-tools) | 16 u/luxel | 0.41 | ericw-tools docs; Q3Map2 wikibook | high |
| Classic | Source / HL2 `lightmapscale` | 16 (8, 4 detail) | 0.30 (0.15, 0.076) | VDC via mirror | high |
| 2014–18 | UE4 density view mode | Ideal 0.2, Max 0.8 texels/cm | 0.05 ideal, 0.0125 max | BaseEngine.ini, LightMapDensityShader.usf | med-high |
| 2014–18 | Unity 5 / 2017 | 40 texels/unit | 0.025 | Unity docs, dev blogs | med |
| 2014–18 | Unity mobile / VR guidance | 5–25 texels/unit | 0.2–0.04 | Android, Microsoft AltspaceVR guides | high |
| 2015–17 | Frostbite Battlefront / Battlefront II | ~50 / ~25 cm, indirect only | 0.5 / 0.25 | O'Donnell GDC18 | high |
| 2016 | DOOM (idTech 6) | lightmaps for static indirect | unpublished | SIGGRAPH16 | high (method) |
| 2023 | Quake II Enhanced | 8 u/luxel | 0.20 | id blog via GWO | high |
| Today | Godot 4 LightmapGI | 0.2 texel size | 0.20 | Godot docs | high |
| Today | Unity 6 | 40 (APV probes alongside) | 0.025 | Unity docs | med |
| Today | DOOM Eternal → Dark Ages | SH lightmaps ~500 MB/level → ray-traced GI | unpublished | Sousa SIGGRAPH25 | high (method) |

- 0.04 sits at the dense end: between UE4 ideal and Unity default; 7–10× denser per axis than Quake/Source; 5× Q2 Enhanced and Godot.
- Indirect-only bakes run coarse (0.25–0.5 m) because direct light is dynamic. Postretro bakes direct shadows into the lightmap, which justifies dense defaults; `plans/done/lightmap-resolution` chose fine density to cure blocky contact penumbrae.
- No published lightmap-texel-to-screen-pixel rule or TAA/upscaler tolerance data found.
- Owner ruling: default stays 0.04.

## 7. Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Atlas preparation with at least one oversize face | chart plan on uncut geometry → pick cuts → cut geometry (sub-faces in place of the parent, each owning its vertices) → recount per-leaf face ranges → rebuild BVH → rebuild CellDrawIndex → encode Cells from the recounted leaves and rerun the cell partition → sub-chart windows → block packing (cluster, cell, sub-block) → vertex block ids and UVs → CellResidencySet from the post-cut Cells | Nothing after the cut reads the pre-cut BVH, leaf ranges, CellDrawIndex or partition |
| P2 | Consumers after preparation that take a BVH | fused bake, shadowmask, animated chunk build, weight maps, cluster directory and pack all run after P1 | All take the rebuilt BVH. Animated chunk ranges index rebuilt leaf slots |
| P3 | Pre-atlas stages: SH family, entity-shadow selection, billboard scatter, ChunkLightList, navmesh, cell visibility | Run before P1, on uncut geometry and the pre-cut BVH | Their sections are byte-identical whether or not any face is cut |
| P4 | No face past the pool edge (the cut runs zero times) | P1 with an empty cut set | Rebuilt leaf ranges, BVH, CellDrawIndex and partition equal the pre-atlas ones. Emitted geometry and BVH match a pre-change build |
| P5 | Padded chart exactly at the pool edge on one axis; the same chart one texel wider | Cut decision per axis | At the edge: no cut. Edge + 1: two sub-charts on that axis, none on the other, extents within one texel |
| P6 | Order of one parent's sub-faces | Window order, rows of v, then u within a row (or the order the executor fixes), contiguous at the parent's slot in its leaf | Every later face index shifts by (sub-faces − 1). The BVH sort key stays monotone within the parent |
| P7 | Non-rectangular oversize face cut on both axes; a cut line through a parent vertex | Clip per window | A window that misses the polygon, or clips to under three vertices, yields no sub-face and no chart. The sub-faces cover the parent exactly |
| P8 | Cut face lit by an animated light | Cut, then weight maps, then the animated shared-vertex and footprint guards | No vertex is shared between sibling sub-faces, and the guards pass |
| P10 | Cell with zero charts | Packing | No block. Contiguity holds trivially, and the runtime cell → block range is empty |
| P11 | Block total at 65,535 and at 65,536, counting every sub-block | Checked at planning, before layer packing and before the bake | 65,535 passes, 65,536 fails by name. With full-edge blocks the 256 bake-layer cap trips first |
| P12 | Two blocks of one cell ready in one drain, with budget for one | Tier, then ascending file offset. The drain always admits its first item | The first installs this drain and the second defers. Faces on the first sample this frame |
| P13 | Cell leaves demand with block A resident and block B mid-read | One recompute marks both dirty | A is freed at the next drain. B's completion is discarded and its buffers are released |
| P14 | Band cell at the pool cap | Refusal and eviction rank by class, priority, lead, then block id. A cell's blocks share one lead | Some of the cell's blocks may be resident and others refused. When the cell becomes visible, the missing blocks read at visible tier and are never refused |
| P15 | Spawn or capture view whose mandatory cell has several blocks | Synchronous install before the first frame; settle query | All of the cell's blocks install before the first frame. Settled is true only once every block of every mandatory cell is resident |
| P16 | Cut face whose sub-charts sit in two blocks: one arrives and the other is evicted or pending | Per-block install | A lit/unlit split along the cut while the second block is missing. It lasts only until the visible-tier read lands |
| P17 | Density edit A → B that moves a cut, then B → A | Three warm builds | Pre-atlas caches hit on both edits. The third build equals the first byte for byte and, within the cache budget, hits what the first wrote |
| P18 | Density edit that moves a cell past the cluster primitive limit | P1 reruns the partition | Cluster membership and block order follow the new partition. Ids 49/50 rebuild and id 51 hits (its key excludes face ranges) |
| P20 | Vertex on a cut line | UV assignment on each side | Both sides map it to the same parent-grid texel position, within vertex UV quantization |

P9 (the full-edge split loop) and P19 (an oversize face on a map with no static lights) wait on owner rulings.
