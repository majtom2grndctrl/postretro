# Research — sparse layer cache and fused walk

Read at `d6e1c8b`, tip of `gameplay-stack-ai-physics-crates`. Lightmap- and cache-side facts
hold on `origin/main` `438a849` too; shadowmask-side facts are branch-only and are re-verified
at merge. The branch's diff against main under `crates/level-compiler/src` touches only
`cache.rs`, `lightmap_bake.rs`, `lightmap_layer.rs`, `shadowmask_bake.rs`, and the new
`shadowmask_bake/assignment.rs`; `pipeline.rs` is identical on both, so the stage order below is
stable across the merge.

## Measured thrash

`content/dev/maps/campaign-test.map`: 32 lights, 14 static baked, 9 layer-bearing; 2048×2048×4
atlas at the default 0.04 m/texel; scratch `--cache-dir`; default 2 GiB budget; 4 cores. The
one-time 140 s `scripts-build` is excluded.

| Run | Lightmap stage | SH stage | Total | Cache |
|---|---|---|---|---|
| Empty cache | 85.8 s | 30.6 s | 162 s | wrote 7,415 entries, 5.8 GB; 36 `lightmap_layer` entries at 122–166 MB each, 5.46 GB together |
| No-edit rerun | 54.6 s | 31.1 s | 102 s | sweep evicted 7,399 of 7,415 (3.44 GiB); `lightmap_section` miss; 23 of 36 layers re-baked; all 3,306 `sh_group` miss; cache back to 5.5 GB |
| One light-intensity edit | 59.1 s | 19.4 s | 86 s | evicted 3.42 GiB; 26 of 36 layer misses; cache back to 5.7 GB |

Cold `ShadowmaskAtlas` on the same map is 8.8 s against the 85.8 s lightmap stage, with four of
the nine layer-bearing lights selected. That is the fusion ceiling for this map.

## The causal chain

`StageCache::get` calls `set_modified(now)` on every hit; `prune_to_budget` runs once from
`main.rs` before the build, sorts by mtime, and evicts oldest-first until the directory fits
`args.cache_max_bytes` (`DEFAULT_MAX_BYTES` = 2 GiB). Stage order in `pipeline.rs`:

```
… BvhBuild → CellVisibility → NavMesh → LightmapBake → ShBake → DeltaShBake → DirectShBake
→ AnimatedDirectShBake → EntityShadowLights → DirectShDeltaBake → BillboardDirectScatterBake
→ ShadowmaskAtlas → ChunkLightList → AnimatedLightChunks → AnimatedWeightMaps → …
```

`LightmapBake` writes `lightmap_layer` and `lightmap_section`; `ShBake` writes every `sh_group`;
`ShadowmaskAtlas` then re-reads the selected lights' `lightmap_layer` entries, so at the next
sweep those are the newest files in the directory. Two GiB of 150 MB entries is roughly the
selected set; everything smaller and older — the section memo, every SH group, the delta
sub-blocks — goes. The prune line and the hit/miss lines are `log::info` (the warm-path
hit/miss pair is additionally gated on `args.verbose`), and `logger::install` defaults the
filter to `warn`, so a collaborator sees none of it.

## What a layer entry holds

`LayerTexel` is 48 bytes, pinned by a `const` assert because `to_bytes`/`from_bytes` cast the
block with `bytemuck`:

| Field | Bytes | Nature |
|---|---|---|
| `idx`, `layer` | 8 | coverage; `layer` is redundant with the key's `target_layer` |
| `fallback_normal` | 12 | `chart.normal`, light-independent, identical across layers |
| `irradiance` | 12 | `contribution * v` |
| `weighted_dir` | 12 | `to_light * (sum(contribution) * v)` |
| `raw_visibility` | 4 | the only value that needed a ray; `-1.0` sentinel when unreached |

