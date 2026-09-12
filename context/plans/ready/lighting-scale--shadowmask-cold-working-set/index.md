# lighting-scale--shadowmask-cold-working-set

Brief · resumable · reads: `context/lib/build_pipeline.md` §PRL section IDs, §Build Cache, §Distribution packaging · read at `6168c5c9`, with `lighting-scale--lightmap-bake-incremental-flush` merged

## Problem

The owner's `--release` compile of `stress-warren-hallway-inspection.map` at lightmap
density 0.04 exhausts a 16 GiB Windows machine in the ShadowmaskAtlas stage. Channel
assignment holds a full-precision visibility record for every covering light-and-texel
pair, and the overlap graph builds a second per-texel index over it while that record is
fully resident. Measured atlas growth puts the failing config at 3.2e8 texels, where 1%
coverage per light already exceeds the machine. Prior plans optimized the record's storage
instead, taking it as unavoidable. It is not: coverage is a geometric predicate on light,
texel position, and surface normal, and only the visibility value needs a ray. When this is
done the graph is built without baking, the per-pair record does not exist, and the stage's
only light-scaling residency is an adjacency matrix of one byte per light pair.

## Decisions

- **Build the overlap graph analytically; delete the per-pair record.** The coverage test
  returns before soft visibility is sampled, so it needs no ray. Nothing carries between
  graph and fill. This retires `shadowmask-bake-scaling`'s irreducible-membership claim at
  its premise rather than diverging from its conclusion.
- **Share the per-chart walk, not just the predicate.** The bake skips charts with
  non-positive UV extent before walking; `chart_raster::chart_interior_dims` clamps to one
  texel. A graph pass assembled from those primitives over-covers every degenerate chart
  and changes bytes. Add a coverage-only mode to the existing walk.
- **Coverage is unshadowed, and that becomes a contract.** Channel assignment stays stable
  under occlusion edits — not under geometry edits, which move texel world positions and do
  change coverage. Shadow-aware coverage becomes a format-level decision rather than a
  quiet tweak.
- **Equivalence is the gate, then a standing test.** Analytic coverage must equal baked
  coverage. A one-time check would let the two definitions drift apart later. On failure
  the work falls back to the bounded-membership shape in `research.md`, and those
  decisions return to the owner.
- **Coloring stays global.** `shadowmask-bake-scaling` forecloses per-tile *coloring*,
  because id-42 gives each light one channel across every atlas texel. Only graph
  construction changes.
- **A prune is load-bearing, and it may skip work but never change the graph.** Unpruned,
  the pass is order 1e11 contribution tests at the failing density, which is not viable.
  The prune may not alter the node set or the edge set: removing a node shifts every later
  light's channel, and dropping a true edge lets coloring give two overlapping lights one
  channel, which turns the fill's relaxed stores into two threads writing one byte. Its
  shape, and the reach fraction it achieves here, are the executor's to measure.
- **Fill in one bake, atlas layer outer, cold and warm alike.** Each light-and-layer
  partition is baked or read from cache, written into its assigned channel, and dropped.
  The warm path shares the defect and the fix.
- **One full-size copy of the section exists at a time, from fill through write.** At the
  failing density that buffer is 1.29 GB, not the shipped artifact's 37.7 MB. Where the
  duplicate copy lives is the executor's finding — candidates sit in the fill's conversion
  and in packing, and `lighting-scale--compile-peak-ram` already owns the packing path's
  `Write`-based serialization. The buffer stays content-sized because it is the bytes
  written to disk.
- **Cache keys and memo semantics are unchanged.** The fill still reads layer partitions,
  so the layer fingerprint still governs; the graph pass reads no cache at all. The
  per-partition layer keys `lighting-scale--lightmap-bake-incremental-flush` landed stay as
  they are.
- **Progress stays determinate.** `shadowmask-bake-scaling` published a real total so the
  stage shows a percentage rather than a spinner. The countable units change from
  light-and-chart bake work to the graph pass plus the fill, so the total is recomputed
  rather than inherited.
- **RSS read out-of-band**, per `shadowmask-bake-scaling`. The residency claim is
  structural rather than budgeted, so measurement only confirms it.
- **Durable capture is scheduled, not deferred.** At promotion, `build_pipeline.md` id-42
  loses "irreducibly light-scaling" and its foreclosure gains the graph-versus-coloring
  distinction; §Distribution packaging's one-bake-at-a-time *rationale* changes with it.

### Non-goals

