# Research — shadowmask cold working set

Read at `6168c5c9`, with `lighting-scale--lightmap-bake-incremental-flush` merged.

## Measured atlas

Three points on `stress-warren-hallway-inspection.map`. The first is the shipped
`.prl`'s section-42 header; the other two are `log_stats` output from `--release`
compiles at `--sh-probe-spacing 3.0`, killed after the lightmap stage.

| Density (m/texel) | Plane | Layers | Total texels | Section-42 bytes | Lightmap stage |
|---|---|---|---|---|---|
| shipped artifact | 512 × 512 | 36 | 9,437,184 | 37,748,736 | — |
| 0.16 | 512 × 512 | 84 | 22,020,096 | 88,080,384 | 205 s |
| 0.08 | 1024 × 1024 | 77 | 80,740,352 | 322,961,408 | 677 s |

Texels rise 3.67× for a 4× density change, so packing holds near-ideal 1/density²
scaling. The plane scales with density while layer count is geometry-driven and roughly
flat. Extrapolating to **0.04: plane 2048 × 2048, ~77 layers, 3.2e8 texels, section 42
1.29 GB (1.20 GiB)**, with the lightmap stage near forty minutes. 2048 is far below
`MAX_ATLAS_DIMENSION`, so no clamp distorts the extrapolation.

This is the one place those figures are derived; everything else refers to them. Deriving
by ratio instead gives 2.96e8 texels and 1.19 GB — close enough not to matter, far enough
apart to read as a contradiction if both appear. Plane × layers is the grounded method,
because the plane provably snaps to a power of two and layer count is near-flat across the
two measured densities.

Selected-light count is 338, corroborated two ways: the shipped section's channel table,
and the map's 763 `_shadow_type static_light_map` entities of which 338 carry
`_bake_only 0`. All 338 are `light_spot`, `light 180`, `_falloff_range 1024`. Max
promotable intensity 180 puts the `DEFAULT_ENTITY_SHADOW_MIN_INTENSITY_RATIO` threshold
at 90, which all clear. `select_entity_shadow_lights` applies no cap and no top-N. The
lightmap bakes all 763; only the 338 promotable ones reach the shadowmask.

## Residency decomposition, cold path

Terms live at the moment `build_shadowmask_from_membership_with_assignment_checkpoint`
runs, at the extrapolated failing config — 338 lights, 3.2e8 texels:

| Term | Symbol | Formula | At 1% coverage | At 100% |
|---|---|---|---|---|
| Membership | `ShadowmaskMembership.by_light` | Σ<sub>lights</sub> covered × 16 B | 16 GB | 1.6 TB |
| Inverted index | `texel_lights` in `overlap_graph_controlled` | distinct covered × ~64 B + Σ pairs × 8 B | ~8 GB | ~830 GB |
| Output | `data` in the fill, plus its `into_inner` collect | plane × layers × 4, ×2 | 2.4 GB | 2.4 GB |
| Transient layers | `chart_outputs` / `LightmapLayer.texels` | ≤ window × covered × 48 B | 0.6 GB | 58 GB |
| Adjacency | `graph` in `overlap_graph_controlled` | n² bytes | 114 KB | 114 KB |

Membership and the inverted index are live simultaneously, and 1% coverage per light
already exceeds 16 GiB between them. Spot lights with a 26 m falloff in a 250 × 208 m
warren plausibly sit near that fraction, which is the observed failure.

The output term is the one the shipped artifact understates: 37.7 MB there, 1.29 GB at
the failing density, and charged twice if the fill-buffer conversion does not elide its
copy. It does not scale with light count, so the analytic shape does not touch
it; charging it once is a separate, cheaper fix.

That size also says density 0.04 is not shippable on this map.
`filter_usable_shadowmask_section` (`renderer/src/lighting/lightmap.rs`) admits a section
on dimensions and layer count alone — 2048 and 77 clear both — and
`upload_shadowmask_texture` then uploads it verbatim as uncompressed `Rgba8Unorm`. Nothing
checks total bytes. A successful 0.04 compile would therefore produce a ~1.29 GB texture
upload, which cuts against `lighting-scale--lightmap-bake-scaling`'s premise. The manual
rows treat 0.04 as a stress probe for that reason.

