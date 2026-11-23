# local-edit-warm-builds — research

Read at 3703dd164. Symbols are cited as found then; re-verify at build.

## Measurements

Dev profile, 4-core container, private cache directory. Each warm variant starts from a copy of the cold baseline's cache.

### stress-street, by edit

| Edit | Wall | Outcome |
|---|---|---|
| Cold (empty cache) | 397 s | DataScript 155 s cold; SH Bake 68 s; Lightmap 124 s |
| No change | 21 s | every stage hits; Packing 10.8 s of it |
| Brush plane point moved 1 ulp | 22 s | every stage hits; PRL byte-identical |
| One lamp light `light` 220 → 230 | 149 s | SH Bake 69 s with 22 of 23 patch layers hit; Lightmap 50 s; 1 of 23 direct deltas |
| One lamp light origin moved 0.01 u | 150 s | as the line above |
| Same light edit in all 23 linked instances | 227 s | SH 68 s; Lightmap 127 s |
| One lamp arm raised 1 u, one instance | 234 s | every stage and every per-light entry misses |
| Same arm edit, all 23 instances | 230 s | every stage misses |

### Lightmap output after the one-brush edit

From `--dump-chart-hashes`, comparing each chart grid's fingerprint and its irradiance-grid hash, matched as multisets:

| | Count |
|---|---|
| Charts before / after | 10,715 / 10,716 |
| Same grid fingerprint | 10,711 |
| Same atlas placement (layer, x, y) | 10,711 |
| Byte-identical irradiance | 10,698 |
| Identical texel area | 98,105,219 of 98,549,570 (99.55%) |

### SH dependency spread

From a temporary recorder over the bounce-one-source later-bounce pass and the probe gather. The edit box is the union of the edited brush's old and new bounds. Seeds are patches inside the box, plus patches whose segment to a static light within range crosses it. Propagation runs over the four later bounces: a patch changes when any recorded hit changes, or when one of its rays crosses the box.

| | stress-street (open roof air) | stress-warren-crates (enclosed rooms) |
|---|---|---|
| Edit | lamp arm +1 u | crate top +16 u |
| Patches | 318,239 | 1,246,212 |
| Patches with any ray crossing the edit | 599 (0.19%) | 7,233 (0.58%) |
| Dirty closure, by later bounce 1 / 2 / 3 / 4 | 5.3% / 62.6% / 95.0% / 100% | 3.0% / 10.0% / 17.7% / 30.5% |
| Valid probes | 226,486 | 653,371 |
| Probes with any ray crossing the edit | 589 (0.26%) | 12,487 (1.91%) |
| Probes whose value changes | 100% | 47.3% |
| Rays: patch / probe | 20.4 M / 58.0 M | 79.8 M / 167.3 M |
| Records at 8 B per ray | 598 MiB | 1,885 MiB |
| SH Bake, cold | 82.5 s | 229.7 s |

Reading:
- Re-gathering only the dirty closure saves nothing in an open map and about half in an enclosed one.
- Re-tracing only crossing rays leaves under 2% of the tracing in both maps.

The rival, a cell-bounded closure, needs a "can see" relation. The only sound one is id 46's portal components, and those collapse to the whole map in any sealed level. A tighter relation is the sightline PVS that `build_pipeline.md` §Id-51 consumers forbids.

## Why keys miss: global identity

- **BSP numbering.**
  - `select_splitter` (`partition/brush_bsp.rs`) scores every brush in the region, so a moved brush can change upper splits.
  - `make_leaf` numbers leaves depth-first, and `CellTree::build` numbers cells in leaf order.
  - `build_leaf_ordered_faces` (`geometry.rs`) orders faces by leaf. Vertex indices and `index_offset` are running totals over that order.
- **Lightmap layer key** (`lightmap_layer/cache_keys.rs`).
  - `LayerKeyContext::light_prefix` folds `atlas_layout_fingerprint`, which covers every chart's frame and placement plus the block table.
  - `geometry_slice_hash` folds `BvhPrimitive::sort_key` (`primitive_sort_key(texture, leaf, index_offset)`, offset masked to 20 bits) and global vertex indices.
  - The payload `LayerTexel.idx` is `y * atlas_width + x` in bake-layer coordinates, so a placement change breaks the payload, not only the key.
  - Visibility values and `chart_raster::chart_texel_seed` are chart-local.
  - The fingerprint also carries chart sampling frames, which `_lightmap_scale` can change; a local key still needs the reached charts' frames.
