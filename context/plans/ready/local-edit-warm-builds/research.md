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
  - `reflection_probes/stage_key.rs::cache_key`: one whole-stage entry; see §Reflection probes.
  - `direct_cache_key` (id 35), both billboard scatter keys, `weight_map_cache_key` and the mover gate's `probe_key_prefix` fold the whole geometry; the probe-direct `MemoPrefix` folds `fold_occluding_geometry`.
  - The ShadowmaskAtlas cut-face owner estimate is uncached and re-runs on any `shadowmask_atlas` miss. Its owner sets span a cut parent's sub-faces across layers.
- **Exempt whole-section stages, cold timings** (dev profile, 4-core container):

  | Stage | stress-street | stress-warren-crates |
  |---|---|---|
  | Cell visibility | 0.55 s | 0.06 s |
  | NavMesh | 0.06 s | 1.86 s |
  | ChunkLightList | 0.05 s | 0.20 s |

  Navmesh and cell visibility grow with arena size. A genre-scale map that shows them taking a real share of a local edit's time becomes a follow-up.

## Reflection probes

Read at 0b7054c3, after the `glossy-world-reflections` merge. The stage serves water and, with `--glossy-reflections`, static world faces.

- **One entry.** `bake_or_load_with` looks up one `StageCache` entry holding the whole id-54 section and the share reports. Placement, the glossy census, exclusion classification and transparent faces run before the lookup on every build. A hit skips only `capture_probes`.
- **Key.** `cache_key` folds the geometry in face order, BVH `index_offset`s, each probe's `source_entity`, fluid and id-17 world faces, its facing, transparent and excluded faces and excluded brush indices, the glossy gate, `prefilter::key_bytes`, `TRACER_EPOCH`, and the lighting sections ids 22, 25, 27, 34, 45, 59 and 62, SDF lights and `.prm` keys.
- **Lighting dependence.** `CaptureScene::shade` reads the lightmap, the SH volume with its depth tiles, SH deltas and animated direct, so any light edit misses the entry today.
- **Tracer.** `SceneTracer::nearest` is its own closest-hit loop over the shared BVH. `hit_order` ties by triangle index, in global face order. `FaceMask` holds `index_offset`s from id-17 faces. `capture_fill::nearest_on_face` and `capture_bounds::nearest_held` also tie by index. Capture and box-fit rays run to the level diagonal + 1 m. Shadow rays use the shared `segment_clear`.
- **Global couplings.**
  - `placement_cap.rs::keep_largest` keeps the largest probes by bound area, map-wide; ties go to pool order, derived probes by first id-17 member.
  - `Facings::facing_of` takes the first face seen as a facing's basis, within 1°, not transitively.
  - Plane-height clusters chain over every unclaimed face of a facing.
  - `prefilter.rs::colour_mip_count`: one world-face probe gives every probe the full mip chain.
  - id-54 world bindings carry id-17 face indices and probe ordinals.
- **Per probe (for the 256² brief).** No probe reads another's output. Exclusion scope is the box's footprint, without its floor. Transparent faces span each bound plane map-wide. The prefilter is per probe and deterministic at any worker count.
- **Progress.** One unit is a probe tile row. A hit publishes no total. The prefilter advances no unit.
- **Fixtures.** stress-street and stress-warren-crates hold no fluid or reflection volume, so their timings never exercise this stage.
- **Cost** (`scratch/probe-capture-cost`, `measurements/probe-capture-cost/`; glossy at 27c25a13d, dev profile, `-j 6`, i9-9980HK):
  - The whole stage takes 0.13–0.46 s per cold build, at most 2.2% of the build.
  - Capture costs about 19–52 ms per probe, at 63,504 rays per probe (126² interior × 4 supersamples). The prefilter, run only with a world-face probe, rivals or exceeds capture. Key hashing takes 50–190 ms, more than placement, and runs even under `--no-cache`.
  - Glossy probes miss about half their rays (behind their own reflector plane); water probes hit 97–100%.
  - Re-shading recorded hits without re-tracing costs 46–67% of capture, so records would save at most about half of it.
