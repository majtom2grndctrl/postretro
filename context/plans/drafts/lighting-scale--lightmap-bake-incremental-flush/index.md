# Lightmap Bake — Incremental Bake-and-Flush (Compile-Time Peak RAM)

## Goal

Bound the lightmap bake's peak resident memory to one layer-partition (and, on the
warm path, one light at a time) instead of the whole map, so interior-heavy and
larger maps compile at fine density without exhausting RAM. Output stays
byte-for-byte identical to the current bake — this is an allocation-lifecycle change,
not a format or quality change. The bake already has the two seams that make this
clean: dilation runs per atlas layer independently, and the irradiance encoder
already loops per layer; each chart lands on exactly one atlas layer (the whole-chart spill rule — a chart
overflowing a layer's area spills to a new layer, never splits), so partitioning
faces by `ChartPlacement.layer` makes an atlas layer a self-contained
bake→encode→drop partition.

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
- Fold from the existing whole-light `"lightmap_layer"` cache blobs in memory —
  decode a light's contribution, fold it into the current layer-partition's
  accumulator, drop it. The on-disk blob format and its cache key are unchanged: no
  re-key, no `LAYER_FORMAT_VERSION` bump. The shadowmask bake (a co-consumer of the
  same `"lightmap_layer"` key) and the second-level `"lightmap_section"` memo are
  therefore untouched.
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
- **The second-level composited-section cache and the `"lightmap_layer"` on-disk
  format.** Both are unchanged — this plan re-schedules the in-memory fold only, so
  the `"lightmap_section"` memo's fingerprint keys and the shadowmask bake's
  shared-key reads are untouched.

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
layer boundary and the irradiance encoder already emits per-layer block blobs; the
direction encoder (`encode_direction_rgba8`) is a flat per-texel loop in layer-major
order, so per-layer direction slices concatenate byte-identically too. The existing
byte-identity gates (`composite_matches_monolithic_atlas_bit_for_bit`,
`multi_layer_composite_matches_monolithic_bit_for_bit`,
`lightmap_composite_equals_monolithic_on_fixtures`) compare the pre-encode
`CompositedAtlas` and do not observe the encode, so the per-layer direction
encode-and-concatenate needs its own encoded-bytes assertion (see Acceptance
criteria) — the more so since the sibling scaling plan reshapes this same direction
encode. The warm path's composite accumulator carries the same whole-map `f32` cost
and gets the same per-layer treatment. Iterate the outer partition loop in ascending
atlas-layer index (`0..layer_count`) so the appended layer-major blob matches
`encode_section`'s ascending-index concatenation. "Leaf order" sequences only the
faces within a layer, and face bake order is byte-irrelevant (scatter is a disjoint
memcpy); leaf order is not layer-index order, so appending layers as leaves are first
encountered would reorder the blob.

### Task 2: Incremental per-light fold (warm path)

Replace the warm path's "collect all `N_lights` per-light layers, then composite
once" with an incremental fold: bake or cache-load one light's contribution, add it
into the composite accumulator in the existing global light order, then drop that
light's layer before loading the next. This collapses the dominant warm-path term —
`N_lights × covered_texels × per-texel-bytes` — to a single per-light layer resident
at a time, an ~`N_lights`× reduction on that term. Folding in the same order the
monolithic bake sums lights per texel keeps the composite bit-identical, so the
byte-identity gate holds. Bit-identity additionally requires direction to stay a
two-phase accumulate-then-normalize: fold every light's `weighted_dir` (plus
`fallback`/`coverage`) into a running per-texel accumulator distinct from the
emitted `direction`, and run the single `normalize` per covered texel only after the
last light — exactly as `composite_layers` does today. Never normalize per fold:
`normalize(A)` then re-fold `B` ≠ `normalize(A+B)`. The per-texel accumulator
persists until all of that texel's lights are folded; only the per-light
`LayerTexel` input is dropped per light. Nest the warm fold layer-outer, light-inner:
for each atlas layer in ascending index (Task 1's partition), fold each light's
contribution to that layer's texels into the one-layer accumulator in global light
order, then normalize once, dilate, encode, and drop before the next layer. The
per-light blob stays whole-light on disk (all layers); a fold step decodes it, takes
the current layer's texels, and drops it — so a light's blob is decoded once per
layer it touches. This trades extra decode passes (cheap, from the dev-local cache)
for leaving the on-disk format and the shadowmask/section-memo consumers untouched.
Warm-path peak resident is one light's decoded footprint plus one layer's
accumulator — the ~`N_lights`× collapse of the dominant `N_lights × covered_texels ×
per-texel-bytes` term, which is what exhausted RAM. If a future measurement shows a
single light's all-layers footprint is itself the residual driver, per-(light,
layer) on-disk slicing is a separate spec that must bring the shadowmask and
section-memo into scope and bump `LAYER_FORMAT_VERSION` — out of scope here.

### Task 3: Peak-RSS validation, multi-layer fixture, and logging discipline