- **Light fields the payload reads.** Record presence is `contribution_covers_shadowmask` (> 1e-12) over `color * intensity * ndotl * atten`. No stored value depends on intensity or colour.
- **Packing order.** `pack_cell_blocks_within` packs each cell on its own. `pack_groups_into_layers` is a sequential MaxRects pass in cluster order and never revisits earlier layers. `choose_layer_dim` is global.
- **SH seeds and grid.**
  - `patch_seed_id(face, cell)` (`surface_radiance/patches.rs`) feeds `patch_frame` and the bounce-one soft-visibility seed.
  - `probe_grid_layout_of` anchors to the world AABB.
  - `probe_is_valid` reads the global exterior flood.
  - `DepthTileSelector` keeps a global top-K ranked by score and then linear probe index.
  - `compact_section_from_dense_atlas` offsets tiles by valid-probe rank.
- **Ray hits.** `closest_hit` keeps the first visited of equal-distance hits, and visit order is BVH order sorted by `primitive_sort_key`. Gather and bounce rays are unbounded. A hit point is `origin + dir * distance`. Later-bounce rays are fixed per patch across bounces (`patch_frame`, `cosine_directions`).
- **Whole-world keys.**
  - `DirectDeltaMemo::key` folds `fold_occluding_geometry`, though `bake_probe_direct_rgb` traces only `segment_clear` within reach.
  - `delta_sh_entry_cache_key` and `chunk_light_list_cache_key_with_version` fold `geometry_content_hash`.
  - `cell_residency_set_cache_key` folds the whole portal, locator and cell data.
  - `mover_probe_bounce`, `mover_absent_bounces` and `sh_base_bake_time` fold id 34's `volume_key`.
  - `reflection_probes/stage_key.rs::cache_key` hashes the packed id 34 and others.

## Fixed warm costs

- **After a `lightmap_section` miss** (`pipeline/lightmap_stage.rs::bake_fused_with_movers`), every (layer, light) partition is loaded and folded. Each layer is dilated and BC6H-encoded (`encode_atlas_layer`, single-threaded) inside one consumer permit. `StepDetector::detect` runs on every partition.
- **Packing** (`finalized_publication.rs::write_finalized_prl`) runs, in order:
  - `build_cluster_payload_spool`: a serial per-cluster BC6H + zstd + blake3 loop;
  - the bound resolutions;
  - section serialisation;
  - write, readback validation, publish.
  None of it is cached.
- **Uncached after id 34:** drop policy, `apply_coarsen_classification`, `classify_base_levels`, `apply_valid_probe_compaction`, the payload cap, the id-34 BC6H encode, and `build_finalized_cluster_metadata`.

## Progress accounting today

- **Units per stage.**

  | Stage | One unit |
  |---|---|
  | SH Bake | a patch chunk per layer and per later bounce, plus one per probe |
  | Delta and direct-delta bakes | a CSR entry |
  | Lightmap | a placement × light pair |
  | ShadowmaskAtlas | a partition, plus one for the encode |
  | Cell Residency | a camera cell |
  | AnimWeightMaps | a (face, light) on a miss, a chunk on a hit |
  | Reflection Probes | a probe × tile row |

- **Hits.** Hits advance the same counter as fresh work. Whole-stage hits advance the full total at once. A `reflection_probes` hit never publishes a total. Culled lightmap charts advance as complete. The mover direct half's handle is never declared. `DeltaShCacheTally` is computed and dropped.
- **ETA** is elapsed since stage start × remaining / done, in both `RemainingEstimate::observe` and `PlainReporter`.
- **Footer** (full layout): `done/total pct%`, then `[█…░…]`, then `Est. remaining Ns`. Step column 32 wide; compact layout applies below 68×18.
- **Colours:** PRIMARY Cyan, SECONDARY Blue, MUTED DarkGray, DIVIDER Gray+DIM, failure Red+BOLD. No `NO_COLOR` handling.
- **Elsewhere.** `PlainReporter` prints percent and ETA lines to stderr. `BuildSummary` prints seconds only. Telemetry `BakeSummaryEvent` / `SummaryRow{name, ms, stage}` lives in level-compiler `telemetry.rs`; the leaf crate's payload is opaque JSON.
- **Presentation rules** exist only in `plans/done/level-compiler-tui/index.md` (Task 4, Presentation and layout behavior).