- **Why per-ray records wait.** A capture ray's answer is richer than an SH hit, so a record would need more than a triangle: the outcome kind (miss, hit, leak guard, fill from a CSG-removed brush side); shadow rays to SDF lights from each hit; material, glow and emissive inputs; and a reach that grows with the map's bounds. At 4 B per ray, records run about 250 KB per probe at 128², about 1 MiB at 256². By extrapolation, 256 probes at 256² cost about 20–50 s of capture, with the prefilter as built growing about 16× per probe. Records don't help the prefilter, since a light edit changes nearly every probe's rows. The 256² brief owns the design, after its prefilter rewrite.
- **IP read for that brief (owner, 2026-11-24).** Tencent WO2026113651A1 claims 10–13 re-shade a probe's stored hit information on a light change. They depend on render-method claim 1, which the compiler never performs, and runtime cube-map G-buffer relighting predates them (§IP). Fences then: a record names what a ray reached and stores no texture, material, normal or colour; re-evaluation runs only in the compiler; the runtime never rewrites or re-shades a tile.

## Footprint

- **Spared sets already pass the default.** `done/occluder-first-partition/measurement/10/round2/rebake/*.log` report spared sets of 2.21–3.59 GiB against the 2 GiB default `--cache-max-size`. `done/mover-channel-bounded-bake/` saw far more.
- **Records at 4 B per ray, before compression** (ray counts from §SH dependency spread): stress-street about 299 MiB, stress-warren-crates about 943 MiB.
- Priced against the budget less the spared set, records at the old default fit in no workspace that has built several maps, so SH would always bake whole. The owner chose to raise the default and keep records unspared, over sparing records with their map or sparing only the current map's.

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
- **Per-light grain for direct layers.** Rejected. The genre's dense, neon lighting puts most brush edits inside several lights' reach, and per-light keys re-bake each of those lights whole. On stress-street a lamp edit sits inside its own lamp's reach and its neighbours'; the 23-instance edit, which touches every lamp, re-baked about 127 s of lightmap. Per-sample grain re-derives each texel's segments from its chart-local seed and re-traces only those meeting changed triangles. It stays exact without stored hits, because direct visibility is any-hit. A sun or other directional light reaches the whole map, so under per-light grain every brush edit would re-bake its whole layer. The first slice measures how many lights a one-instance edit reaches.

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

**Tencent WO2026113651A1, claims 10–13 (reflection-probe records, for the 256² brief).** Read from `done/reflection-probes/ip-check.md` §1 and `done/reflection-probe-fluid-pass-performance/ip-check.md`.
- Claims 10–13 update a probe's stored lighting when an update condition holds, by re-shading stored texture or G-buffer information. They depend on claim 1, a method that parallax-corrects a probe and renders a reflection image. The compiler performs no step of claim 1.
- Prior art: FFXV (SIGGRAPH 2016 talk) cites Far Cry 4 (McAuley, GDC 2015) for cube maps that store a mini G-buffer and relight at run time. Both predate the 2024-11-28 priority. The incremental ray-tracing papers above cover re-evaluating cached rays.
- Fences for that brief: as §Reflection probes, IP read (owner, 2026-11-24).

**Cited-by patent.** NVIDIA US11922567B2 is live. Its claim 1 covers server-computed indirect lighting sent to a client, which does not reach a build cache.

## Rivals, continued (second direction review)
- **Content-keyed per-(light, chart or patch chunk) direct entries over the shaft geometry, with diff records for SH only.**
  - Gains: direct light stays a pure memo, a revert hits for free, and no reach geometry is stored per light.
  - Costs: a whole chart re-traces where per-sample grain re-traces single texels, and the cache runs two models.
  - Not chosen. SH needs the diff model regardless, because its rays are unbounded. Writing every patched entry under its full content key restores free reverts and shared-cache safety inside one model.

## Pin table

