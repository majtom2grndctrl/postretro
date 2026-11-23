# local-edit-warm-builds

Brief · resumable · reads: `context/lib/build_pipeline.md` §Build Cache, §Progress reporting, controls, and logging, §Id-51 consumers · `done/surface-radiance-cache/` · `done/incremental-bake-per-element/` · `done/level-compiler-tui/` · `context/research/gi-rtx-lite/ip-check.md` · read at 3703dd164

## Problem
**Basis.** Owner-observed defect in the authoring loop: an edit's rebuild costs what the whole map costs, and the build cannot show why. After a TrenchBroom linked-group update, prl-build rebuilds as if the cache were empty. On stress-street, raising one brush in one lamp instance costs 234 s warm against 21 s for a no-op. Every cache stage misses, though 99.5% of lightmap texels come out byte-identical (`research.md` §Measurements).

**Cause.** Cache keys, seeds and tie-breaks are built from global identity: BSP leaf and cell order, global face and vertex indices, a whole-atlas fingerprint, and a probe grid anchored to the map's bounds. Any edit renumbers them. On top of that, the SH base volume is one entry whose gather re-traces every ray after any light or geometry edit. Linked groups only make this frequent: one edit lands in every instance. Even a no-op pays fixed passes that no cache covers: Packing alone is half of it.

**Why it can't be seen.** Hits and fresh work share one progress counter, so the author can't see what an edit cost, and hits make the ETA wrong.

**Done when:**
- A warm build after a local edit re-computes only the samples and rays the edit can reach, and stays byte-identical to a cold build of the edited map.
- Every stage reports cached, fresh and total work in the TUI, the plain reporter, the Build Summary and telemetry.

## Decisions
- **Exactness holds.** Warm equals cold under the Determinism invariant (`build_pipeline.md` §Build Cache). Its carve-outs stay as they are. No approximate warm path: the retired one stays retired (`done/surface-radiance-cache/`).
- **Stable geometry identity is the foundation.**
  - Faces, charts, patches, triangles and probes are identified by content: the source brush side plus the fragment's world-space geometry. Traversal order never identifies them.
  - Every key, seed and tie-break that reads a global index today reads the stable identity instead. That covers patch and chart seeds, soft-visibility seeds, and closest-hit ties, which break by identity rather than BVH visit order.
  - The probe grid anchors to a world-fixed lattice, so growing the map's bounds moves no existing probe.
  - Baked bytes change once. No runtime format change beyond what identity forces (pre-stable).
- **Direct layers localize per sample.** This covers lightmap partitions, the ShadowmaskAtlas raw source and SH patch-direct layers.
  - Each is cached per light, keyed on the light's geometric fields and the shared bake parameters. Intensity and colour enter only through record presence, so a tint or dim edit re-traces nothing unless it crosses the presence threshold.
  - Each entry records the reach geometry it was baked against. A warm build diffs that against the current reach geometry and re-derives every sample's shadow segments from its chart-local seed (`chart_texel_seed`, already world-frame-keyed).
  - It re-traces only samples on changed charts or patches, and samples whose segment meets a changed triangle. Visibility is any-hit, so every other sample is exact as stored. No hit records are kept.
  - An embedded-emitter flag flip re-bakes that light whole.
- **Lightmap payload and encode.**
  - Cached partitions store chart-relative texels, remapped through the current placement at fold.
  - Encoded atlas layers and ShadowmaskAtlas output are memoised per atlas layer, keyed on that layer's placements, its contributing partitions and the encode parameters. Each carries its texel-step report.
  - Suns keep a whole-world reach; that is their real dependency.
- **SH locality through recorded ray hits.**
  - The cache keeps every later-bounce patch ray's and probe gather ray's hit as a stable triangle identity, 4 B per ray. Distance is re-intersected exactly. Records are zstd-compressed per chunk and streamed, never held whole.
  - After an edit, a warm bake re-traces only:
    - rays whose recorded segment crosses changed geometry (old ∪ new);
    - rays from new or changed patches and probes;
    - rays of probes whose validity changed.
  - Everything else is re-evaluated from the records, in the cold bake's order.
  - The depth-tile top-K is reselected every build from per-probe scores.
  - Rival rejected on measurement: re-gathering the dirty closure reaches 100% of an open map (`research.md` §SH dependency spread).
- **The cache contract changes.** A diff-patched entry is no longer a pure memo whose key covers every input, so §Build Cache (Key composition, Determinism invariant) states the new contract:
  - Every entry, patched or fresh, is written under the full content key of what it now holds. An exact key match still hits outright, so reverting an edit is free and maps sharing a cache never overwrite each other.
  - On a miss, the bake looks up the nearest baseline for the same stable identity, diffs against it, and writes a new entry. It never mutates the baseline.
  - Each record chunk names the baseline geometry digest it was traced on. A chunk whose digest disagrees is a miss, so a killed or mixed write never serves.
  - Rival: content-keyed per-(light, chart) direct entries hashing the shaft geometry, with diffs for SH only. It keeps the old memo for direct light but re-traces whole charts and runs two cache models. One model is chosen because SH needs the diff model anyway, its rays being unbounded (`research.md` §Rivals).
