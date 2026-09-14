# lighting-scale--sparse-layer-cache-and-fused-walk

Brief · resumable · reads: `context/lib/build_pipeline.md` §Build Cache, §PRL section IDs ·
`context/lib/development_guide.md` §1, §2.5, §6 · read at `d6e1c8b` (tip of
`gameplay-stack-ai-physics-crates`, carrying `lighting-scale--shadowmask-cold-working-set`;
`origin/main` `438a849` is its ancestor) · lands on that branch after it merges

## Problem

A developer measured `prl-build`'s warm cache on the canonical dev map and found it does not
do what §Build Cache says: on `campaign-test.map` a no-edit rebuild takes 102 s against 162 s
from an empty cache, the start-of-build sweep evicts nearly every entry the previous build
wrote, and the build re-bakes most layer partitions and every SH group (`research.md`). Cause:
cache recency is file mtime, bumped on read, and the shadowmask stage re-reads its selected
lights' `lightmap_layer` entries after the lightmap and SH stages have written — so the sweep
keeps a budget's worth of 150 MB layer entries, the cheapest bytes in the cache to re-bake, and
evicts every small memo. A layer entry is 48 bytes per covered texel per light, of which 4
needed a ray; the rest is light-independent coverage or a Lambert term the compositor can
recompute.
The re-read exists because the lightmap bake, the shadowmask fill, and the layer writer each
walk the same charts and trace the same rays. Two factors compound: the payload size is why
the sweep runs at all, and the re-read is why it evicts the wrong entries. None of it is
visible at default verbosity. When this is done, a warm edit-and-rebake loop on
campaign-test-class maps fits the default budget, a no-edit rebuild is dominated by hits rather
than re-bakes, and `.prl` bytes are unchanged.

## Decisions

- **One brief, two strides, each a complete shape.** Stride 1 ships the sparse layer payload
  and the budget warning; it fixes the thrash alone and is the stopping point if the
  collaborator handoff arrives first. That stopping point does not rest on the reach fraction:
  a sparse record costs 8 bytes per reached texel — an index and the visibility, where dense
  needs no index because position is implicit — against 48 dense per covered texel. So sparse
  is at most a sixth even if every texel is reached: campaign-test's 5.46 GB of layers becomes
  at most 0.91 GB, and the whole cache fits the 2 GiB budget. Only the tenth-of-dense row
  depends on reach. Stride 2 hoists atlas preparation, moves the SH block above it, and fuses
  the three chart walks into one multi-sink pass, which buys what stride 1 does not: the
  duplicated trace leaves every `--release` compile, and the SH keys stop moving. Neither
  fixes the thrash; stride 1 already did. The stride-1 writer is a sink over the chart-walk
  primitive the branch already added, so stride 2 adds sinks rather than rewriting the writer;
  two briefs would land either a writer stride 2 rewrites or a walk with one sink nothing
  exercises.
- **Sparse, not narrow.** A `lightmap_layer` entry stores, per reached texel, only the raw
  soft visibility — including zero for fully occluded texels, and NaN. Reached is the analytic
  pre-ray coverage predicate, so presence in the entry *is* shadowmask membership. These are
  not two functions tested equal: the shadowmask's coverage predicate and the bake's early
  return are one call, and the branch's equivalence test pins it against baked coverage light
  by light. Coverage, fallback normal, world position and seed are not stored; the compositor
  re-runs the same deterministic walk over the atlas it already holds. Diverges from
  `perf-warm-lightmap-section-cache`'s deferral of value-sparse layers: the compositor branches
  it feared rewriting now sit behind that walk primitive and test. Should the identity ever be
  broken, presence-is-membership breaks with it; the fallback is to treat the analytic
  predicate as a pre-filter that must be a superset and emit on the baked predicate, keeping
  correctness at the cost of the skip. Undo cost is a format bump.
- **Reconstruction reproduces the dense fold term for term.** Per covered texel, lights fold in
  global order. A stored visibility above zero yields the shadowed irradiance and weighted
  direction from the unshadowed Lambert term evaluated with the walk's own inputs; a stored
  visibility at or below zero yields exactly zero for both, never the product; an absent light
  adds nothing, which is bit-identical to the dense layer's explicit `+0.0` because the
  accumulator starts at positive zero (`research.md` §Reconstruction is exact).
