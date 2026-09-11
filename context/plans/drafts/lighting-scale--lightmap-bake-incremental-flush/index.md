# Lightmap Bake — Incremental Bake-and-Flush (Compile-Time Peak RAM)

## Goal

Bound the lightmap bake's peak resident working set to one atlas layer (cold path) and to one light's
contribution to a single atlas layer at a time, over a one-layer accumulator (warm
path), instead of scaling with the whole map. Bake peak RAM
scales with total texels, so as maps combine per-surface scale regions with geometric
detail the bake breaks down on constrained-RAM machines; this hardens prl-build against
that breakdown. Output stays byte-for-byte identical to the current bake — an
allocation-lifecycle change, not a format or quality change. The bake already has the
seams that make this clean: dilation and the irradiance encoder both run per atlas layer,
the landed direction encode reduces and RG8-encodes per layer plane, and leaf cohesion
(all of a leaf's charts on one atlas layer) makes an atlas layer a self-contained
bake→encode→drop partition.

## Direction

**Problem.** Bake peak RAM scales with the whole atlas: `layer_count` uncompressed `f32`
layers resident at once (cold path), and all `N_lights` per-light layers resident before a
single composite (warm path). Peak tracks total texels, so it binds as maps grow in scale
and detail — the durable cause is the working-set lifecycle, not any one map: the bake holds
the whole atlas resident when it need only hold one partition. Observed:
`stress-warren-hallway-inspection` at the 0.04 default density OOM'd on the owner's Windows
box, blocking `dist` build test runs. The same bake on a roomier box was throughput-bound at
~7.8 GB peak (`context/plans/done/lighting-scale--lightmap-bake-scaling/research.md`) — a
different RAM ceiling, not a contradiction. The landed scale-region work lowers texel counts
on decorative surfaces and may ease this map, but does not remove the lifecycle cause.

**Prior commitments.** This is the lightmap track of the `lighting-scale--compile-peak-ram`
epic, which sanctions bounded-working-set work and reserves the "a previously-OOMing bake
completes" posture for the lightmap bake because it partitions cleanly by atlas layer.
Landed dependencies: `lighting-scale--lightmap-bake-scaling` (per-surface scale regions; RG8
half-res direction encode — encode-only, so the in-memory working set this plan bounds is
unchanged) and `lightmap-bake-throughput` (parallelized both bake paths into per-chart
parallel maps — this plan's per-layer partition and per-light fold compose with that
parallelism; the ordered accumulate is the only serialization point). The byte-identity and
whole-`.prl` determinism gates are the epic's shared contract.

**Alternatives rejected.** Shelve until a fixture OOMs on standard hardware — rejected: the
OOM already blocked a `dist` build on the owner's machine, and the fix is a reversible,
byte-gated allocation-lifecycle change cheap to land now (undo is allocation order plus a
cache-version bump). Cold path only, drop the warm fold — rejected: warm-path peak binds
during lighting iteration, where a section-cache miss materializes all `N_lights` layers on
the same constrained machines the author works on. Attack throughput instead — orthogonal;
already addressed by `lightmap-bake-throughput`, and it does not lower peak RAM.

## Scope

### In scope

- **Cold path (monolithic bake):** allocate, bake, dilate, encode, and drop the
  uncompressed `f32` composited buffers **one atlas layer at a time**, appending each
  layer's encoded irradiance/direction slice to the layer-major blob. Resident `f32`
  working set bounded to one layer rather than `layer_count` layers.
- **Warm path (per-light incremental cache):** fold the per-light contributions into a
  **one-layer** accumulator, **atlas layer outer and lights inner**, dropping each light's
  one-layer partition as soon as it is added, rather than holding all `N_lights` layers or a
  whole-atlas accumulator resident before a single composite. Resident set bounded to one
  light's one-layer partition plus one layer's accumulator.
- Re-key / re-slice the per-light layer cache blobs as needed so a single fold step
  loads only the texels it needs; bump the layer cache-format version (dev-local
  cache regenerates on next bake).
- Preserve the byte-identity gate between the warm composite and the cold monolithic
  bake, and the whole-`.prl` determinism gate.

### Out of scope / non-goals

- **Any change to `.prl` output bytes.** The baked section is identical; only the
  order and lifetime of allocations change. The byte-identity gate is the contract.
- **Runtime VRAM / the 256-layer texture budget.** The now-landed sibling plan
  `lighting-scale--lightmap-bake-scaling` owns the shipped-atlas footprint; its win is
  bytes-per-layer (coarser dims + `Rg8` direction), not fewer layers — `layer_count` is
  unchanged.
- **The direction downsample / `Rg8` channel-drop.** Landed in that sibling plan
  (`Rg8Unorm` octahedral, half-res per axis via `DIRECTION_TEXEL_SCALE = 2`). This plan's
  per-layer encode wraps the already-landed direction path rather than reshaping it (see
  Sequencing).
- **The SH storage-buffer / delta footprint problem.** Unrelated GPU budget, separate
  spec.
- **The second-level composited-section cache.** It memoizes the whole section and is
  unchanged; only the per-light layer level is re-scoped.

## Tasks

### Task 1: Per-layer bake-encode-drop (both paths)

Restructure the monolithic bake so it processes one atlas layer at a time: allocate a
single-layer composited buffer sized for one layer, bake only the faces whose chart
placement lands on that layer, dilate (already per-layer), encode that layer's
irradiance and direction slice, append to the growing layer-major blobs, and drop the
`f32` buffers before advancing to the next layer. Iterate atlas layers sequentially in
ascending index order (0..`layer_count`) and append in that order, matching
`encode_section`'s per-layer concatenation; do not parallelize the outer per-layer loop and
append on completion — that makes the blob order (and the `.prl`) nondeterministic and holds
more than one layer resident, breaking both the byte-identity gate and the peak-RAM bound.
This bounds the uncompressed
working set to one layer instead of `layer_count`. The result must be byte-identical
to the current whole-atlas bake: per-layer bake + per-layer dilate + per-layer encode
+ concatenation already equals the whole-atlas path because every encode stage is
layer-local: dilation never crosses a layer boundary (`CompositedAtlas::dilate` loops per
layer), the irradiance BC6H encoder already emits one block blob per layer, and the
direction path reduces and RG8-encodes each layer's plane independently
(`reduce_direction_atlas` reads only its matching input plane, `encode_direction_rg8` is a
per-texel map), so per-layer slicing concatenates to the identical direction blob. The
warm path's composite accumulator carries the same whole-map `f32` cost; bounding it to
one atlas layer is realized jointly with Task 2's per-light fold, since the current
composite consumes every light's layer at once and cannot go per-layer until that fold
is nested inside the per-atlas-layer loop. The landed per-chart parallel scatter still
applies within a layer: leaf cohesion keeps each leaf's charts on one atlas layer, so the
parallel per-chart bake targets the current single-layer buffer rather than the whole atlas.
The reused `scatter_chart_into_atlas` addresses its destination by the chart's global
`ChartPlacement.layer`, so the single-layer buffer needs the placement rebased to layer 0
(or a layer-agnostic scatter); an unrebased `placement.layer > 0` scattered into a one-layer
buffer writes out of bounds (and trips the `placement.layer < layer_count` assert in debug).
Keep the face→layer grouping deterministic (leaf order) so the bake stays reproducible.

### Task 2: Incremental per-light fold (warm path)

Replace the warm path's "collect all `N_lights` per-light layers, then composite once"
with an incremental fold nested inside Task 1's per-atlas-layer loop, atlas layer outer and
lights inner: for each atlas layer, zero a one-layer accumulator, then walk lights in the
existing global order and fold each light's contribution to that layer — sliced from the
re-scoped per-light blob on a cache hit, or baked for that layer's charts alone on a miss
(filtered by `ChartPlacement.layer`, so a miss never materializes the light's other layers)
— dropping each light's one-layer partition before the next; then encode-and-append that
layer's accumulator and drop it before advancing. This collapses the dominant warm-path
term — `N_lights × covered_texels × per-texel-bytes` — to a single per-light partition
resident at a time, an ~`N_lights`× reduction on that term, and holds the accumulator to one
layer rather than the whole atlas. A light-outer fold keeps a whole-atlas accumulator
resident across every light and misses that bound. Folding lights in the same per-texel
order the monolithic bake sums them keeps the composite bit-identical, so the byte-identity
gate holds; the ordered accumulate is the only serialization point, and each light's
partition still bakes with the landed per-chart parallelism. Re-scope the per-light cache
blob (which currently spans all layers) so a fold step loads only the partition it needs,
and bump the layer cache-format version to invalidate stale blobs. Warm-path peak resident
is therefore one light's one-layer partition plus one layer's accumulator. The win lands on
section-cache-miss warm bakes — the lighting- or geometry-iteration case that materializes
per-light layers; a no-edit rebuild served by the second-level section memo never
materializes them and is unaffected.

## Sequencing

**Cross-plan dependency (satisfied — scaling has landed):**
`lighting-scale--lightmap-bake-scaling` is merged. It reshaped the direction encode to
`Rg8Unorm` octahedral at half-res per axis (`DIRECTION_TEXEL_SCALE = 2`, via
`reduce_direction_atlas` + `encode_direction_rg8`, both driven from `encode_section`). This
plan's per-layer encode-and-append wraps that already-landed direction path; per-layer
slicing stays byte-identical because the reduction and encode are layer-local (Task 1).

**Phase 1 (sequential):** Task 1 — establishes the per-layer partition iterator and
the encode-and-append assembly both paths reuse.
**Phase 2 (sequential):** Task 2 — nests the per-light fold inside Task 1's
per-atlas-layer loop (atlas layer outer, lights inner) to bound the warm accumulator to one
layer.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Baked `.prl` bytes identical to the pre-change bake | current bake (the contract) | Task 1 per-layer slice + concatenation; Task 2 ordered per-light fold — threatened if a slice crosses a layer plane or the fold reorders lights | AC 1 |
| Warm composite equals cold monolithic, bit-for-bit | `incremental-bake-per-element` gates | Task 2 fold sums lights in the cold path's order | AC 2 |
| Uncompressed working set bounded to one partition | Task 1 (one atlas layer), Task 2 (one light's one-layer slice) | each buffer dropped before the next is allocated; the accumulator is the only retained buffer | AC 3, AC 4 |

### Ordering pins

Concrete orderings the bake must honor. Each is testable; each names the task that executes it.

| id | Scenario | Ordering | Expected outcome | Kind | Task |
|---|---|---|---|---|---|
| OP1 | Cold per-layer encode + append on a multi-layer atlas | Encode each atlas layer's irradiance and direction slice and append; outer per-layer loop sequential ascending | Slices appended ascending `0..layer_count`; blob byte-identical to the whole-atlas `encode_section`, and identical across two runs | determinism / byte-identity | Task 1 |
| OP2 | Scatter a chart on array layer L>0 into a one-layer buffer | Allocate a one-layer buffer for layer L; parallel-scatter its charts (`placement.layer == L`) | Placement rebased to layer 0; no out-of-bounds write, no assert trip; buffer equals layer L's slice of the whole-atlas bake | correctness | Task 1 |
| OP3 | Warm fold nesting on a multi-layer, multi-light atlas | Atlas layer outer, lights inner in global order; one-layer accumulator; encode-and-drop per layer | Peak resident = one light's partition + one layer's accumulator; composite bit-identical to cold | working-set / byte-identity | Task 2 |
| OP4 | Warm fold with zero non-Sdf lights (every light `ShadowType::Sdf`) | Light set for the fold is empty; fold runs zero iterations; composite the empty layer set | Reshaped warm output byte-identical to the pre-change warm output — the pre-existing empty-slice divergence in `composite_layers` (empty-slice fallback vs cold coverage) is reproduced, not fixed, so the warm-vs-cold gate (AC 2) is not asserted for this case | byte-identity edge | Task 2 |
| OP5 | Fold a light reaching zero texels (fully out of influence) | Light layer enumerates the full covered set with `0.0` terms; fold, then drop | Adds `0.0` (no perturbation), ORs coverage; result matches cold | byte-identity edge | Task 2 |
| OP6 | Atlas layer with zero charts | Iterate layers `0..layer_count` | Cannot occur — the packer places at least one leaf per opened layer; each layer's full-size buffer is still allocated and encoded | invariant | Task 1 |
| OP7 | Per-layer parallel scatter lifecycle | Parallel-scatter a layer's charts, then dilate, encode, drop | The parallel scatter joins before the drop; no chart task writes after the layer buffer is dropped | lifecycle | Task 1 |

## Acceptance criteria

- [ ] The baked `.prl` is byte-identical to the pre-change bake for a multi-layer
  fixture map (cold path and warm path both).
- [ ] The byte-identity gate between the warm per-light composite and the cold
  monolithic bake still passes.
- [ ] Compile-time peak RSS is measured and reported on the map and density AC 4 caps
  (`stress-warren-hallway-inspection` at 0.04), as absolute figures for both the pre-change
  bake and the reshaped bake: the cold-path uncompressed working set scales with one atlas
  layer rather than `layer_count` — its share of peak RSS drops by ≥ `(layer_count − 1) /
  layer_count` — and the warm-path per-light layer resident set drops ~`N_lights`× (from all
  lights resident to one at a time). The reported pre-change cold-path peak RSS and
  post-change peak are the figures AC 4 caps between, so AC 4 reuses them without re-running
  the retired pre-change lifecycle.
- [ ] With the process address space capped below the pre-change cold-path peak RSS
  reported by the criterion above and at or above the post-change peak (Linux `ulimit -v`,
  Windows Job Object memory limit), the reshaped bake of the map and density that blocked
  the `dist` build (`stress-warren-hallway-inspection` at 0.04) completes and writes a
  `.prl` that passes the byte-identity gate. The pre-change peak is the figure that
  criterion reports, so the retired lifecycle need not be re-run.
- [ ] Re-baking the same map twice yields byte-identical `.prl` output.
- [ ] A normal (non-verbose) bake gains no new per-item log spam; any per-partition
  memory or size breakdown appears only under `-v`/`--verbose`, and any footprint
  summary is a single `log::info` line.

## Rough sketch

Bake — `crates/level-compiler/src/lightmap_bake.rs`: `CompositedAtlas` (irradiance
`Vec<f32>`, direction `Vec<Vec3>`, coverage `Vec<bool>`, layer-major) is ~29 B/texel
across all layers (16 + 12 + 1); the direction buffer stays `Vec<Vec3>` in memory even
after the sibling's `Rg8` switch — that switch is encode-only, so the working set this plan
bounds is unchanged. `bake_monolithic_atlas` (→ `bake_monolithic_atlas_controlled`)
allocates the atlas whole, scatters every baked face into it via a per-chart `par_iter`
(from `lightmap-bake-throughput`), and dilates (already a per-layer loop); its caller
`bake_lightmap_controlled` then calls `encode_section` —
irradiance loops per layer for BC6H, direction is a single-pass `encode_direction_rg8` after
an optional per-axis `reduce_direction_atlas` (`DIRECTION_TEXEL_SCALE`, default 2). The
per-layer partition is the seam: group faces by `ChartPlacement.layer` (`chart_raster.rs`;
leaf cohesion via `place_leaf` keeps a leaf's charts on one layer), encode a layer's slice,
drop. `DEFAULT_TEXEL_DENSITY_METERS = 0.04`.

Warm path — `crates/level-compiler/src/pipeline.rs` (the lightmap section of
`run_after_parsing`): today builds a `Vec<LightmapLayer>` over all lights (each from cache or
`lightmap_layer::bake_light_layer_controlled`), then `lightmap_layer::composite_layers`,
then `dilate` + `encode_section`. `LayerTexel`/`LightmapLayer` live in `lightmap_layer.rs`;
each `LightmapLayer` is dense over covered texels at `size_of::<LayerTexel>()` (48 B,
static-asserted; `weighted_dir: [f32; 3]` is unchanged by the `Rg8` switch) — the
`N_lights`× term. The fold replaces the collect-then-composite with an atlas-layer-outer,
light-inner accumulate-and-drop loop, holding one light's one-layer partition and a
one-layer accumulator (the cold path at the same site sums lights inline per texel, so it
carries only the whole-map `f32` term, addressed by Task 1). Per-light bakes stay parallel
per chart; the accumulate is ordered.

Cache: the per-light layer cache is compiler-internal and dev-local, keyed on
`LAYER_FORMAT_VERSION` (`lightmap_layer.rs`, currently 4); bumping it regenerates the cache
on the next bake. The second-level composited-section memo (`LIGHTMAP_SECTION_VERSION`,
currently 2) is unchanged.

Byte-identity / determinism gates already in tree (the AC preserves these):
`composite_matches_monolithic_atlas_bit_for_bit`,
`multi_layer_composite_matches_monolithic_bit_for_bit`, and
`lightmap_composite_equals_monolithic_on_fixtures` (`lightmap_layer.rs` tests) compare the
pre-BC6H composite against the monolithic bake;
`plain_cli_is_deterministic_and_preserves_progress_summary_contracts` and
`gate_heavily_lit_cold_compact_sh_output_is_deterministic` (`tests/compiler_cli_contract.rs`)
gate whole-`.prl` determinism.

Logging discipline: mirror the existing bake summary — `lightmap_bake::log_stats` is a
single `log::info!` line, called only under the `if args.verbose` gate in `pipeline.rs`; any
per-partition breakdown this plan adds stays behind that same gate. Non-verbose bakes gain
nothing.