`LayerTexel` is 48 B, pinned by a `size_of` const assert. `ShadowmaskMembershipTexel` is
16 B. Only the first two terms scale with selected-light count, and the analytic shape
removes both.

## Coverage is analytic

`light_texel_contribution_and_visibility` (`lightmap_bake.rs`) has exactly one path that
returns the `None` which `lightmap_layer.rs` turns into the `-1.0` sentinel:

```
let (contribution, to_light) = light_contribution_and_direction(light, world_p, surface_normal);
if contribution.length_squared() <= 1.0e-12 {
    return (Vec3::ZERO, Vec3::ZERO, None);   // only None; returns before any ray
}
let v = soft_visibility(world_p, surface_normal, light, seed, area_sample_count, trace);
```

Every later return is `Some`. `soft_visibility` is the only consumer of the `trace`
closure, and `light_contribution_and_direction(light, surface_point, surface_normal)`
takes no BVH, no primitives, and no closure. So membership is exactly
`contribution.length_squared() > 1.0e-12` — closed-form on light parameters, texel world
position, and surface normal.

`collect_layer_membership` filters `raw_visibility < 0.0`, not `>= 0.0`. NaN is therefore
*retained*, which its comment and `nan_raw_visibility_is_retained_with_legacy_inclusion_semantics`
both state. NaN can only arise from `soft_visibility`, downstream of the contribution
gate, so it is in both the baked and the analytic set. The sets agree there rather than
diverging.

### The divergence surface is the walk, not the epsilon

`chart_raster::chart_interior_dims` and `chart_texel_world_position` are already shared by
`lightmap_bake`, `lightmap_layer`, `animated_light_weight_maps`, and
`animated_light_chunks`, so reusing them looks sufficient. It is not.
`bake_light_layer_chart_controlled` returns an empty buffer for any chart whose UV extent
is non-positive, *before* it reaches `chart_interior_dims` — which clamps both dimensions
to at least 1 so a degenerate chart still maps to one texel. A graph pass assembled from
those two helpers therefore covers one texel per degenerate chart that the bake covers
with none, adds spurious adjacency edges, and can change channel assignment and bytes.

The by-construction answer is a coverage-only mode of the existing per-chart walk, which
already holds the skip, the padding offsets, and the contribution test together. Sharing
only the predicate and rebuilding the walk around it reintroduces the divergence in a
place no test names.

### Cost of the graph pass

Unpruned it is selected lights × atlas texels contribution tests, order 1e11
at the failing density (338 × 3.2e8). That is not viable, so the prune is load-bearing.

Do not size it from the published reach fractions.
`lighting-scale--cold-sh-bake-falloff-early-out` measured 4–6% of the light set per site
(median 7, p95 10), but `lighting-scale--cold-bake-reaching-light-spike` says of its own
number: it "bounds the mechanism, not the shipping win — do not project the fixture number
onto real content." Two further gaps: those figures come from an **exact per-point range
test**, while this stage prunes by light bounds against chart bounds, which the same plan
measures as 3–5× looser; and they were taken on other bakes' geometry. Order 5e9 is a
plausible landing spot and still has headroom against 1e11, but the executor measures this
stage's own fraction.

The machinery to reuse already ships — `affinity_grid` decompose plus `ReachIndex`
inversion, consumed by the direct SH bake. The reaching-light spike is scoped to the cold
base-indirect SH bake and the cold lightmap bake; it never mentions the shadowmask stage,
so no plan owns pruning here.

Parallelizing the pass puts it under the `governor.rs` contract: `enter` once at a work
item's outermost boundary, never a bare `checkpoint`, which honors pause but ignores the
`-j` cap. `lightmap-bake-throughput` documents that contract and the nested-wait deadlock
it prevents, and it reached into this stage to construct a `BakeControl` for exactly this
reason.