| Not doing | Warrant |
|---|---|
| Any `.prl` byte, format version, or runtime change | Bytes, selection ordering, channel assignment, drop policy, invalid-selection behavior all stay |
| Capping or ranking selected lights | `shadowmask-bake-scaling` rejected it — capping drops shadows |
| Reshaping the shared section encoder | `lighting-scale--compile-peak-ram` owns the `Write`-based serialization path and names this section. This brief states the one-copy invariant; that plan is one way to satisfy it |
| Attributing the 42 TB allocation | `compiler-implausible-allocation-guard` owns it. Compiler-wide robustness, and it must land without waiting on this stage's equivalence gate |
| Making SH-delta complete | Ray-bake payload, no analytic equivalent, so the lever here does not transfer |
| The id-42 format expansion | `shadowmask-no-drop-atlas` owns block dimension and slot assignment, and records a collision risk with restructures of this assignment seam — order the two deliberately. The intensity-ordered retention it preserves gets cheaper; a contribution- or coverage-weighted priority would have to re-materialize the deleted record |
| Bounding the output below its on-disk size | A format question |
| Exposing residency as a flag | After the restructure there is nothing left to tune: light-scaling residency is the adjacency matrix, and the only content-sized term is the section itself |

## Acceptance

### Automated

Equivalence — decides the shape, then stands so the two definitions cannot drift:

- [ ] Analytic coverage equals baked coverage, light by light, across the fixture matrix
  and the golden. Edges pinned both ways: contributions at the two representable values
  straddling the epsilon, with the shared comparison itself asserted identical rather than
  an exact-epsilon input constructed; a fully occluded texel, covered with value zero; a
  texel beyond falloff range, not covered; a chart with non-positive UV extent, covered by
  neither; a NaN visibility, covered by both.
- [ ] The prune is a superset test, asserted directly: for every light-and-chart pair the
  prune rejects, the coverage-only walk over that chart returns no texel. A rejected pair
  that covers a texel fails the gate, whatever the aggregate sets say. Pins `ord-prune-unsound`
  — an unsound prune is not a coverage miss, it is two threads storing to one byte.
- [ ] A selected light the prune removes entirely keeps its graph node and its channel.
  Bytes and the whole channel table are unchanged from a run with pruning disabled. Pins
  `ord-zero-coverage`.

The new pass — residency and ordering; none of these can pass today:

- [ ] No structure indexed by light-and-texel is live at any point in the stage. The only
  light-scaling residency left is the adjacency matrix at one byte per light pair — 114 KB
  at 338 lights — asserted from its dimensions and element size, not from a memory
  measurement, since the compiler has no in-process peak probe.
- [ ] Light-scaling residency is independent of atlas layer count at fixed plane size and
  light count. The output buffer is excluded and stays proportional to plane times layers:
  it is the bytes written to disk, and this row makes no claim about it.
- [ ] Grep and review gate, not a test: exactly one full-size output allocation appears in
  the stage, and the atomic-to-plain conversion is shown to be in-place or replaced. A
  counting allocator asserting one allocation above the plane size is the only runnable
  form, and is worth writing only if the review finds a second copy.
- [ ] The output buffer is allocated once on every path, including the wholly filtered
  selection on the cached path, which today takes the full fill route while the uncached
  path short-circuits. Pins `ord-filtered-alloc`; byte equality hides this, so the row
  asserts allocations.
- [ ] Coloring runs exactly once, over the completed adjacency. A graph pass whose last
  work item is delayed cannot produce a channel assignment before that item joins. Pins
  `ord-graph-barrier` — draining the governor is not the barrier.
- [ ] A pause during the graph pass parks it and resumes it to byte-identical output, and
  lowering `-j` mid-pass neither preempts an in-flight item nor deadlocks against a
  permitted chart item. Pins `ord-pause-midgraph`.
- [ ] Progress advances during the graph pass. Nothing advances there today, so this row
  is red on the current build by design.

Regression guards — these pass today:

- [ ] Refuse side: two lights whose only shared texel lies on an atlas layer above zero
  receive different channels. Permit side: two lights sharing no texel on any layer may
  receive the same channel. Asserted through a test-only seam on the analytic graph pass
  that accepts synthetic per-light coverage, since the membership struct today's test feeds
  is deleted.
- [ ] The graph pass produces the same adjacency and the same channel table under reversed
  tile order, independently of `-j`. Pins `ord-discovery` — adjacency is a set union, not
  first-writer-wins.
- [ ] Byte identity across every lifecycle path — cold, cold cache miss, warm partition
  miss, whole-section hit — and across the analytic route.
- [ ] Degenerate inputs unchanged: zero selection emits no section; a wholly filtered
  selection emits the empty payload and indeterminate progress; an out-of-range selection
  keeps its dropped slot; a single-layer atlas matches a single-layer golden literal
  captured before the restructure, alongside the existing two-layer golden.
- [ ] Cache behavior unchanged: a rebuild with no input change hits the section memo; a
  changed selected-light parameter misses it and still hits every other light's layer
  partition, with the re-run graph pass reading no cache entry. Pins `ord-one-light-changed`.
- [ ] The stage cache version and the layer key shape are unchanged by this work. A cache
  written before the restructure is read after it with no re-bake — changing how coverage
  is discovered is not on the version's documented bump list.
