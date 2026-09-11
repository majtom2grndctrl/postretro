# Lightmap Bake — Incremental Bake-and-Flush (Compile-Time Peak RAM)

## Goal

Bound the lightmap bake's peak resident memory to one layer-partition (and, on the
warm path, one light at a time) instead of the whole map, so interior-heavy and
larger maps compile at fine density without exhausting RAM. Output stays
byte-for-byte identical to the current bake — this is an allocation-lifecycle change,
not a format or quality change. The bake already has the two seams that make this
clean: dilation runs per atlas layer independently, and the irradiance encoder
already loops per layer; leaf cohesion (all of a leaf's charts on one atlas layer)
makes an atlas layer a self-contained bake→encode→drop partition.

## Research note — premise re-validation (from now-landed sibling `lighting-scale--lightmap-bake-scaling`)

The now-landed sibling scaling plan's baseline measurement
(`context/plans/done/lighting-scale--lightmap-bake-scaling/research.md`) re-tests this
plan's motivating claim. The full `stress-warren-hallway-inspection` fixture baked at the
0.04 default density did **not** exhaust RAM: it peaked ~7.8 GB (peak VmHWM) and was bounded
by bake throughput (~70 min projected to complete; reached 93% at the ~65 min budget), and
neither atlas cap was hit. So "a bake that previously
exhausted RAM on the stress fixture" (Goal, AC) is not demonstrated by the largest current
interior fixture — peak RAM scales with total texels, so a larger map would eventually
bind, but no shipped fixture does today. Before building, either name a fixture that
actually OOMs at fine density, or reframe the win as the bounded-working-set invariant this
plan delivers regardless (peak drops from `layer_count` uncompressed layers to one). The
mechanism is unaffected; only the motivating claim needs re-grounding. Bake throughput —
the confirmed 0.04 barrier — is a separate concern this plan does not address.

## Scope

### In scope

- **Cold path (monolithic bake):** allocate, bake, dilate, encode, and drop the
  uncompressed `f32` composited buffers **one atlas layer at a time**, appending each
  layer's encoded irradiance/direction slice to the layer-major blob. Resident `f32`
  working set bounded to one layer rather than `layer_count` layers.
- **Warm path (per-light incremental cache):** fold each light's contribution into
  the composite accumulator **one light at a time**, dropping each per-light layer as
  soon as it is added, rather than holding all `N_lights` layers resident before a
  single composite. Resident per-light layer set bounded to one.
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
`f32` buffers before advancing to the next layer. This bounds the uncompressed
working set to one layer instead of `layer_count`. The result must be byte-identical
to the current whole-atlas bake: per-layer bake + per-layer dilate + per-layer encode
+ concatenation already equals the whole-atlas path because every encode stage is
layer-local: dilation never crosses a layer boundary (`CompositedAtlas::dilate` loops per
layer), the irradiance BC6H encoder already emits one block blob per layer, and the
direction path reduces and RG8-encodes each layer's plane independently
(`reduce_direction_atlas` reads only its matching input plane, `encode_direction_rg8` is a
per-texel map), so per-layer slicing concatenates to the identical direction blob. The
warm path's composite accumulator carries the same whole-map `f32` cost and gets the
same per-layer treatment. Keep the face→layer grouping deterministic (leaf order) so
the bake stays reproducible.

### Task 2: Incremental per-light fold (warm path)

Replace the warm path's "collect all `N_lights` per-light layers, then composite
once" with an incremental fold: bake or cache-load one light's contribution, add it
into the composite accumulator in the existing global light order, then drop that
light's layer before loading the next. This collapses the dominant warm-path term —
`N_lights × covered_texels × per-texel-bytes` — to a single per-light layer resident
at a time, an ~`N_lights`× reduction on that term. Folding in the same order the
monolithic bake sums lights per texel keeps the composite bit-identical, so the
byte-identity gate holds. Where a per-light cache blob currently spans all layers,
re-scope it so a fold step loads only the partition it needs (combining with Task 1's
per-layer partitioning); bump the layer cache-format version to invalidate stale
blobs. The composite accumulator itself is bounded by Task 1's per-layer treatment,
so warm-path peak resident is ~one light × one layer plus one layer's accumulator.

## Sequencing

**Cross-plan dependency (satisfied — scaling has landed):**
`lighting-scale--lightmap-bake-scaling` is merged. It reshaped the direction encode to
`Rg8Unorm` octahedral at half-res per axis (`DIRECTION_TEXEL_SCALE = 2`, via
`reduce_direction_atlas` + `encode_direction_rg8`, both driven from `encode_section`). This
plan's per-layer encode-and-append wraps that already-landed direction path; per-layer
slicing stays byte-identical because the reduction and encode are layer-local (Task 1).

**Phase 1 (sequential):** Task 1 — establishes the per-layer partition iterator and
the encode-and-append assembly both paths reuse.
**Phase 2 (sequential):** Task 2 — consumes Task 1's per-layer partitioning to bound
the per-light fold; shares the composite accumulator lifecycle.

## Acceptance criteria

- [ ] The baked `.prl` is byte-identical to the pre-change bake for a multi-layer
  fixture map (cold path and warm path both).
- [ ] The byte-identity gate between the warm per-light composite and the cold
  monolithic bake still passes.
- [ ] Compile-time peak RSS on a fine-density multi-layer stress bake drops materially
  versus the pre-change bake and is measured and reported: the cold-path uncompressed
  working set scales with one atlas layer rather than `layer_count`, and the
  warm-path per-light layer resident set drops ~`N_lights`× (from all lights resident
  to one at a time).
- [ ] A bake at a density that produced multiple atlas layers and previously
  exhausted RAM on the stress fixture now completes; where it did not previously OOM,
  the uncompressed-buffer share of peak RSS is reduced by ≥ `(layer_count − 1) /
  layer_count`.
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
allocates the atlas whole, scatters every baked face into it, and dilates (already a
per-layer loop); its caller `bake_lightmap_controlled` then calls `encode_section` —
irradiance loops per layer for BC6H, direction is a single-pass `encode_direction_rg8` after
an optional per-axis `reduce_direction_atlas` (`DIRECTION_TEXEL_SCALE`, default 2). The
per-layer partition is the seam: group faces by `ChartPlacement.layer` (`chart_raster.rs`;
leaf cohesion via `place_leaf` keeps a leaf's charts on one layer), encode a layer's slice,
drop. `DEFAULT_TEXEL_DENSITY_METERS = 0.04`.

Warm path — `crates/level-compiler/src/pipeline.rs` (the lightmap section of
`run_pipeline`): today builds a `Vec<LightmapLayer>` over all lights (each from cache or
`lightmap_layer::bake_light_layer_controlled`), then `lightmap_layer::composite_layers`,
then `dilate` + `encode_section`. `LayerTexel`/`LightmapLayer` live in `lightmap_layer.rs`;
each `LightmapLayer` is dense over covered texels at `size_of::<LayerTexel>()` (48 B,
static-asserted; `weighted_dir: [f32; 3]` is unchanged by the `Rg8` switch) — the
`N_lights`× term. The fold replaces the collect-then-composite with an accumulate-and-drop
loop over lights (the cold path at the same site sums lights inline per texel, so it carries
only the whole-map `f32` term, addressed by Task 1).

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
