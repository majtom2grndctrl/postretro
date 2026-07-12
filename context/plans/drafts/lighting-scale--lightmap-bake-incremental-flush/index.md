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
- **Runtime VRAM / the 256-layer texture budget.** Sibling plan
  `lighting-scale--lightmap-bake-scaling` owns the shipped-atlas footprint.
- **The direction downsample / Rg8 channel-drop.** Same sibling plan. This plan must
  slot cleanly under whatever direction-encode shape that plan lands (see
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
+ concatenation already equals the whole-atlas path because dilation never crosses a
layer boundary and the irradiance encoder already emits per-layer block blobs. The
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

**Cross-plan dependency:** implement `lighting-scale--lightmap-bake-scaling` first.
Both plans edit the direction-encode path in the bake; the scaling plan reshapes the
direction encode (coarser dims + `Rg8`), and this plan wraps the per-layer
encode-and-append loop around it. Landing scaling first means this plan's per-layer
encode simply calls the already-reshaped direction encoder.

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
across all layers today; `bake_monolithic_atlas` allocates it whole, bakes every
face, dilates (already a per-layer loop), then `encode_section` (irradiance already
loops per layer for BC6H). The per-layer partition is the seam: bake faces by
`ChartPlacement.layer`, encode a layer's slice, drop. `DEFAULT_TEXEL_DENSITY_METERS =
0.04`.

Warm path — `crates/level-compiler/src/main.rs`: today builds a
`Vec<LightmapLayer>` over all lights, then `composite_layers(&layers, …)`, then
`dilate` + `encode_section`. Each `LightmapLayer` is dense over covered texels at
`size_of::<LayerTexel>()` (48 B) per texel — the `N_lights`× term. The fold replaces
the collect-then-composite with an accumulate-and-drop loop over lights (and the cold
path at the same site sums lights inline per texel, so it has only the whole-map
`f32` term, addressed by Task 1). The per-light layer cache format version gates
stale-blob invalidation.

Cache: the layer cache is compiler-internal and dev-local; a format-version bump
regenerates it on the next bake. The second-level section cache is unchanged.

Logging discipline: mirror the existing bake summary — one-line `log::info`, with any
per-partition breakdown behind the `--verbose` gate that already wraps
`lightmap_bake::log_stats`. Non-verbose bakes gain nothing.