- **The tracer contracts its locality.** §Ray traversal's Answer contract gains a rule: a ray's answer depends only on triangles whose τ-padded bounds meet its segment, and ties break by stable identity. Every crossing test relies on that rule. A test pins it, so a tracer change cannot silently break warm equals cold.
- **No visibility relation is added.** SH locality comes from ray records, never from a cell-to-cell relation. The no-PVS rule stands (`build_pipeline.md` §Id-51 consumers).
- **Cascade stages go per-entry, keyed on the stable identities they read:**
  - indirect (id 27) and direct (ids 41, 45) SH deltas: reach-local geometry, not the whole world;
  - mover pose memos: the inputs each pose reads, not id 34's whole key;
  - reflection probes: per probe;
  - cell residency: per camera cell, with its walk's dependency set recorded;
  - chunk light lists: per chunk.
- **Fixed warm costs are owned here.**
  - Packing's cluster payload spool is memoised per cluster chunk and parallelised.
  - The uncached passes after id 34 are memoised or parallelised until the targets hold.
  - Byte output is unchanged.
- **The ray-record footprint is priced before tracing.**
  - Ray counts are known in advance (patches × rays per patch, plus valid probes × rays per probe), so the record size is priced at the SH gate, as the working-set gate prices memory (`development_guide.md` §1.4).
  - A map whose records would pass the per-map ceiling bakes its SH as one whole entry, as today. It stays exact, and the Build Summary names the warning.
  - Records are exempt from the spared set: they are evicted first, oldest first. Losing them costs one full SH trace, never correctness.
  - The Build Summary reports the footprint against the ceiling.
- **This reverses documented choices.**
  - "A geometry edit re-keys every layer" is retired, from `build_pipeline.md` §Build Cache and from the decisions of `done/incremental-bake-per-element/` and `done/surface-radiance-cache/`. Those plans named finer geometry locality as future work; this brief is that work.
  - `incremental-bake-per-element` also ruled out primary-ray-hit caching. The SH ray records lift that ruling.
- **IP guard (owner).** The ray records follow incremental-ray-tracing art (Murakami & Hirota 1989–92, Jevans 1992). Crytek US20140327673A1 is abandoned (`ip-check.md`, `research.md` §IP). The records keep that shape:
  - Each record is a memoised ray cast: (sample, fixed ray index, geometry epoch) → hit triangle. It never stores a patch or sample index; patches are found at evaluation from the triangle and the re-intersected point.
  - No per-sample array of hit samples, and no per-sample light list.
  - Invalidation is by segment against changed-geometry bounds.
  - The existing guards hold (`done/surface-radiance-cache/`):
    - ray directions stay fixed and are never chosen or weighted by a record;
    - records carry no per-light coefficient;
    - the cache never reaches PRL.
- **Progress accounting.**
  - Each stage counts cached and fresh units separately. A unit the bake proves needs no work, such as a culled chart, counts as cached ("not computed this build").
  - Every stage reports, including whole-stage memo hits and `reflection_probes` hits.
  - ETA is pending units × fresh-unit rate, measured over fresh time only. With nothing fresh yet, no ETA is shown.
  - Counts are display-only; they never feed bake output (§Progress reporting).
- **Progress presentation.** Rationale is in `research.md` §Progress presentation.
  - **The bar** runs cached, then fresh, then pending. Each segment differs in glyph as well as colour: cached `▓` in the secondary accent, fresh `█` in the primary, pending `░` muted.
  - **Colour.** Theme-relative named colours only. `NO_COLOR` drops colour and keeps the glyphs.
  - **Counts** read cached / fresh / total in SI with three significant digits: exact below 1,000, then lowercase `k`, uppercase `M`/`G`, rounding carried (`999,500` → `1.00M`).
  - **Narrow widths** drop labels and the unit noun first, then the counts, then the percent. The bar goes last.
  - **Sidebar.** A whole-stage hit marks its step cached with a word or glyph, not colour alone.
  - **Plain reporter** prints the same three counts on its progress and stage-end lines.
  - **Build Summary.** Each stage row gains its cached share. The totals gain fully cached stages and fresh compute time.
  - **Telemetry.** Stage and summary events gain cached and fresh counts.