Orderings the Acceptance rows cite.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| RW-1 | Chained warm edits | cold A → warm e1 → warm e2 (no cold between) | The e2 build re-traces only e2's direct diff samples and e2's re-trace set; no chunk e1 left unchanged misses; equals cold of A+e1+e2 |
| RW-2 | Tracer or stage version bump | build → bump `TRACER_EPOCH` or a stage version → build, records present, no geometry edit | No pre-bump entry or record serves as a baseline; every sample and ray re-traces; equals cold |
| RW-3 | Record eviction by sweep | build → next build's sweep evicts all or some ray-record chunks → build → build | All evicted: one full SH trace, then local again. Some evicted: at most those chunks' rays re-trace. Equals cold throughout |
| RW-4 | Revert | A → B (success) → A | No eviction between: every entry A hit before hits, nothing re-traced. Sweep evicts A-only entries between: those re-bake, equals cold |
| RW-5 | Coincident faces | two coincident faces, different textures → edit elsewhere that renumbers | Closest-hit winner equal warm and cold, before and after; equals cold |
| RW-6 | Sampling-frame edit | `_lightmap_scale` changed on one brush, no geometry moved | The brush's charts re-trace; other charts hit; equals cold |
| RW-7 | Atlas layer size change | edit elsewhere doubles layer size; one layer's placements unchanged | That layer's encode memo misses; equals cold |
| RW-8 | Pause mid-stage | fresh units → pause N s → resume | ETA after resume equals ETA before pause, in TUI and plain reporter; no ETA countdown while paused |
| RW-9 | Total raised or late; filler units | total published late, or re-published larger mid-stage; units completed without work at stage end | ETA stays defined from fresh rate; filler counts cached; cached + fresh = last total at stage end |
| RW-10 | Telemetry counts | stage begin → finish/skip | Finish/skip events carry final cached/fresh/total; begin and no-total stages carry null, never zero |
| RW-11 | Ceiling crossing | renumbering edit takes map past ceiling → edit back under → local edit | Past: SH whole, no records written, warns by name, direct local. Back: SH whole, records written, no warning. Next: local. Equals cold each time |
| RW-12 | Zero bounce source | every static light dimmed to zero → brush edit → one light restored | No later-bounce records exist after the zero build; the restore build traces the rays it needs; no absent record reads as a miss; equals cold |
| RW-13 | Chunk gone mid-bake | this build looks up baseline → another build's sweep removes a chunk → this build reads it | That chunk's rays re-trace; equals cold |
| RW-14 | Two edits, one build | e1 and e2 in different places → one warm build | Re-trace = union of both re-trace sets and direct diff samples; equals cold |
| RW-15 | Kill mid-record write | build killed after some record chunks, before SH entry → rebuild | Equals cold; re-trace ≤ re-trace set + direct diff samples + rays of chunks failing digest |
| RW-16 | Two worktrees, one map | W1 edits e1, W2 edits e2, built alternately on one cache | Each equals its cold build; each re-traces at most the union of e1's and e2's re-trace sets and direct diff samples |
| RW-17 | Default budget fit | empty cache, default `--cache-max-size` → cold stress-street → warm one-brush edit | Spared set plus ray records priced under the budget; SH local; no ceiling or eviction warning |
| RW-18 | Populated workspace fit | every `content/dev` map built → default `--cache-max-size` → cold stress-warren-crates → warm one-brush edit | Records priced under the ceiling beside every map's spared set; SH local; no ceiling or eviction warning |
| RW-19 | Cross-map record eviction | cold A → build map B at a budget whose sweep evicts A's ray records → A no-op → A one-brush edit → A second edit | No-op: every entry hits, nothing traced, no eviction cost named. First edit: SH traces whole, and the Build Summary names the eviction. Second edit: local. Equals cold throughout |
| RW-20 | Referenced chunks outlive debris | cold A → several warm edits of A, each leaving orphaned chunks → a sweep that must reclaim less than the orphans hold → A edit | No chunk A's newest entry references is evicted; the edit is local; equals cold |
| RW-21 | Referenced chunks outlive every orphaned entry | cold A → warm edit of A → cold B → warm edits of B, orphaning B chunks and lightmap and SH entries younger than A's referenced chunks → a sweep that must reclaim more than A's orphaned chunks hold and less than every map's orphaned entries hold → A edit | No chunk A's newest entry references is evicted; A's edit is local; equals cold |
| RW-22 | Reordered re-save, probe maps | cold glossy-reflections-test (glossy on, area lights present, a probe with exclusions) and cold reflection-lake-test → re-save writing every brush's sides and the brushes in reverse order, the fluid brush's included → warm | The reflection-probe stage re-runs whole and serves no stale entry; id 54 equals the cold build's, its bindings in the re-saved map's numbering |
