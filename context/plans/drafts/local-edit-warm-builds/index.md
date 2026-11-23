# local-edit-warm-builds

Brief · resumable · reads: `context/lib/build_pipeline.md` §Build Cache, §Ray traversal, §Source-format neutrality, §Compiler pipeline step 12 (Exact skips), §Progress reporting, controls, and logging, §Id-51 consumers · `docs/level_design.md` §Compiling Your Map · `done/surface-radiance-cache/` · `done/incremental-bake-per-element/` · `done/level-compiler-tui/` · background: `context/research/gi-rtx-lite/ip-check.md` · read at 3703dd164

## Problem
**Basis.** Owner-observed defect in the authoring loop: an edit's rebuild costs what the whole map costs, and the build cannot show why. After a TrenchBroom linked-group update, prl-build rebuilds as if the cache were empty. On stress-street, raising one brush in one lamp instance costs 234 s warm against 21 s for a no-op. Every cache stage misses, though 99.5% of lightmap texels come out byte-identical (`research.md` §Measurements).

**Cause.** Cache keys, seeds and tie-breaks are built from global identity: BSP leaf and cell order, global face and vertex indices, a whole-atlas fingerprint, and a probe grid anchored to the map's bounds. Any edit renumbers them. On top of that, the SH base volume is one entry whose gather re-traces every ray after any light or geometry edit. Linked groups only make this frequent: one edit lands in every instance. Even a no-op pays fixed passes that no cache covers: Packing alone is half of it.

**Why it can't be seen.** Hits and fresh work share one progress counter, so the author can't see what an edit cost, and hits make the ETA wrong.

**Done when:**
- A warm build after a local edit re-computes only the samples and rays the edit can reach, and stays byte-identical to a cold build of the edited map.
- Every stage reports cached, fresh and total work in the TUI, the plain reporter, the Build Summary and telemetry.

## Decisions
- **Exactness holds.** Warm equals cold under the Determinism invariant (`build_pipeline.md` §Build Cache), with its carve-outs as they are. The retired approximate warm path stays retired (`done/surface-radiance-cache/`).
- **Stable geometry identity is the foundation.**
  - Faces, charts, patches, triangles and probes are identified by content: the source brush side plus the fragment's world-space geometry. Traversal order never identifies them. A face re-cut because an edit moved a BSP split is a changed face: re-traced exactly, and named in the Build Summary as a re-partition.
  - Triangle corners are emitted in a canonical order, in cold and warm bakes alike. Re-saved but unchanged geometry keeps its identity, and a re-intersected distance stays bit-exact.
  - Every key, seed and tie-break that reads a global index today reads the stable identity instead: patch and chart seeds, soft-visibility seeds, and closest-hit ties.
  - The probe and affinity grids anchor to one world-fixed lattice, snapped at the affinity grid's multiple of probe spacing, so growing the map's bounds moves no existing probe or affinity cell. The chunk light-list grid keeps its bounds anchor and deliberate half-cell pad; that stage is cheap to re-run.
  - Baked bytes change once. No runtime format change beyond what identity forces (pre-stable).
- **Lightmap and shadowmask light localize per sample.**
  - Partitions are cached per light, keyed on the light's geometric fields and the shared bake parameters. Intensity and colour enter only through record presence.
  - Each entry records the reach geometry it was baked against. A warm build diffs that against the current reach geometry and re-derives every texel's segments (light-ward, footprint clip and lead-in) from its chart-local seed.
  - It re-traces only texels on changed charts, texels whose segment meets a changed triangle, and texels in tiles whose exact-skip cull verdict changed while a changed triangle lies within that tile's cull query. Visibility is any-hit, so every other texel is exact as stored.
  - An embedded-emitter flag flip re-bakes that light whole.
  - Partitions store chart-relative texels, remapped through the current placement at fold.
  - Encoded atlas layers and ShadowmaskAtlas output are memoised per atlas layer. The key covers the layer's placements, its contributing partitions, those lights' photometric fields and the encode parameters. Each memo carries its texel-step report.