- **Both lightmap cache epochs bump.** The layer payload changes, and irradiance synthesis
  moves from bake into composite — each stage's own computation changes, which is what
  §Build Cache's bump rule keys on. The section bump is redundant for invalidation, since the
  section key already folds the layer epoch, but it keeps each epoch honest about what its
  stage computes. The shadowmask memo folds the layer epoch and invalidates with it; the test
  pinning the current layer epoch value updates with the bump.
- **The layer cache stays the shadowmask's single raw-mask source**, consumed as the sparse
  record with quantization unchanged. In stride 2 it is no longer read from a later stage: the
  fill consumes the partition the lightmap fold holds in memory. The two memos miss
  independently — the section key folds every layer hash, the shadowmask key only the selected
  lights' — so a selection-only edit, a lost shadowmask memo, or a shadowmask-only epoch bump
  can hit the section and miss the shadowmask, leaving no fold to consume. The lightmap stage
  therefore probes the shadowmask memo before taking its own section hit, and folds regardless
  when the shadowmask memo misses. One reader either way, and no second path into the cache.
- **Atlas preparation becomes its own stage, and the SH block runs before it.** Chart planning,
  packing, and the vertex split are not ray work; the lightmap bake, shadowmask fill, animated
  light chunks, and animated weight maps all read only the charts and placements they produce.
  Every SH-block consumer reads vertex positions through the index buffer or as an AABB fold
  and nothing the split changes, so SH outputs are bit-identical either side of it. The move
  is required, not cosmetic: the fill needs the selected-light set, which the direct SH delta
  bake can clear wholesale, and channel assignment must precede the walk so the fill writes
  straight into its channel — the shadowmask brief's residency invariants leave nowhere to
  park a per-light raw record. By construction the SH-block keys then hash pre-UV geometry, so
  density and scale-region edits stop invalidating SH, with no hand-written key logic.
- **One walk, several sinks, cold and warm alike.** Per layer, chart, texel, and light, the
  Lambert term and the visibility ray are computed once and offered to the lightmap
  accumulator, the shadowmask channel fill for selected lights, and — warm only — the sparse
  writer. Coverage is still evaluated twice: the analytic graph pass needs it per chart and
  light before the walk begins, and that pass stays. Ray work is what the walk shares. The
  cold `--release` path keeps its inline per-texel light sum as a sink over the same walk:
  "exact ship source of truth" constrains values, not the call graph.
- **One warning closes the observability gap.** When the entries a build read or wrote total
  more than `--cache-max-size`, the build ends with one `log::warn!` naming both figures.
  Warn, because the default level hides info and the prune and hit/miss lines are exactly what
  a collaborator never saw. At build end, not at the sweep: the sweep cannot know the live
  set; the build can.
- **No compression, no prune policy.** The sparse payload is far under budget without either;
  compression adds decode CPU to the fold; a stage-class prune would ship as permanent policy
  to relieve an eviction pressure that stride 1 removes outright, once the payload fits.
- **Layer placement.** Compiler-internal, dev-local cache and stage topology. No runtime, no
  PRL section, no constant outside the cache epochs.

### Non-goals

| Not doing | Warrant |
|---|---|
| Any `.prl` byte, section, or runtime change | The byte-identity gate is the contract; every row inherits it |
| Narrowing the layer key's whole-light fold | The unread fields are rare edits; the saving is small and under-keying is a one-way door. Revisit only if a measured edit log shows orphans dominating after slimming |
| Collaborator ergonomics | `docs/level_design.md` says nothing about warm vs `--release`, the cache, disk, or memory; an allocation failure aborts without drop, so `tui/tui_terminal.rs` never restores the terminal; the delta working-set default is a constant tuned to one host. Different files, no shared analysis — a separate direct build, `compiler-collaborator-ergonomics`, runnable in parallel |
| The refuse half of `compiler-implausible-allocation-guard` | That brief owns both halves, including report-at-stage-start |
| Folding the animated weight-map bake into the walk | Shares the raster loop, not the lights or the seed mixer; no ray shared, no thrash through it |
| The SH probe-side duplication | Direct tile and direct-delta sub-block bake the same probe with the same seeds; same pattern, other axis, not on this chain |
| Compressing cache entries | Revisit only if a measured warm loop is I/O-bound |
| Runtime and GPU footprint; adaptive density | Other plans |

## Acceptance

### Automated

Byte identity — the contract; the existing gates pass today:

- [ ] Warm composite equals cold direct lightmap byte-for-byte pre-BC6H on the existing unit
  gates and on a new end-to-end atlas comparison through the real stage, two fixtures, one
  multi-layer: cold; warm from an empty cache; warm from the cache that run wrote. No such
  end-to-end row exists today, and this is the first change that moves synthesis out of the bake.