- [ ] A coloring-dropped light's layer cache entries are populated or skipped by a stated
  rule, and the following compile's hit rate matches that rule. Bytes unchanged either way.
  Pins `ord-dropped-partition`.
- [ ] A layer with both warm and baked partitions advances progress to exactly the
  published total, once, with no overshoot, whichever order the branches interleave in.
  Pins `ord-mixed-partition`.
- [ ] Progress reads short of the published total until after the section memo write, on
  both the cached and uncached paths, under the layer-outer fill. Pins `ord-final-unit`.
- [ ] The stage publishes a real total exactly once, and reaches completion only when the
  section is finished.
- [ ] Recompiling an unchanged input twice, at one worker and at many, produces
  byte-identical section-42 bytes, on a named fixture that emits section 42. No such
  coverage exists today; the existing whole-file determinism test is ignored by default and
  its fixture may emit no section 42.

### Manual

Density 0.04 on this map is a **stress probe, not a supported configuration**. It emits a
section near 1.29 GB, and the runtime's usability filter checks only dimensions and layer
count — nothing stands between that artifact and an uncompressed texture upload of the
same size. These rows prove the compiler survives the stress case; they make no claim that
the result ships.

- [ ] On the owner's 16 GiB Windows machine, a `--release` compile of
  `stress-warren-hallway-inspection.map` at density 0.04 reaches and completes this stage,
  with out-of-band peak RSS recorded. Budget the run: the lightmap stage alone takes
  roughly forty minutes at that density.
- [ ] If a machine with headroom can produce a reference build at this density, the stress
  compile's section-42 bytes match it. No pre-change build can serve as that reference —
  the pre-change compiler cannot reach the end of this stage at 0.04, which is the defect.
  Absent such a machine, byte evidence rests on the supported-density row below and on the
  automated golden gates, and this row is recorded as not run.
- [ ] A supported-density compile of the same map is unchanged in bytes and no slower than
  a pre-change build by more than a stated margin.

## Path

- One call site, `bake_shadowmask_atlas_cached` in `pipeline.rs`. Cold and warm converge on
  `collect_shadowmask_membership_in_batches` and
  `build_shadowmask_from_membership_with_assignment_checkpoint`; both lose their reason to
  exist in their current form. `texel_lights` in `overlap_graph_controlled` is deleted
  rather than replaced — it recovers adjacency from the stored record, which the graph pass
  now emits directly.
- The coverage-only walk belongs beside `bake_light_layer_chart_controlled`, which already
  holds the degenerate-chart skip and the padding offsets the graph pass must match.
- The reach-cull machinery already ships: `affinity_grid` decompose plus `ReachIndex`
  inversion, consumed by the direct SH bake. Reuse it rather than writing a second one.
  `lighting-scale--cold-bake-reaching-light-spike` measures the same mechanism for the cold
  SH and lightmap bakes and does not reach this stage, so no plan owns pruning here.
- Rival framing, rejected: decline uniform 0.04 on warren-class maps and coarsen decorative
  surfaces through the shipped per-surface scale regions. That lowers texel counts and
  would ease this map, but it is a content lever against a lifecycle cause — the same
  rebuttal `lighting-scale--lightmap-bake-incremental-flush` makes for the analogous case.
  Worth doing on its own merits; it does not replace this work.
- The graph pass is the stage's new long stretch and its progress signal. Parallelizing it
  puts it under the governor contract in `governor.rs`: `enter` exactly once at a work
  item's outermost boundary, never a bare `checkpoint`, which honors pause but ignores the
  `-j` cap. The nested-wait rule applies too — `lightmap-bake-throughput` documents both
  and the deadlock they prevent. `record_assignment_operation`'s cadence still covers the
  serial assignment step.
- The fill fold mirrors `collect_cached_shadowmask_membership_by_partition` on the sibling
  branch, but writes straight into the output channel instead of accumulating membership.
- Check whether converting the fill buffer to its final form already elides its copy
  through Rust's in-place collect specialization; the element types match in size and
  align. That decides whether the single-allocation row is a fix or an assertion.
- `TOP_LEVEL_MULTILAYER_FIVE_WAY_GOLDEN` pins four lifecycle paths to one inline byte
  array and is the cheapest byte gate. `ResidentLayerTracker` counts resident payloads and
  can carry the residency rows, but it is `#[cfg(test)]`, so the release rows stay
  out-of-band.
- First slice: the equivalence gate. It is the only thing that can still change the shape,
  and everything else is wasted if it fails.
- `shadowmask_bake.rs` is past 3,600 lines. Split it behaviour-preserving, own commit,
  before restructuring.

## Open questions

- The resident-partition constant — **delegated**: unchanged from today unless measurement
  says otherwise.
- The reach fraction for this stage's geometry — **delegated**: measured during the build,
  not inherited from another bake's fixture, and recorded in the plan of record.
- The supported-density margin in the last manual row — **delegated**.
