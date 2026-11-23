# local-edit-warm-builds

Brief · resumable · reads: `context/lib/build_pipeline.md` §Build Cache, §Progress reporting, controls, and logging, §Id-51 consumers · `done/surface-radiance-cache/` · `done/incremental-bake-per-element/` · `done/level-compiler-tui/` · `context/research/gi-rtx-lite/ip-check.md` · read at 3703dd164

## Problem
**Basis.** Owner-observed defect in the authoring loop: an edit's rebuild costs what the whole map costs, and the build cannot show why. After a TrenchBroom linked-group update, prl-build rebuilds as if the cache were empty. On stress-street, raising one brush in one lamp instance costs 234 s warm against 21 s for a no-op. Every cache stage misses, though 99.5% of lightmap texels come out byte-identical (`research.md` §Measurements).

**Cause.** Cache keys, seeds and tie-breaks are built from global identity: BSP leaf and cell order, global face and vertex indices, a whole-atlas fingerprint, and a probe grid anchored to the map's bounds. Any edit renumbers them. On top of that, the SH base volume is one entry whose gather re-traces every ray after any light or geometry edit. Linked groups only make this frequent: one edit lands in every instance.

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
- **Cache footprint is bounded and visible.** The ray records have a per-map disk ceiling, set at build from the stress fixtures. It is sized against `--cache-max-size`'s spared-set rule (§Build Cache, Eviction), since every live map's records are spared. The Build Summary reports the footprint against it, and an overrun is a named warning. The cache stays disposable.
- **This reverses documented choices.**
  - "A geometry edit re-keys every layer" is retired, from `build_pipeline.md` §Build Cache and from the decisions of `done/incremental-bake-per-element/` and `done/surface-radiance-cache/`. Those plans named finer geometry locality as future work; this brief is that work.
  - `incremental-bake-per-element` also ruled out primary-ray-hit caching. The SH ray records lift that ruling, subject to the IP question below.
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
- [ ] a linked group updated through TrenchBroom's transform-and-reclone path, rotated instances included.

**Locality.**
- [ ] After a local brush edit, a light whose reach excludes the edit hits every lightmap layer entry it had.
- [ ] A renumbering-only edit, with no geometry near any light, hits every per-light lightmap, SH delta and mover entry.
- [ ] A tint or intensity change with no presence change re-traces no lightmap visibility.
- [ ] After a brush edit inside a light's reach, that light re-traces only the samples on changed charts or patches, plus samples whose shadow segment meets the changed triangles. The build reports the count.
- [ ] After a local edit, the SH rays re-traced are exactly the crossing set; the build reports the count.

**Refusals.**
- [ ] A stale entry from an older stage version, or a truncated or corrupt ray record, misses and re-bakes. It never serves.
- [ ] An edit far from a cached ray's segment never re-traces it; an edit across it always does.

**Footprint.**
- [ ] The Build Summary reports the cache footprint with the ray records.
- [ ] Exceeding the ceiling warns by name.

**Progress accounting.**
- [ ] Accounting is correct for each kind of stage: all hits, all fresh, mixed, whole-stage hit, and zero units.
- [ ] Every stage, including a `reflection_probes` hit and the mover direct half, publishes a total.
- [ ] ETA follows the fresh rate. An all-cached stage shows no ETA.

**Presentation.**
- [ ] The bar renders correctly in full and compact layouts at every width from 2 cells up.
- [ ] Under `NO_COLOR` the segments stay distinct by glyph alone.
- [ ] The number format covers 0, 999, 1,000 → `1.00k`, 999,500 → `1.00M`, and the largest count.
- [ ] Plain reporter lines, the Build Summary's cached share and the telemetry events carry the same counts the TUI shows.

### Manual
- [ ] stress-street (dev profile, the 4-core reference container, warm from a copy of the baseline cache): no-op ≤ 12 s, one-brush edit ≤ 45 s, one-light edit ≤ 45 s, and the 23-instance linked-lamp brush edit ≤ 45 s.
- [ ] stress-warren-crates: the same three runs, reported against the same targets.
- [ ] The owner watches a warm local-edit build in light and dark terminal themes, under `NO_COLOR`, and in the compact layout. Glyph and accent choices are tuned with the owner at that point.

## Path
- **First slice.** Stable identity for faces, patches and triangles, plus the SH ray record, proven on stress-street. Measure:
  - warm equals cold after the arm edit;
  - re-trace count against the crossing count in `research.md`;
  - re-evaluation time;
  - record size at 4 B per ray.
  This falsifies the riskiest assumptions: that re-evaluation is cheap and that a re-intersected distance is bit-exact.
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
- Whether persisted per-ray hit records stay clear of Crytek US20140327673A1 claim 1 (`ip-check.md`, "concern if granted"). Research is under way. Answer by recording an IP-guard decision, or by redesigning the record — owner — **blocks build** for the SH ray-record half only.
- Exact form of the content-derived identity: which brush-side and fragment fields, and how to handle collisions and duplicated geometry — **delegated**.
- The per-map ray-record ceiling value, set from stress-fixture measurement — **delegated**.
- Whether stable identity alone keeps atlas placement steady enough, or packing also needs anchoring (for example per-cluster regions) — **delegated**, measured against the per-layer memo hit rate.