- [ ] Both encoded-section modes, uncompressed and BC6H, stay byte-identical across the fold.
- [ ] Whole-file determinism holds at one worker and many; the shadowmask section is
  byte-identical across cold, warm miss, warm partition miss, and whole-section hit.
- [ ] The warm fallback with no layer-bearing lights still emits one uncovered plane.
- [ ] The byte-identity reference stays independent of the fused walk: the reference module
  references no walk entry point. Shared leaf kernels are excluded — the per-texel contribution,
  soft visibility, the texel seed, the chart texel position. Both sides call those today and
  after, so a defect there passes either way. Without this row the gate compares the walk to
  itself.
- [ ] The frozen reference is compared against the cold stage output on the same two fixtures
  the end-to-end row uses, so a walk defect that only real chart layouts expose cannot pass
  walk-versus-walk.

Reconstruction edges — both sides of each predicate:

- [ ] A texel the light does not reach: absent from the partition, contributes nothing,
  absent from shadowmask membership.
- [ ] A texel fully occluded: present with value zero; zero irradiance and zero weighted
  direction, not the term times zero; present in membership with channel byte zero.
- [ ] A texel at NaN visibility: present; composite bits equal the dense fold's; present in
  membership as today.
- [ ] A weighted-direction component that is negative zero from one light, with a later light
  in global order unreached, composites to the dense fold's bits (`research.md` P4).
- [ ] Contributions at the two representable values straddling the coverage epsilon: lower
  absent, upper present, using the baker's own comparison.

Cache behaviour. The lightmap path's layer hit/miss lines are verbose-gated and the shadowmask
path's are not, and `compiler-log-hygiene` will move both to debug — so rows below that count
reads run verbose or count through a test-only counter, never by scraping a default-level build:

- [ ] On a named multi-layer fixture, total bytes written under the layer stage id are below a
  tenth of the dense figure, asserted from the entries written.
- [ ] A no-edit rebuild reads no layer entry and hits both lightmap memos; a one-light edit
  re-bakes only the edited light's partitions and reads each unaffected partition once per
  consumer in stride 1, exactly once after stride 2. For a selected light the shadowmask fill
  is the second consumer until fusion removes it (`research.md` P5).
- [ ] A decodable partition whose indices fall outside the layer's covered set, exceed its
  covered count, or are not strictly increasing is a soft miss that re-bakes — never used,
  never an error.
- [ ] An entry written before this change is not read after it, for both epochs.
- [ ] A build whose read-plus-written bytes exceed the budget ends with exactly one warning
  naming both figures; under budget, none; `--no-cache` and `--release`, none.
- [ ] An entry the build writes and then reads, or reads twice, counts once toward the
  warning's figure; a live set under budget raises no warning however many times its entries
  were touched (`research.md` P2).
- [ ] A light that reaches no texel on a layer writes a partition with zero texels; the rerun
  hits it and re-bakes nothing, the fold adds nothing, and the light is absent from that
  layer's channels (`research.md` P3).

Stride 2 — order and fusion:

- [ ] Every SH-block stage completes before atlas preparation begins, and a lightmap-density or
  scale-region edit hits every SH-block memo while missing the lightmap ones.
- [ ] A fixture whose selection is cleared because the direct SH delta section is absent emits
  no shadowmask section and fills no channel, bytes unchanged.
- [ ] After the lightmap fold completes, no stage reads a layer entry, cold or warm. Counted
  the same way as the cache rows above.
- [ ] Lightmap section memo hit with shadowmask memo miss — a selection-only edit, or a
  shadowmask memo lost while the section memo survived: the shadowmask section equals the cold
  bytes, no partition is re-baked, each selected partition is read at most once, and no layer
  entry is read after the lightmap stage ends (`research.md` P1).
- [ ] Channel assignment is complete before the first texel of the fused walk, and the fill
  writes only into the assigned channel — two lights sharing a texel on a layer above zero
  still land in different channels.
- [ ] The lightmap stage publishes one real total covering the fused work; the shadowmask stage
  publishes one real total covering graph pass and encode; neither overshoots.
- [ ] The Build Summary stage order changes deliberately: the summary-contract test is updated
  to the new order, not loosened. The TUI step sections flatten to the stage list in the same
  order, with atlas preparation under World.

### Manual