## Prior commitments

`shadowmask-bake-scaling` (done) is the direct predecessor. It windowed the full
`LightmapLayer` set and introduced the 16 B membership entry — a ~3× constant-factor win
that took `stress-warren-lit` (157 lights) from ~14 GB to order 1 GB. It states the
current failure in advance: *"finer density or many more lights re-consume the
headroom."* 338 lights at density 0.04 is that case.

It calls membership *"the irreducible working set of the global-per-light coloring"* and
rejects alternative (a), **per-tile coloring**, because id-42 stores one channel per light
global across all atlas texels, so assignment must see a light's whole overlap set at
once. That foreclosure is on assigning channels per tile and still holds. It does not
reach graph *construction*, which can be ordered freely so long as coloring runs once over
the completed adjacency.

The irreducibility claim rests on an unstated premise — that coverage is knowable only by
baking. It is not, and the same premise sits under `build_pipeline.md` §PRL section IDs:
*"the coloring holds every covering light's record at once, so the working set is
irreducibly light-scaling."* Under the analytic shape the light × texel term disappears
and that sentence becomes false, not merely improved. It is scheduled for revision at
promotion.

`lighting-scale--compile-peak-ram` (done) rejected spill for SH-delta because coarsening's
neighbor-coupled smoothing random-accesses the whole section and would thrash. SH-delta
also has no analytic escape: its dense payload is a ray-bake result, not a predicate.

One foreclosure to name. The per-pair record is the only structure where per-texel
visibility values and cross-light adjacency coexist. Deleting it costs nothing for the
shipped intensity-ordered drop priority, which reads light parameters only. A
contribution-weighted or coverage-weighted retention priority — plausible for a future
no-drop or quality pass — would have to re-materialize the term in some form.

`shadowmask-no-drop-atlas` (draft) replaces four-colour-with-drops with block-aware slot
assignment and changes the id-42 format. It consumes the adjacency this work produces;
building that adjacency without a bake makes its job cheaper, not harder.

## Fallback shape: bounded membership

If the equivalence gate fails, the design reverts to bounding the record instead of
removing it. Recorded here so the fallback is not re-derived.

Split the two consumers — coloring needs coverage, the fill needs values — and tile graph
construction, accumulating adjacency globally so coloring still sees the complete graph.
Size the tile from a budget parameter rather than from a fixture, mirroring
`--sh-delta-working-set-max-size` on the completing side, because stress-warren
under-approximates production geometry and any constant-factor headroom is the card
`shadowmask-bake-scaling` already spent. Replace `texel_lights` with a sort-and-scan over
each tile, worth roughly 1.5–2× on its own. Carry values between the passes in a
run-scoped spill beside the staged output — `--release` never creates a cache directory,
since `construct_stage_cache` returns `None` before reading `args.cache_dir` — capped, and
removed on every exit path including an OOM kill, which a `dist` stage-6 bake can hit and
which would otherwise leave a multi-GB temp file inside the payload root.

Under that shape, quantizing `raw_visibility` to `u8` at record time is byte-identical and
shrinks the spill: the fill's quantizer is pure, `f32::clamp` propagates NaN, and the cast
saturates, so both orders yield zero for NaN. `collect_layer_membership`'s negative filter
must still run on the `f32`, because after quantization a NaN is indistinguishable from a
genuine zero.

## Measurement precedent

No peak-RSS, allocation counter, or measured memory report exists anywhere in the
compiler; `reporter.rs` tracks stages and timing only, and every footprint number today is
a projection — `gate_delta_working_set`'s. `shadowmask-bake-scaling` worked with that and
said so: process RSS is measured out-of-band (`VmHWM`, `/usr/bin/time -v`) and reported as
a manual gate, not a unit test.

`lighting-scale--lightmap-bake-incremental-flush` takes no position on in-process probes.
It scopes peak RSS out — "whole-compiler completion and peak RSS… shadowmask membership
and output residency are a follow-on concern" — and names this work as that follow-on. Its
acceptance does pin log hygiene: one `log::info` footprint line, per-partition breakdowns
behind `--verbose`.

