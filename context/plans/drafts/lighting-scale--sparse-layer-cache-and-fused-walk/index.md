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
lights' `lightmap_layer` entries after every other stage has written — so the sweep keeps a
budget's worth of 150 MB layer entries, the cheapest bytes in the cache to re-bake, and evicts
every small memo. A layer entry is 48 bytes per covered texel per light, of which 4 needed a
ray; the rest is light-independent coverage or a Lambert term the compositor can recompute.
The re-read exists because the lightmap bake, the shadowmask fill, and the layer writer each
walk the same charts and trace the same rays. The duplication is the thrash mechanism, and
none of it is visible at default verbosity. When this is done, a warm edit-and-rebake loop on
campaign-test-class maps fits the default budget, a no-edit rebuild is dominated by hits rather
than re-bakes, and `.prl` bytes are unchanged.

## Decisions

- **One brief, two strides, each a complete shape.** Stride 1 ships the sparse layer payload
  and the budget warning; it fixes the thrash alone and is the stopping point if the
  collaborator handoff arrives first. Stride 2 hoists atlas preparation, moves the SH block
  above it, and fuses the three chart walks into one multi-sink pass, removing the re-read.
  The stride-1 writer is a sink over the chart-walk primitive the branch already added, so
  stride 2 adds sinks rather than rewriting the writer; two briefs would land either a writer
  stride 2 rewrites or a walk with one sink nothing exercises.
- **Sparse, not narrow.** A `lightmap_layer` entry stores, per reached texel, only the raw
  soft visibility — including zero for fully occluded texels, and NaN. Reached is the analytic
  pre-ray coverage predicate the branch tested equal to baked coverage, so presence in the
  entry *is* shadowmask membership. Coverage, fallback normal, world position and seed are not
  stored; the compositor re-runs the same deterministic walk over the atlas it already holds.
  Diverges from `perf-warm-lightmap-section-cache`'s deferral of value-sparse layers: the
  compositor branches it feared rewriting now sit behind the branch's walk primitive and
  equivalence test. Undo cost is a format bump.
- **Reconstruction reproduces the dense fold term for term.** Per covered texel, lights fold in
  global order. A stored visibility above zero yields the shadowed irradiance and weighted
  direction from the unshadowed Lambert term evaluated with the walk's own inputs; a stored
  visibility at or below zero yields exactly zero for both, never the product; an absent light
  adds nothing, which is bit-identical to the dense layer's explicit `+0.0` because the
  accumulator starts at positive zero (`research.md` §Reconstruction is exact).
- **Both lightmap cache epochs bump**, per §Build Cache's bump rule: the layer payload changes
  and irradiance synthesis moves from bake to composite. The shadowmask memo folds the layer
  epoch and invalidates with it.
- **The layer cache stays the shadowmask's single raw-mask source**, consumed as the sparse
  record with quantization unchanged. In stride 2 it is no longer read from a later stage: the
  fill consumes the partition the lightmap fold holds in memory.
- **Atlas preparation becomes its own stage, and the SH block runs before it.** Chart planning,
  packing, and the vertex split are not ray work, and three stages consume only their output.
  Every SH-block consumer reads vertex positions through the index buffer or as an AABB fold
  and nothing the split changes, so SH outputs are bit-identical either side of it. The move
  is required, not cosmetic: the fill needs the selected-light set, which the direct SH delta
  bake can clear wholesale, and channel assignment must precede the walk so the fill writes
  straight into its channel — the shadowmask brief's residency invariants leave nowhere to
  park a per-light raw record. By construction the SH-block keys then hash pre-UV geometry, so
  density and scale-region edits stop invalidating SH, with no hand-written key logic.
- **One walk, several sinks, cold and warm alike.** Per layer, chart, texel, and light,
  coverage, Lambert term and visibility are computed once and offered to the lightmap
  accumulator, the shadowmask channel fill for selected lights, and — warm only — the sparse
  writer. The cold `--release` path keeps its inline per-texel light sum as a sink over the
  same walk: "exact ship source of truth" constrains values, not the call graph.