- **Conventions become durable.** These presentation rules, plus the ones in `done/level-compiler-tui/` (theme-relative palette, one primary and one secondary accent, quantised spacing, progress-driven liveness, compact view), land in `build_pipeline.md` §Progress reporting.
- **Non-goals.**
  - TrenchBroom-specific normalisation. Its 6-digit origin rewrite in rotated instances legitimately re-bakes the lights it moves.
  - Approximate or preview SH, and any PVS or tighter id 46.
  - Mover bake fidelity rules.
  - Runtime streaming and residency behaviour.

## Acceptance

### Automated

**Warm equals cold.** For each case, a warm build from the baseline's cache equals a cold build of the edited map, byte for byte, with BC6H sections compared before encode:
- [ ] a brush moved inside one room;
- [ ] a brush added earlier in file order, renumbering far faces;
- [ ] an edit that changes an upper BSP split;
- [ ] an edit that grows the map's bounds;
- [ ] a seal turned into a leak, and back;
- [ ] a light added, deleted, moved, re-tinted, and dimmed to zero;
- [ ] a sun light moved;
- [ ] a linked group updated through TrenchBroom's transform-and-reclone path, rotated instances included;
- [ ] two coincident faces with different textures, then an edit elsewhere that renumbers faces (RW-5);
- [ ] a `_lightmap_scale` change on one brush, moving no geometry (RW-6);
- [ ] an edit elsewhere that changes the atlas layer size, leaving some layer's placements unchanged (RW-7);
- [ ] every static light dimmed to zero, then a brush edit, then one light restored (RW-12).
- [ ] Two local edits, each built warm in turn: the second build re-traces only the samples and rays its own edit reaches, and equals a cold build of the twice-edited map (RW-1).
- [ ] Two edits in different places between builds re-trace exactly the union of their crossing sets, and the build equals cold (RW-14).

**Locality.**
- [ ] After a local brush edit that renumbers faces and moves other charts in the atlas, a light whose reach excludes the edit hits every lightmap entry it had.
- [ ] A renumbering-only edit, with no geometry near any light, hits every per-light lightmap, SH delta and mover entry.
- [ ] A tint or intensity change with no presence change re-traces no lightmap visibility.
- [ ] After a brush edit that meets only a texel's footprint clip segment or lead-in, never its light-ward segments, that texel re-traces, and the warm build equals cold.
- [ ] An edit that grows the map's bounds moves no existing probe, and its SH re-trace is exactly the crossing set.
- [ ] A local edit re-bakes only the reflection probes, camera cells and chunk light lists whose recorded inputs it touches; every other one hits.
- [ ] After a local edit, every atlas layer whose placements and contributing partitions are unchanged hits its encode memo, equals the cold build's encoded layer byte for byte, and replays the cold build's texel-step warnings.
- [ ] A cluster payload memo hit equals the cold build's encoded payload byte for byte.
- [ ] After a brush edit inside a light's reach, that light re-traces only the samples on changed charts or patches, plus samples whose shadow segment meets the changed triangles. The build reports the count.
- [ ] After a local edit, the SH rays re-traced are exactly the crossing set; the build reports the count.

**Refusals.**
- [ ] A stale entry from an older stage version, or a truncated or corrupt ray record, misses and re-bakes. It never serves.
- [ ] After a tracer-epoch or stage-version bump, no entry or ray record written before it serves as a baseline: every sample and ray re-traces, and the build equals cold (RW-2).
- [ ] A build killed while writing ray records, before its SH entry lands: the next build equals cold and re-traces at most the crossing set plus the rays of chunks that fail their digest (RW-15).
- [ ] An edit far from a cached ray's segment never re-traces it; an edit across it always does.
- [ ] A tracer test pins the locality rule: an answer never changes when a triangle whose padded bounds miss the segment is added or removed, including when two triangles tie at the hit distance and the change renumbers every other triangle.

**Footprint.**
- [ ] The Build Summary reports the cache footprint with the ray records.
- [ ] A map priced past the ray-record ceiling bakes SH whole, byte-identical to cold, and warns by name.
- [ ] Evicting a map's ray records through the start-of-build sweep, at a budget too small to keep them, costs one full SH trace on its next build; the output stays byte-identical, and the build after that is local again. Evicting only some of its record chunks re-traces at most those chunks' rays (RW-3).
- [ ] Reverting an edit, with no eviction between the builds, hits every entry it hit before the edit, with nothing re-traced; with eviction between, it re-bakes only what was evicted and equals cold (RW-4).
- [ ] Two maps sharing one cache directory never serve each other's entries.
- [ ] A ray-record chunk that is missing, or whose baseline digest disagrees with its entry, misses: its rays re-trace and the build equals cold, including when another build's sweep removes it mid-bake (RW-13).
- [ ] An edit that renumbers faces and takes a map past the ray-record ceiling, then one that brings it back: each equals cold, direct lighting stays local in both, and the first build back under the ceiling traces SH whole and the next is local again (RW-11).
- [ ] Ray records stream per chunk: a warm SH bake's measured peak stays within the SH base gate's prediction, with at most one record chunk per worker resident.