- [ ] After each stride, the three-run measurement on `campaign-test.map` — empty cache,
  no-edit rerun, one light-intensity edit — scratch cache dir, default budget. Record cache
  bytes after each run, entries evicted at the sweep, `lightmap_section` and `sh_group` hit
  counts, lightmap and SH stage times. Headline: the no-edit rerun evicts nothing it then
  needs, both lightmap memos hit, every SH group hits.
- [ ] At the fusion commit, a defect injected into the walk fails the cold gate rather than
  passing it. Manual because automating it needs a test-only perturbation hook in the walk,
  which this brief does not add.
- [ ] After stride 2, the cold shadowmask stage on the same map costs graph pass plus encode —
  no re-trace — and the whole cold build is no slower than before.
- [ ] After stride 2, one `--release` compile of `stress-warren-hallway-inspection.map` at
  density 0.04 on the owner's Windows machine, peak working set captured out-of-band. It
  serves this brief's residency claim and closes the pending manual rows of
  `lighting-scale--lightmap-bake-incremental-flush`; after stride 2 because fusion changes
  cold residency.

## Path

- The walk is `lightmap_layer::for_each_light_layer_chart_texel` on the branch; the first sink
  shape is `bake_light_layer_chart_controlled`; the presence predicate is
  `lightmap_bake::light_texel_is_covered`. Reconstruction lands in
  `IncrementalLayerAccumulator::fold_partition`; `light_texel_contribution_and_visibility` is
  its oracle — the two early returns are the two predicates. `validate_layer_partition` becomes
  a bounds-and-monotone check; the record's `layer` field moves to the header.
- Hoisting: `prepare_atlas` is called from the warm block in `pipeline.rs` and inside
  `bake_lightmap_controlled`; both take a prepared atlas afterwards. The SH block reads
  geometry, tree, exterior leaves, BVH and lights only, so it moves as a unit. `StageId`
  declaration order and `label` drive the Build Summary. The TUI's step sections are a second
  ordering the compiler does not check; a test pins them against the stage list, so they move
  with it. Atlas preparation joins the World section, not Lighting — chart planning and packing
  trace no rays, and keeping Lighting to the ray-tracing stages is what makes its header mean
  something once the SH bakes run first.
- Coloring before the walk: the branch's `build_analytic_overlap_graph` and
  `shadowmask_bake/assignment.rs` already precede the fill; keep them, move the fill into a
  walk sink. Cold fusion routes only the shipping path — `bake_atlas_layer_controlled` —
  through the walk plus sinks. `bake_face_chart` is called by that path *and* by
  `bake_monolithic_atlas_controlled`, so the byte-identity reference keeps its own frozen copy
  of the current raster loop rather than following the walk: a gate whose two sides run the
  same kernel proves nothing.
- The warning: tally bytes in `StageCache::get` on hit and in `put`; report from `main.rs`
  against `args.cache_max_bytes`.
- Rivals, rejected: a narrow dense record (6× at every reach fraction, where sparse matches
  that only in the worst case and beats it as reach drops — and the re-read stays either way);
  a per-layout coverage side table (a second copy of the walk); a stage-class prune
  (scaffolding); an end-of-build live-set prune (sweeping what this build actually used
  defeats the mtime inversion, but leaves the payload size that creates the eviction pressure);
  compression (decode on the fold).
- First slice: the sparse partition plus reconstruction under today's fold, gated by the new
  end-to-end cold-versus-warm row — it falsifies the riskiest assumption before any stage moves.
- `lightmap_bake.rs`, `lightmap_layer.rs`, `shadowmask_bake.rs` and `pipeline.rs` are far past
  the split threshold: split behaviour-preserving in their own commit first; the lightmap stage
  block of `pipeline.rs` is the natural extraction and the hoist wants it anyway.

## Open questions

- Sparse record layout — interleaved pairs or split arrays, delta-coded index or not —
  **delegated**: the decision is presence semantics, not bytes.
- Whether `ChunkLightList` moves above atlas preparation with the SH block — **delegated**.
- The reach fraction on campaign-test, hence the measured payload ratio — **delegated**:
  measured in stride 1 and recorded in the plan of record. The 8.18% figure in `research.md` is
  light/*chart* pairs on mini-warren; the payload ratio turns on the light/*texel* fraction,
  which can differ materially, and `shadowmask-cold-working-set`'s research says not to size
  from published reach fractions. The cache-bytes row asserts a tenth of dense, which holds
  well short of the chart-pair figure.