`LightmapLayer` is dense over every covered texel ("deliberately not value-sparse", per its
doc) and `bake_light_layer_chart_controlled` walks every texel of every chart for every light.
The branch's analytic graph pass measured its light-AABB × chart-AABB prune on another fixture
at 8.18 % of light-chart pairs kept; the reach spike measured 4–6 % of the static light set
reaching a receiver on its fixture. Neither is a light/*texel* fraction, which is what the
payload ratio actually turns on, and `shadowmask-cold-working-set`'s own research says not to
size from published reach fractions. They establish that the win is large, not how large: the
acceptance row asserts a tenth of dense, well short of either figure. The campaign-test
fraction is measured in stride 1, not inherited.

No compression exists anywhere in the crate (`Cargo.toml` has no flate2/zstd/lz4/snap);
`put`/`write_entry` write bytes verbatim under a 44-byte frame (`PRC2` magic, `u64` length,
blake3). The branch adds `put_streamed` for peak RAM, not disk.

## Reconstruction is exact

`light_texel_contribution_and_visibility` is the whole per-light per-texel bake:

1. `(contribution, to_light) = light_contribution_and_direction(light, world_p, normal)` — the
   unshadowed Lambert term, before any ray.
2. `contribution.length_squared() <= 1.0e-12` → `(ZERO, ZERO, None)`. The branch names this
   predicate `contribution_covers_shadowmask` and wraps it as `light_texel_is_covered`; its test
   `analytic_coverage_matches_baked_coverage_on_multilayer_golden` pins
   `analytic == (raw_visibility >= 0)` light by light.
3. `v = soft_visibility(...)`; `v <= 0.0` → `(ZERO, ZERO, Some(0.0))`. Not `contribution * 0`:
   `to_light * 0.0` is `-0.0` on negative components, and the early return is what the dense
   record stored.
4. Otherwise `irr = contribution * v`, `weighted_dir = to_light * ((c.x + c.y + c.z) * v)`,
   `Some(v)`. NaN `v` passes `v <= 0.0` as false and propagates.

A sparse record stores `v` for case 3 and 4 texels. The fold recomputes step 1 with the walk's
own `world_p` and `chart.normal` (the walk is `for_each_light_layer_chart_texel`, a pure
function of the atlas and chart index) and applies steps 3–4 verbatim.

**Signed zero.** Both `IncrementalLayerAccumulator::zeroed` and `bake_face_chart` start every
accumulator at `Vec3::ZERO` (+0.0). Under round-to-nearest, `(+0.0) + (-0.0) = +0.0` and
`x + (+0.0) = x` bitwise for every `x` the accumulator can hold, so an accumulator that starts
at +0.0 never holds −0.0 and skipping an unreached light is bit-identical to the dense layer's
explicit `+0.0`. The brief keeps a row with a −0.0 term because the argument is cheap to test
and expensive to be wrong about. NaN: `NaN + 0.0` returns the NaN operand on every target the
compiler builds for, so payload bits survive the dense fold's extra adds; the NaN row asserts
bit equality rather than `==`.

**Fold order.** Dense adds lights in global `static_lights` order per texel, layer-outer,
light-inner (`pipeline.rs` warm block; `fold_partition` per light). Sparse keeps that order;
only the terms that are exactly zero disappear.

## Why coverage is not stored

The handoff proposed storing coverage and geometry once per atlas layout. The compositor
already holds `SharedAtlas` (charts, placements, dims) when it folds, and
`validate_layer_partition` today re-walks the chart/row/column order to check membership
without storing anything. A per-layout side table of `(idx, world_p, normal)` would be ~28
bytes per covered texel — the same order as today's dense entry — and would need its own
validation against the atlas. Re-walking costs one analytic Lambert per covered texel per
light per layer, which the cold bake already pays before its first ray.

## The SH block reads positions only

Audited outside `#[cfg(test)]`: `sh_group.rs`, `direct_sh_bake.rs`, `delta_sh_bake.rs`,
`animated_direct_sh_bake.rs`, `billboard_direct_scatter_bake.rs`, `sh_bake.rs`,
`sh_density.rs`, `sh_coarsen.rs`, `affinity_grid.rs`, `sh_runtime_envelope*.rs`,
`entity_shadow_select.rs`, `delta_sections.rs`, `sh_analyze.rs`, and `chunk_light_list_bake.rs`.
Every geometry read is `geometry.geometry.vertices[...].position` through `indices`, an AABB
min/max fold over `vertices`, or `vertices.is_empty()`. None reads `lightmap_uv`, `faces`,
`face_index_ranges`, `texture_names`, or `vertices.len()` as a value. `split_shared_vertices`
pushes cloned vertices and rewrites index values; `index_offset`, `index_count`, and
`indices.len()` are untouched, so triangle enumeration, the world AABB, the probe grid, and
affinity cells are bit-identical. `build_bvh` already runs before the split and its primitives
are reused after it — the invariance is load-bearing today.

The SH block (`ShBake` through `BillboardDirectScatterBake`, lines 1192–1940 of `pipeline.rs`)
reads no lightmap-stage output: no `lightmap_bake_output`, atlas dims, charts, placements, or
`final_lightmap_density`. Its context is `&geo_result`, tree, exterior leaves, BVH, primitives,
and the light sets.

`sh_group::geometry_content_hash` postcard-serializes the whole `GeometryResult` — including
`lightmap_uv` on every `Vertex` — into the keys of `sh_group`, `direct_sh_bake`,
`delta_sh_bake`, `animated_direct_sh_bake`, `billboard_direct_scatter_bake`, and
`chunk_light_list_bake`. The comment at the weight-map key site saying "the lightmap/sh stages
hash a pre-bake geometry clone" is stale: `sh_ctx.geometry` is `&geo_result` after
`prepare_atlas(&mut geo_result, …)`. `cache_cross_bake_tests.rs` asserts hit/miss behaviour
and epoch bumps, not any hash value.

## Why the selection forces the reorder

`ShadowmaskAtlas` consumes `delta_sections.entity_shadow_lights`. `DirectShDeltaBake` resolves
that from the raw selection and clears it to `None` when the direct delta section is absent or
unusable (`(None, None, None)` arms in `pipeline.rs`). Channel assignment is per light and must
precede the fill so the fill can write the quantized byte into its channel during the walk —
the shadowmask brief's residency invariants ("no structure indexed by light-and-texel is live";
"one full-size copy of the section") leave nowhere to park a per-light raw record between walk
and fill. At the stress config that record would be 338 lights × ~2.6e7 reached texels × 4 B,
tens of GB, sparse or not. So: selection final → graph and coloring → fused walk.

## Duplication inventory

| Site | Walk | Rays | Shares with the fused walk |
|---|---|---|---|
| `bake_face_chart` — two callers: `bake_atlas_layer_controlled` (shipping cold path) and `bake_monolithic_atlas_controlled` (byte-identity reference) | own raster loop, identical to the walk's | all static lights, discards `v` via `light_texel_contribution` | walk and rays — but only the shipping caller may move to the walk; moving both collapses the gate into a tautology |
| `bake_light_layer_chart_controlled` (warm layer writer) | the walk | one light | walk and rays |
| shadowmask fill (branch) | reads layer entries or bakes via the same chart function | selected lights | walk and rays |
| `animated_light_weight_maps.rs` | own loop over the same charts | animated lights, SplitMix64 texel seed vs the lightmap's FNV-1a | walk only |
| `bake_probe_direct_rgb` via `bake_probe_tile` and `bake_direct_delta_subblock` | probe grid | same probe, same `static_index` seeds, once summed and once per light | nothing — other axis |

## Prior commitments touched

- `build-stage-cache` (done): no eviction policy then; LRU by mtime landed later and is the
  mechanism inverted here. Corruption is a per-entry soft miss — preserved.
- `perf-warm-lightmap-section-cache` (done): chose the section memo over value-sparse layers
  ("high risk to the byte-identity gate — rewrites the proven compositor coverage/fallback
  logic"). This brief takes the deferred option now that the walk primitive and the
  equivalence test exist.
- `static-light-shadowmask-cache-addendum` (done): made `lightmap_layer` the shadowmask's
  raw-mask source and rejected "a duplicate per-light raw-visibility cache". Sparse entries
  keep that single source.
- `lighting-scale--lightmap-bake-incremental-flush` (in progress): per-partition keys with
  `target_layer`, layer-outer/light-inner fold, `LAYER_FORMAT_VERSION` bump to 5. Its pending
  owner-machine row runs `--release`, which bypasses the cache, so no format change here can
  affect it; its four automated rows are bookkeeping — the named tests exist.
- `lighting-scale--shadowmask-cold-working-set` (ready; implemented on the branch): analytic
  graph, coverage-is-unshadowed contract, fill-in-one-bake layer-outer, one-copy residency,
  "cache keys and memo semantics unchanged" for *that* work. This brief bumps the layer epoch
  deliberately and keeps the memo key shape.
- `compiler-log-hygiene` (draft): downgrades `[cache]` hit/miss to debug. Level is the gate;
  the new warning is a warn on purpose.
- `compiler-implausible-allocation-guard` (draft): owns attribution of the 42 TB request.

## Not verified

- `animated_lm_weight_maps` entry size relative to the sparse layers; it is the likely
  runner-up after slimming, at chunk-rect rather than atlas grain.
- The reach fraction on campaign-test.