**IP guard.**
- [ ] A ray record holds one hit-triangle identity per fixed ray, 4 B before compression, and no patch, sample or light value.
- [ ] Gather and bounce ray directions are identical with ray records present, absent or corrupted.
- [ ] No code that writes PRL reads ray records (grep gate).

**Progress accounting.**
- [ ] Accounting is correct for each kind of stage: all hits, all fresh, mixed, whole-stage hit, zero units, a total published late, and a total raised mid-stage. Each unit lands in exactly one of cached or fresh; units a stage completes without work count cached; at stage end, cached plus fresh equals the last published total (RW-9).
- [ ] Every stage, including a `reflection_probes` hit and the mover direct half, publishes a total.
- [ ] ETA follows the fresh rate in both the TUI and the plain reporter, and time spent paused never counts as fresh time. An all-cached stage shows no ETA (RW-8).

**Presentation.**
- [ ] The bar renders correctly in full and compact layouts at every width from 2 cells up.
- [ ] Under `NO_COLOR` the segments stay distinct by glyph alone.
- [ ] The number format covers 0, 999, 1,000 → `1.00k`, 999,500 → `1.00M`, and the largest count.
- [ ] Plain reporter lines, the Build Summary's cached share and the telemetry events carry the same counts the TUI shows. Finish events carry the final counts; a stage with no published total carries null counts (RW-10).
- [ ] The Build Summary's totals name the fully cached stages and the fresh compute time, and the summary telemetry event carries both.

**Durable record.**
- [ ] `build_pipeline.md` states the new cache contract (§Build Cache), the tracer locality rule (§Ray traversal), and the progress presentation rules with the carried TUI conventions (§Progress reporting); no `context/lib/` doc still says a geometry edit re-keys every layer.

### Manual
- [ ] stress-street (dev profile, the 4-core reference container, warm from a copy of the baseline cache): no-op ≤ 12 s, one-brush edit ≤ 45 s, one-light edit ≤ 45 s, and the 23-instance linked-lamp brush edit ≤ 45 s.
- [ ] stress-warren-crates (same profile, container and baseline-cache copy): the no-op, one-brush edit and one-light edit, each reported against its stress-street target.
- [ ] The owner watches a warm local-edit build in light and dark terminal themes, under `NO_COLOR`, and in the compact layout. Glyph and accent choices are tuned with the owner at that point.

## Path
- **First slice.** Stable identity for faces, patches and triangles, plus the SH ray record, proven on stress-street. Measure:
  - warm equals cold after the arm edit;
  - re-trace count against the crossing count in `research.md`;
  - re-evaluation time;
  - record size at 4 B per ray.
  This falsifies the riskiest assumptions: that re-evaluation is cheap and that a re-intersected distance is bit-exact.
  - If the distance is not bit-exact, records grow to 8 B (triangle plus distance). That is an owner check on the footprint.
  - If re-evaluation is not cheap, stop and report to the owner before building the rest of the SH half.
- **Seams:**
  - `build_leaf_ordered_faces`, `primitive_sort_key` and `closest_hit` (identity and ties);
  - `patch_seed_id` and `probe_grid_layout_of`;
  - `LayerKeyContext` and `geometry_slice_hash`;
  - `LayerTexel` and `IncrementalLayerAccumulator::fold_partition`;
  - `pack_groups_into_layers`;
  - `later_bounce_sum`, `gather_patch` and `PatchGather::sample`;
  - `DepthTileSelector`;
  - `build_cluster_payload_spool`;
  - `StageProgress`, `RemainingEstimate` and `progress_bar_line`.
- **Precedents:**
  - the warm-cache harness in `research.md` §Measurements (copy the baseline cache per variant);
  - `--dump-chart-hashes` for lightmap comparisons;
  - the mover channel's near-field domain, as prior region-limited SH work.
- **Split first:**
  - `pipeline.rs` before the progress plumbing;
  - `geometry.rs` before identity;
  - `pipeline/lightmap_stage.rs`, `lightmap_layer.rs` and `tui/tui_render.rs` before their changes.
- **Measurement order.** Packing dominates the no-op; measure it before deciding memoise versus parallelise.

## Open questions
- Exact form of the content-derived identity: which brush-side and fragment fields, and how to handle collisions and duplicated geometry — **delegated**.
- The per-map ray-record ceiling value, set from stress-fixture measurement — **delegated**.
- Whether stable identity alone keeps atlas placement steady enough, or packing also needs anchoring (for example per-cluster regions) — **delegated**, measured against the per-layer memo hit rate.