Provision a multi-layer fixture — a density/area that opens ≥2 atlas layers — for the
byte-identity assertions. Keep it in the cheap gate set only if it stays within the
bin-target budget (`testing_guide.md` §3); otherwise `#[ignore]`-gate it alongside the
`stress-warren` suite. Measure and report compile-time peak RSS on a fine-density
multi-layer bake before and after the change (AC: RSS drop), and confirm a density
that previously exhausted RAM now completes. Logging discipline: a single
`log::info` footprint summary, with any per-partition memory/size breakdown behind
the existing `--verbose` gate that wraps `lightmap_bake::log_stats` — non-verbose
bakes gain nothing new.

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
**Phase 3:** Task 3 — validation, fixture, and logging, layered over Tasks 1-2.

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
- [ ] The per-layer direction encode-and-concatenate equals the whole-buffer direction
  encode at the `.prl`-bytes level (the pre-encode `CompositedAtlas` gates do not
  observe the encode).
- [ ] The ordering/lifecycle pins P1–P9 (see Ordering pins) hold; P1, P2, and P6 have
  dedicated gate tests.

## Ordering pins

Byte-identity rests on accumulation/lifecycle mechanics the tasks assert a result for
but do not fully pin. Each row is a concrete test; the Acceptance criteria reference
these by number rather than restating them.

| # | Scenario | Ordering / schedule | Expected outcome |
|---|----------|---------------------|------------------|
| P1 | N lights, warm incremental fold | Sum all N per-light contributions into `weighted_dir` first; normalize once per covered texel after the last light | Byte-identical `direction` to `bake_face_chart`; per-fold normalize never occurs |
| P2 | Accumulator lifetime, one partition | `weighted_dir`/`fallback`/`coverage` persist across all N folds; only the per-light `LayerTexel` input is dropped per light | Buffers freed only after the partition's normalize + dilate + encode; no drop-before-normalize |
| P3 | Covered-but-dark texel (‖Σ weighted_dir‖² ≤ 1e-8) | Any fold order | `direction = fallback_normal` (surface normal), identical to monolith |
| P4 | Multi-layer atlas, leaf order visits layer 1 before layer 0 | Outer loop ascending layer index; append per layer | Blob ordered layer 0 slice → layer 1 slice, matching `encode_section`'s `0..layer_count` |
| P5 | `layer_count == 1` | Per-layer partition = whole atlas | Exactly one encode pass, no double-encode; bytes identical to pre-change |
| P6 | Shadowmask co-consumer of the shared `"lightmap_layer"` key | Warm fold reads the whole-light blob in memory; on-disk blob and key unchanged | Shadowmask still reads complete `raw_visibility` across all atlas layers from the unchanged blob; no stage rewrites the other's blob; shadowmask output byte-identical |
| P7 | On-disk format unchanged | This plan bumps no cache version; the `"lightmap_layer"` blob is byte-for-byte what it was | `"lightmap_layer"`, `"lightmap_section"`, and `"shadowmask_atlas"` cache entries are all still valid — no regeneration, no staleness |
| P8 | `N_lights == 0` | Warm path enters bake | Short-circuits to the placeholder before the fold, identical to the cold path; accumulator never entered |
| P9 | Chart adjacent in atlas (x,y) to a chart on a different atlas layer | Per-layer dilation | Dilation reads only within-layer neighbors; no cross-layer gutter fill; byte-identical |

## Rough sketch

Bake — `crates/level-compiler/src/lightmap_bake.rs`: `CompositedAtlas` (irradiance
`Vec<f32>`, direction `Vec<Vec3>`, coverage `Vec<bool>`, layer-major) is ~29 B/texel
across all layers today; `bake_monolithic_atlas` allocates it whole, bakes every
face, dilates (already a per-layer loop), then `encode_section` (irradiance already
loops per layer for BC6H). Today `bake_monolithic_atlas` returns the whole atlas and
`encode_section` runs in the caller (`bake_lightmap_controlled`) on that returned
buffer — so the per-layer bake→encode→drop seam must move the encode inside the
per-layer loop (or restructure the return), since the RAM win requires a layer to be
encoded before the next one allocates. The per-layer partition is the seam: bake faces by
`ChartPlacement.layer`, encode a layer's slice, drop. `DEFAULT_TEXEL_DENSITY_METERS =
0.04`.

Warm path — `crates/level-compiler/src/pipeline.rs` (the `lightmap_section`
cache-miss branch): today builds a `Vec<LightmapLayer>` over all lights, then
`composite_layers(&layers, …)`, then `dilate` + `encode_section`. (main.rs is
arg-parsing only; the warm-path driver, the per-light layer cache get/put, and the
composite/dilate/encode_section sequence all live in pipeline.rs.) Each
`LightmapLayer` is dense over covered texels at
`size_of::<LayerTexel>()` (48 B) per texel — the `N_lights`× term. The fold replaces
the collect-then-composite with an accumulate-and-drop loop over lights (and the cold
path at the same site sums lights inline per texel, so it has only the whole-map
`f32` term, addressed by Task 1). The per-light `"lightmap_layer"` blobs are read
whole and folded in memory; their on-disk format and cache key are unchanged, so no
format-version bump is needed and the shadowmask co-consumer of that key is
unaffected.

Cache: the layer cache and the second-level section cache are both unchanged — this
plan alters only the order and lifetime of in-memory allocations.

Logging discipline: mirror the existing bake summary — one-line `log::info`, with any
per-partition breakdown behind the `--verbose` gate that already wraps
`lightmap_bake::log_stats`. Non-verbose bakes gain nothing.