`ResidentLayerTracker` is `#[cfg(test)]` and cannot observe a `--release` run. The
shadowmask dimension log line is behind `--verbose` and runs *after* the allocation that
fails. The whole-`.prl` determinism test is `#[ignore]`d and runs a fixture that may not
emit section 42, so no shadowmask determinism coverage exists.

## Related but not consumed

`dist` packaging runs bakes one at a time because "a second concurrent bake multiplies
peak shadowmask-atlas memory" (`build_pipeline.md` §Distribution packaging). The analytic
shape removes the light-scaling term that rationale rests on; the serialization may still
be wanted for throughput, but its stated reason changes.

## Ordering pins

Orderings the Decisions imply and did not state. Each has an Acceptance row citing it.

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| ord-zero-coverage | A selected light whose bounds intersect no chart bounds, so the prune removes it before any chart walk. | Prune, then graph node construction, then coloring, then the channel table write. | The light is still node *i* in a graph of `selected.len()` nodes, with zero edges. It receives a real channel, not the dropped sentinel, and every later light keeps the channel it has today. Pruning may remove work; it may never remove a node. |
| ord-prune-unsound | Two lights share one texel of a chart whose bounds the prune says one light's bounds do not reach. | Prune, graph, coloring, then the parallel fill. | Same-channel lights would store to one byte offset from two threads. The parallel fill's relaxed stores are licensed only by graph-guaranteed disjointness, so the prune must be a proven superset of analytic coverage, not an approximation. |
| ord-graph-barrier | Coloring is reached while one tile of the parallel graph pass is still running. | Every graph work item joins; then adjacency is read; then coloring. | Coloring never observes partial adjacency. Draining the governor is not the barrier — `set_permits` does not preempt, so a permit-free governor and a finished pass are different states. |
| ord-discovery | Two lights overlap on two texels in two different tiles; tile order reverses between runs. | Tiles discover edges in either order; per-tile adjacency merges; coloring reads the union. | Identical edge set, degree ordering, and channel table. Adjacency is a set union, not first-writer-wins, with no per-tile edge budget that could drop a duplicate discovery. |
| ord-dropped-partition | A light coloring dropped, in a run whose layer cache is cold. | Assignment, then the per-partition fill decides to bake or skip. | Bytes unchanged either way. Whether the dropped light's layer cache entries are populated is a stated rule, because the next compile's cost depends on it. |
| ord-mixed-partition | Within one atlas layer, some lights' partitions hit cache and some are baked, interleaved. | Per-partition read or bake, then channel write, then drop. | Both branches advance the same unit count for the same partition. Completed never exceeds the published total and reaches it exactly once. |
| ord-final-unit | The last partition of the last layer completes, then the section is assembled, then the memo is written. | Layer-outer fill, then finalization, then the section memo write. | Exactly one unit is withheld through finalization and the memo write. Layer-outer fill has no natural "light zero's unit", so the withheld unit is named explicitly. |
| ord-pause-midgraph | Pause is set, then `-j` lowered, while the graph pass has covered some tiles and not others. | The pass parks; permits re-target; the pass resumes and finishes; then coloring. | Accumulated adjacency survives the park untouched. Lowering `-j` does not preempt in-flight items, so the pass must not read a permit-free governor as completion. |
| ord-one-light-changed | One selected light's parameters change; the other lights are untouched. | Section memo misses; the graph pass re-runs whole, reading no cache; the fill reads warm partitions and bakes one. | Unchanged lights' layer partitions all hit. The whole graph pass is the recompile's new floor cost — the price of the decision that it reads no cache. |
| ord-filtered-alloc | Every selected light is filtered out, on the cached path. | Selection filtering, then — on the cached path only — the fill machinery runs over an empty light set. | Both paths allocate the output exactly once. Today the uncached path short-circuits to one filled buffer while the cached path builds the atomic buffer and collects it. Byte equality hides this, so the row asserts allocations, not bytes. |