## Progress presentation: interaction-design notes

- **Who reads it.** An author waiting on an edit-build loop. Their questions, in order:
  1. Is it doing real work, or reusing?
  2. How much is left?
  3. Did my edit cost what I expected?

  The bar answers 1 at a glance. The numbers answer 2. The Build Summary answers 3 after the fact.
- **Segment order:** cached, then fresh, then pending. Cached work is front-loaded in spirit, being free. Fresh sits next to pending, so the growing edge is where attention goes. The bar shows proportions, not time order: hits and fresh units interleave in time.
- **Emphasis.** Fresh takes the primary accent: it is the cost. Cached takes the secondary accent and a lighter glyph, so a fully cached stage reads as quiet, not as alarm.
- **Never colour alone.** Each segment has its own glyph shape, so the bar reads under `NO_COLOR`, on monochrome terminals, with colour-vision deficiency, and in plain-text logs pasted into an issue. `NO_COLOR` (any non-empty value) drops colour and keeps glyphs.
- **Numbers.**
  - SI, three significant digits: `870`, `12.3k`, `1.24M`. Lowercase `k` per SI; uppercase `M` and `G`, so a count never reads as minutes beside `37.7s`.
  - Exact below 1,000, so a small count never shows a false decimal.
  - Rounding carries: 999,500 shows as `1.00M`, never `1000k`.
- **Width degradation** keeps meaning before precision: labelled counts with a unit noun, then bare `cached/fresh/total`, then percent only, then the bar alone.
- **ETA.** Rate comes from fresh units over fresh time, and pending units are projected at that rate. It is pessimistic when later units hit, and that is chosen over optimism: an ETA that drops is welcome, one that grows erodes trust. With nothing fresh yet, no ETA is shown, rather than a made-up number.
- **Units differ by stage**, so cross-stage totals are never summed. The Build Summary compares stages by cached share and time.

## Rivals considered

- **Approximate warm SH.** Rejected. It reopens `done/surface-radiance-cache/` "Warm id 34 equals cold", which the owner keeps.
- **Re-gather the dirty closure.** Rejected on measurement: 100% in open maps.
- **Stabilise the BSP instead of decoupling identity.** Rejected as the primary lever. It is a partition-quality trade, and identity decoupling is needed anyway for packing and seeds.
- **8 B per ray (triangle and distance).** The owner chose 4 B (triangle, with distance re-intersected), compressed and streamed.
- **Stable identity only, SH left as one entry.** Rejected. SH costs 68–69 s on every light or geometry edit (§Measurements), so the 45 s edit targets fail without SH locality.
- **Per-light grain for direct layers.** Rejected. All of stress-street's static lights are the linked lamps, and a lamp edit sits inside every lamp's reach, so per-light keys still re-bake about 127 s of lightmap work for the owner's case. Per-sample grain re-derives each texel's segments from its chart-local seed and re-traces only those meeting changed triangles. It stays exact without stored hits, because direct visibility is any-hit.

## IP

Engineering research on 2026-11-23; not legal advice.

**Crytek US20140327673A1.**
- Application 13/887,266, priority 2013-05-03, published 2014-11-06.
- Abandoned 2017-01-23 for failure to respond to an office action: the "STCB" legal event at https://patents.google.com/patent/US20140327673A1/en. That status is Google's mirror of USPTO data; Patent Center was not reachable to confirm it.
- No foreign family and no continuation, and none can follow a dead parent.
- The A1 claims are framed as real-time: claim 1 in its preamble, claim 11 "during run time", claims 13 and 21 a GPU. The spec ([0021]) defines real-time as 15–60 fps.
- Its [0018] discloses recomputing only the intersections tied to a modified sample. As a 2014 publication, that is prior art.

**Older art for the design.** Read from snippets; full texts were outside the network allowlist.
- Murakami & Hirota, "Incremental ray tracing" (1989 workshop; Springer 1992).
- Jevans, "Object space temporal coherence for ray tracing" (GI 1992).
- Brière & Poulin (SIGGRAPH 1996).
- Bala, Dorsey & Teller (EGWR 1999).
- Chen, "Incremental radiosity" (SIGGRAPH 1990).
- Drettakis & Sillion (SIGGRAPH 1997).

**Cited-by patent.** NVIDIA US11922567B2 is live. Its claim 1 covers server-computed indirect lighting sent to a client, which does not reach a build cache.