- **SH patch-direct layers stay keyed on the whole light and localize per sample.** They store radiance, which bakes colour and intensity in: the `done/surface-radiance-cache/` IP guard (Tencent '451). A tint edit re-shades that light's patches. With the light unchanged and its reach geometry changed, the layer re-traces only patches on changed geometry and samples whose segment meets a changed triangle, by the same diff as the lightmap. Its key folds only its reach geometry, as the lightmap partition's does.
- **SH later bounces and the gather localize through recorded ray hits.**
  - The cache keeps every later-bounce patch ray's and probe gather ray's hit as a stable triangle identity, 4 B per ray. Distance is re-intersected exactly. Records stream per chunk and are never held whole.
  - A warm bake re-traces only:
    - rays whose recorded segment crosses changed geometry (old ∪ new);
    - rays from new or changed patches and probes;
    - rays of probes whose validity changed.
  - Every other ray is re-evaluated from its record, in the cold bake's order. The depth-tile top-K is reselected every build.
  - Re-gathering the dirty closure was rejected on measurement (`research.md` §SH dependency spread).
- **The cache contract changes.** §Build Cache (Key composition, Determinism invariant) states it:
  - Every entry, patched or fresh, is written under the full content key of what it now holds. An exact match hits outright, so a revert is free.
  - On a miss, the bake diffs against the nearest baseline with the same stable identity, stage version and tracer epoch, and writes a new entry. It never mutates the baseline.
  - Each record chunk names its baseline geometry digest. A patched entry references each unchanged chunk by that chunk's own digest and rewrites nothing. A chunk whose digest disagrees with the entry's reference is a miss.
  - One diff model serves direct and SH alike (`research.md` §Rivals).
- **The tracer contracts its locality.** §Ray traversal's Answer contract gains a rule: a ray's answer depends only on triangles whose τ-padded bounds meet its segment, and ties break by stable identity.
- **No visibility relation is added.** The no-PVS rule stands (`build_pipeline.md` §Id-51 consumers).
- **Cascade stages go per-entry on the stable identities they read:**
  - indirect (id 27) and direct (ids 41, 45) SH deltas, over reach-local geometry;
  - mover pose memos, over the inputs each pose reads;
  - reflection probes, per probe;
  - cell residency, per camera cell, with its walk's dependency set;
  - chunk light lists, per chunk.
- **Fixed warm costs are owned here.** Packing and the uncached passes after id 34 are memoised or parallelised, as measurement decides. Byte output is unchanged.
- **The ray-record footprint is priced before tracing.**
  - The per-map ceiling is priced against `--cache-max-size` less the spared set, so the budget never holds records it cannot keep.
  - A map priced past it bakes SH as one whole entry, exact, with a named warning.
  - Ray records are exempt from the spared set and evict with other unspared entries, oldest first. When a map's records are evicted, its Build Summary says SH traced whole because of it.
  - The Build Summary reports the footprint against the ceiling.
- **This reverses documented choices.**
  - "A geometry edit re-keys every layer" leaves `build_pipeline.md` §Build Cache and the decisions of `done/incremental-bake-per-element/` and `done/surface-radiance-cache/`; both named finer geometry locality as future work.
  - The ray records also lift `incremental-bake-per-element`'s ruling against caching primary-ray hits.
- **IP guard (owner).** The ray records follow incremental-ray-tracing art (Murakami & Hirota 1989–92, Jevans 1992); Crytek US20140327673A1 is abandoned (`research.md` §IP).
  - A record is a memoised ray cast: (sample, fixed ray index, baseline geometry digest) → hit triangle. It never stores a patch or sample index.
  - No per-sample array of hit samples, and no per-sample light list.
  - Ray directions stay fixed and are never chosen or weighted by a record.
  - Records carry no per-light coefficient, and the cache never reaches PRL.
- **Progress accounting.**
  - Each stage counts cached and fresh units. A unit is fresh if this build computed it: traced it, or, in a stage that does not trace, ran its work rather than reading or replaying a stored result. Units served from cache, re-evaluated from records, patched without a re-trace, or proven to need no work count cached.
  - Every stage reports, whole-stage memo hits included.
  - ETA is pending units × the fresh-unit rate over unpaused fresh time. With nothing fresh yet, no ETA is shown.
- **Progress presentation.** Rationale in `research.md` §Progress presentation.
  - **The bar** runs cached, then fresh, then pending. Each segment differs by glyph shape as well as colour, and a nonzero segment always gets at least one cell. The bar needs at least three cells; a narrower layout drops it. Fresh takes the primary accent.
  - **Colour.** Theme-relative named colours only. `NO_COLOR` drops colour and keeps the glyphs.
  - **Counts** read cached / fresh / total in SI with three significant digits: exact below 1,000, then lowercase `k`, uppercase `M`, `G`, `T`, `P`, `E`, rounding carried (999,500 → `1.00M`).
  - **Narrow widths** drop labels first, then counts, then percent. The bar goes last.
  - **Sidebar.** A whole-stage hit marks its step cached with a word or glyph, never colour alone.
  - **Plain reporter and telemetry** carry the same counts.
  - **Build Summary.** Each stage row gains its cached share, and the totals gain fully cached stages and fresh compute time.
- **The build explains its cost.** For each stage that traced, the Build Summary names what caused it: changed lights by entity, changed geometry by source brush, and the samples and rays re-traced.
- **Conventions become durable.**
  - These rules, plus the presentation rules of `done/level-compiler-tui/`, land in `build_pipeline.md` §Progress reporting.
  - `docs/level_design.md` §Compiling Your Map tells authors how to read the progress bar and the summary, and which edits cost the most, re-saving rotated linked instances among them.
- **Non-goal: canonicalising light origins.** TrenchBroom's lower-precision origin rewrite in rotated linked instances moves lights a little, and those lights re-bake. No rounding makes a rewrite stable near its boundaries, and snapping can bury a light against a wall. The Build Summary's cost lines name the re-baked lights, so the author sees why.
- **Non-goal: approximate or preview SH, and any PVS or tighter id 46.** Exactness and the no-PVS rule already decide it. Named because both are the obvious shortcuts.

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
- [ ] Two worktrees of one map, each with its own edit, built alternately on one cache: each build equals its cold build and re-traces at most the union of both edits' crossing sets (RW-16).

**Locality.**
- [ ] After a local brush edit that renumbers faces and moves other charts in the atlas, a light whose reach excludes the edit hits every lightmap entry it had.
- [ ] A renumbering-only edit that moves no BSP split, with no geometry near any light, hits every per-light lightmap, SH delta and mover entry.
- [ ] An edit that moves an upper BSP split re-traces only texels and rays on re-cut faces plus the crossing set, equals cold, and its Build Summary names the re-partition and the re-cut face count.
- [ ] After a brush edit inside a light's reach, that light's SH patch-direct layer re-traces only patches on changed geometry and samples whose segment meets the changed triangles; a light whose reach excludes the edit hits its layer.
- [ ] A tint or intensity change with no presence change re-traces no lightmap visibility.
- [ ] Re-ordering a brush face's plane points, writing a brush's sides in another order, and re-cloning a rotated linked instance that holds no light, each with no geometric change, emit the same corners in the same order and hit every entry.
- [ ] After a brush edit that meets only a texel's footprint clip segment or lead-in, never its light-ward segments, that texel re-traces, and the warm build equals cold.
- [ ] An edit that grows the map's bounds moves no existing probe, and its SH re-trace is exactly the crossing set.
- [ ] A local edit that leaves the map's bounds unchanged re-bakes only the reflection probes, camera cells and chunk light lists whose recorded inputs it touches; every other one hits.
- [ ] After a local edit, every atlas layer whose placements, layer size, contributing partitions, those lights' photometric fields and encode parameters are unchanged hits its encode memo, equals the cold build's encoded layer byte for byte, and replays the cold build's texel-step warnings.
- [ ] A cluster payload memo hit equals the cold build's encoded payload byte for byte.
- [ ] After a brush edit inside a light's reach, that light re-traces only texels on changed charts, texels whose segment meets the changed triangles, and texels in tiles whose cull verdict changed while a changed triangle lies within the tile's cull query. The build reports the count.
- [ ] An edit far from a tile that changes the scene-wide shaft-cull bounds (a longer triangle edge or a wider scene extent) re-traces no texel of a tile whose cull query holds no changed triangle, and the warm build equals cold.
- [ ] After an edit that brings a triangle within a culled tile's shaft margin without meeting any of its segments, that tile's texels re-trace and count in the re-traced total, and the warm build equals cold.
- [ ] After a local edit, the SH rays re-traced are exactly the crossing set; the build reports the count.

**Refusals.**
- [ ] A stale entry from an older stage version, or a truncated or corrupt ray record, misses and re-bakes. It never serves.
- [ ] After a tracer-epoch or stage-version bump, no entry or ray record written before it serves as a baseline: every sample and ray re-traces, and the build equals cold (RW-2).
- [ ] A build killed while writing ray records, before its SH entry lands: the next build equals cold and re-traces at most the crossing set plus the rays of chunks that fail their digest (RW-15).
- [ ] An edit far from a cached ray's segment never re-traces it; an edit across it always does.
- [ ] A tracer test pins the locality rule: an answer never changes when a triangle whose padded bounds miss the segment is added or removed, including when two triangles tie at the hit distance and the change renumbers every other triangle.

**Footprint.**
- [ ] The Build Summary reports the cache footprint with the ray records, and the ray-record footprint against its ceiling.
- [ ] At the default `--cache-max-size`, stress-street's ray records price under the ceiling, and a warm one-brush edit traces SH locally with no ceiling or eviction warning (RW-17).
- [ ] A map priced past the ray-record ceiling bakes SH whole, byte-identical to cold, and warns by name. The ceiling never exceeds the cache budget less the spared set.
- [ ] When the sweep evicts all of a map's ray records, its next Build Summary says SH traced whole because of the eviction.
- [ ] Evicting a map's ray records through the start-of-build sweep, at a budget too small to keep them, costs one full SH trace on its next build; the output stays byte-identical, and the build after that is local again. Evicting only some of its record chunks re-traces at most those chunks' rays (RW-3).
- [ ] Reverting an edit, with no eviction between the builds, hits every entry it hit before the edit, with nothing re-traced; with eviction between, it re-bakes only what was evicted and equals cold (RW-4).
- [ ] Interleaved builds of two maps in one cache each stay byte-identical to their cold builds, and neither overwrites an entry the other holds; sharing equal entries is allowed.
- [ ] A ray-record chunk that is missing, or whose digest disagrees with its entry's reference to it, misses: its rays re-trace and the build equals cold, including when another build's sweep removes it mid-bake (RW-13). A chunk carried forward from an older baseline, whose digest matches the reference, serves.
- [ ] After a one-brush edit, the warm build writes only record chunks that hold a re-traced ray; every other chunk its entry references is the baseline's file, not rewritten.
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
- [ ] The bar renders correctly in full and compact layouts at every width from 3 cells up, and a narrower layout omits it.
- [ ] Under `NO_COLOR` the segments stay distinct by glyph alone.
- [ ] The number format covers 0, 999, 1,000 → `1.00k`, 999,500 → `1.00M`, and the largest 64-bit count.
- [ ] A stage with one fresh unit in a million still shows one fresh cell, and a stage with no fresh unit shows none.
- [ ] A local edit's progress reads mostly cached: a unit re-evaluated from records or patched without a re-trace counts cached.
- [ ] Plain reporter lines, the Build Summary's cached share and the telemetry events carry the same counts the TUI shows. Finish events carry the final counts; a stage with no published total carries null counts (RW-10).
- [ ] The Build Summary's totals name the fully cached stages and the fresh compute time, and the summary telemetry event carries both.
- [ ] After a one-light edit and after a one-brush edit that moves no BSP split, the Build Summary names the changed light or source brush as the cause of each traced stage, with re-traced counts, and names no unchanged light or brush.

**Durable record.**
- [ ] `build_pipeline.md` states the new cache contract (§Build Cache), the tracer locality rule (§Ray traversal), and the progress presentation rules with the carried TUI conventions (§Progress reporting); no `context/lib/` doc still says a geometry edit re-keys every layer.
- [ ] `docs/level_design.md` explains the progress bar, the summary's cost lines, and which edits cost the most, naming no compiler diagnostic flag.

### Manual
Targets are owner targets: a miss stops the build and reports to the owner; it is not an executor tuning loop. Runs use default worker count and default `--cache-max-size`, timed as wall time from launch to exit, warm from a copy of the baseline cache.
- [ ] On any machine but the reference container, a one-brush, one-light or 23-instance linked-lamp edit on stress-street takes at most twice that machine's no-op.
- [ ] stress-street, dev profile, 4-core reference container: no-op ≤ 12 s, and each of those edits ≤ 45 s.
- [ ] stress-warren-crates, same setup: the no-op, one-brush and one-light edits, each reported against its stress-street target.
- [ ] stress-street on a Mac with the SDK's release-profile prl-build: the same runs, reported.
- [ ] The owner watches a warm local-edit build in light and dark terminal themes, under `NO_COLOR`, and in the compact layout, and tunes glyphs and accents.

## Path
- **First slice.** Stable identity for faces, patches and triangles, plus the SH ray record, proven on stress-street. Measure:
  - warm equals cold after the arm edit;
  - re-trace count against the crossing count in `research.md`;
  - re-evaluation time;
  - record size at 4 B per ray.
  This falsifies the riskiest assumptions: that re-evaluation is cheap and that a re-intersected distance is bit-exact.
  - If the distance is not bit-exact, records grow to 8 B (triangle plus distance). That is an owner check on the footprint.
  - If re-evaluation is not cheap, stop and report to the owner before building the rest of the SH half.
  - Also measure how often an edit moves a BSP split, and how many faces it re-cuts. Splitter stability is a follow-up only if that measurement says it matters.
- **Seams:** identity and ties at `build_leaf_ordered_faces`, `primitive_sort_key`, `closest_hit`; seeds and grids at `patch_seed_id`, `probe_grid_layout_of`, the affinity grid and the chunk light-list grid; lightmap keys and payload at `LayerKeyContext`, `geometry_slice_hash`, `LayerTexel`, `IncrementalLayerAccumulator::fold_partition`, `pack_groups_into_layers`; SH at `later_bounce_sum`, `gather_patch`, `PatchGather::sample`, `DepthTileSelector`; progress at `StageProgress`, `RemainingEstimate`, `progress_bar_line`.
- **Precedents:** the warm-cache harness in `research.md` §Measurements; `--dump-chart-hashes` for lightmap comparisons; the mover channel's near-field domain as prior region-limited SH work.
- **Split first:** `pipeline.rs`, `geometry.rs`, `pipeline/lightmap_stage.rs`, `lightmap_layer.rs` and `tui/tui_render.rs`, each before its change.
- **Starting values:** bar glyphs cached `▓`, fresh `█`, pending `░`; records zstd-compressed per chunk; the ceiling priced at the SH base gate beside its working-set price.
- **Fixed costs:** measure Packing's parts first (cluster payload spool, bound resolutions, serialisation, readback); the spool is the likely memo or parallel target.

## Open questions
- Exact form of the content-derived identity: which brush-side and fragment fields, and how to handle collisions and duplicated geometry — **delegated**.
- The per-map ray-record ceiling value, within the budget rule — **delegated**.
- Whether stable identity alone keeps atlas placement steady enough, or packing also needs anchoring (for example per-cluster regions) — **delegated**, measured against the per-layer memo hit rate.