- **One warning closes the observability gap.** When the entries a build read or wrote total
  more than `--cache-max-size`, the build ends with one `log::warn!` naming both figures.
  Warn, because the default level hides info and the prune and hit/miss lines are exactly what
  a collaborator never saw. At build end, not at the sweep: the sweep cannot know the live
  set; the build can.
- **No compression, no prune policy.** The sparse payload is far under budget without either;
  compression adds decode CPU to the fold, and a stage-class prune is scaffolding stride 2
  makes moot that would ship as permanent policy.
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

Reconstruction edges — both sides of each predicate:

- [ ] A texel the light does not reach: absent from the partition, contributes nothing,
  absent from shadowmask membership.
- [ ] A texel fully occluded: present with value zero; zero irradiance and zero weighted
  direction, not the term times zero; present in membership with channel byte zero.
- [ ] A texel at NaN visibility: present; composite bits equal the dense fold's; present in
  membership as today.
- [ ] A weighted-direction component that is negative zero from one light, with a later light
  in global order unreached, composites to the dense fold's bits.
- [ ] Contributions at the two representable values straddling the coverage epsilon: lower
  absent, upper present, using the baker's own comparison.

Cache behaviour:

- [ ] On a named multi-layer fixture, total bytes written under the layer stage id are below a
  tenth of the dense figure, asserted from the entries written.
- [ ] A no-edit rebuild reads no layer entry and hits both lightmap memos; a one-light edit
  reads each unaffected partition exactly once and re-bakes only the edited light's partitions.
  Asserted on captured log lines.
- [ ] A decodable partition whose indices fall outside the layer's covered set, exceed its
  covered count, or are not strictly increasing is a soft miss that re-bakes — never used,
  never an error.
- [ ] An entry written before this change is not read after it, for both epochs.
- [ ] A build whose read-plus-written bytes exceed the budget ends with exactly one warning
  naming both figures; under budget, none; `--no-cache` and `--release`, none.

Stride 2 — order and fusion:

- [ ] Every SH-block stage completes before atlas preparation begins, and a lightmap-density or
  scale-region edit hits every SH-block memo while missing the lightmap ones.
- [ ] A fixture whose selection is cleared because the direct SH delta section is absent emits
  no shadowmask section and fills no channel, bytes unchanged.
- [ ] After the lightmap fold completes, no stage reads a layer entry, cold or warm. Asserted
  on captured log lines.
- [ ] Channel assignment is complete before the first texel of the fused walk, and the fill
  writes only into the assigned channel — two lights sharing a texel on a layer above zero
  still land in different channels.
- [ ] The lightmap stage publishes one real total covering the fused work; the shadowmask stage
  publishes one real total covering graph pass and encode; neither overshoots.
- [ ] The Build Summary stage order changes deliberately: the summary-contract test is updated
  to the new order, not loosened.

### Manual

- [ ] After each stride, the three-run measurement on `campaign-test.map` — empty cache,
  no-edit rerun, one light-intensity edit — scratch cache dir, default budget. Record cache
  bytes after each run, entries evicted at the sweep, `lightmap_section` and `sh_group` hit
  counts, lightmap and SH stage times. Headline: the no-edit rerun evicts nothing it then
  needs, both lightmap memos hit, every SH group hits.
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
  declaration order and `label` drive the Build Summary.
- Coloring before the walk: the branch's `build_analytic_overlap_graph` and
  `shadowmask_bake/assignment.rs` already precede the fill; keep them, move the fill into a
  walk sink. Cold fusion replaces `bake_face_chart`'s raster loop with the walk plus sinks;
  `bake_monolithic_atlas_controlled` stays as the test oracle, untouched.
- The warning: tally bytes in `StageCache::get` on hit and in `put`; report from `main.rs`
  against `args.cache_max_bytes`.
- Rivals, rejected: a narrow dense record (roughly 6× where sparse is roughly 100×, and the
  re-read stays); a per-layout coverage side table (a second copy of the walk); a stage-class
  prune (scaffolding); compression (decode on the fold).
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
  measured in stride 1 and recorded in the plan of record; the figures in `research.md` are
  from other fixtures.
