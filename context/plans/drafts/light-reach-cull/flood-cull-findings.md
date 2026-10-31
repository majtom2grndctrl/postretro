# Lightmap flood cull — findings

> Copied from branch `spike/lightmap-flood-cull-sizing` at 8b2eb15a9 (`context/plans/drafts/lightmap-stage-sizing/flood-cull-findings.md`). The patch, raw report directories and spike-only source files it names stay on that branch; read them with `git show spike/lightmap-flood-cull-sizing:<path>`.

Measurement spike, 2026-10-31, Intel Mac (i9-9980HK, 14 workers). Sizes a portal-flood cull for fully dark (light, texel) pairs in `prl-build`'s Lightmap Bake. No production change landed. Instrumentation and the prototype skip are in `flood-cull-instrumentation.patch` beside this note (diff against 343ff98b6; branch `spike/lightmap-flood-cull-sizing`). Raw counter reports are in `flood-cull-raw/`.

**Soundness warning.** The per-texel flood test, keyed on the texel centre's leaf, is **not** conservative: 21 hallway texels it would skip are lit (18 lit early-outs, 3 partial). Example: light 571 (origin −47.32, 30.28, −111.35, `_light_size` 0.019), chart 1972, texels at (−43.294, 20.32, −106.92) and (−43.294, 20.32, −106.95), centre leaf 1808 outside the flood, visibility 1.0 and 0.26. The footprint straddles a leaf boundary into a flooded leaf. The chart-level test, over every leaf the padded chart slab touches, skipped 0 lit texels on all three maps. Build on the chart-level test, never the centre leaf.

**Recommendation: build flood (B) as a chart-level skip.** On the full hallway it drops 54.5% of (light, chart) pairs and 81% of dark early-outs. A prototype cut Lightmap Bake from 860.2 s to 491.4 s (−43%), `cmp`-identical PRL. Flood (A) skips nothing on any map, as predicted. The small maps gain nothing from either flood.

## Setup

- Binaries: `cargo build --release -p postretro-level-compiler` at 343ff98b6 (clean) and at the spike branch (instrumented). `scripts-build` beside both.
- Bakes: `prl-build <map> -o <out> --release --no-tui --baked-root <scratch>`, cold, one at a time, under `caffeinate -dims`, `pgrep` clean, `CPU_Speed_Limit 100` at each start, 5-minute cool-downs between hallway bakes.
- Counters: `POSTRETRO_LM_SIZING=<report path>`. Per covered (light, texel): outcome, stored visibility class, receiver leaf, and which skip variants would drop it. No timers. Counted runs are separate from timed runs.
- Prototype: `POSTRETRO_LM_FLOOD_PROTO=1`. A (light, chart) pair the chart-level flood test rejects skips its clip pre-pass and shaft test, and every covered texel stores visibility 0 without a ray. The walk, the Lambert term and the record writes stay.
- Bytes: with the counters unset, set, and the prototype on, every PRL is `cmp`-identical to the clean bake (movement-feel, kinematic-platform, hallway).

### Floods

- **(A) Component flood.** Production `affinity_grid::light_portal_reach` over runtime cells and runtime portals, unbounded. Receiver: the runtime cell of the texel's lifted centre.
- **(B) Ball-bounded flood.** Over the full compiler BSP tree (every static brush solid) and its own portals, regenerated for the spike. Seeds: open leaves the emitter sphere (`light_size` + 1 mm) reaches. A portal is crossed only when its polygon lies within R of the light centre, R = `falloff_range` + `light_size` + largest footprint radius (0.025 m) + 0.01 m. Solid and exterior leaves are never entered.
- **Granularity.** Per texel: the lifted centre's leaf is open and outside the flood (centre in solid or exterior never skips). Per (light, chart): every leaf the chart's padded rectangle touches, at lifts 0.25, 1 and 2 mm, padded by the footprint radius ×1.05 + 2 mm. *Strict*: every touched leaf is open and outside the flood. *Relaxed*: every touched open leaf is outside the flood; solid and exterior leaves are ignored.
- **Exclusions.** Suns are never culled, embedded-emitter lights stay exact, and a light whose origin sits in a solid or exterior leaf bypasses. None of the three maps has a sun, an embedded emitter, or a bypassed light, so every static light below is eligible.

## 1. Baseline (clean, cold `--release`)

| Map | Lightmap Bake | of which TexelSteps (CPU) | SH Bake | ShadowmaskAtlas | Total |
|---|---|---|---|---|---|
| hallway | 860.17 s | 98.24 s | 115.93 s | 16.94 s | 1078.56 s |
| movement-feel | 30.75 s | 2.97 s | 3.07 s | 4.64 s | 41.05 s |
| kinematic-platform | 12.85 s | 0.42 s | 5.11 s | 1.37 s | 26.37 s |

The hallway matches the 852.6 s recorded in `measurements/surface-radiance-cache`. Full Build Summaries: Appendix. Contrary to the expectation, movement-feel's lightmap bake is not a few seconds: it has 99.8 M covered texels.

## 2. Context

| | hallway | movement-feel | kinematic-platform |
|---|---|---|---|
| static lights in the walk | 757: 85 point, 672 spot, all soft | 29: 24 point, 5 spot, all soft | 20: 4 point, 16 spot, all soft |
| sun / embedded / bypass | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| (light, chart) pairs walked | 81,788 (70,006 cover a texel) | 1,735 (1,686) | 170 (148) |
| texel visits | 5.68 B | 225.7 M | 76.1 M |
| covered texels | 2.897 B | 99.76 M | 7.36 M |
| full tree: leaves / open / portals | 9,624 / 3,600 / 9,370 | 558 / 139 / 424 | 79 / 23 / 125 |
| connected components, full tree | 2 (one holds 3,599 of 3,600 open leaves) | 1 | 1 |
| connected components, runtime cells | 1 (1,857 interior cells) | 1 (65) | 1 (10) |
| flood A, cells per light | 1,857 every light | 65 every light | 10 every light |
| flood B, open leaves per light | mean 110.8, p50 85, p95 302, max 449 | mean 36.1, p95 58, max 62 | mean 7.0, max 13 |
| world-geometry open edges (watertightness warning) | 557 | 147 | none warned |

Flood construction is cheap: 46 ms install (full-tree portals) and 458 ms of floods summed over 757 lights on the hallway.

## 3. Covered (light, texel) pairs by outcome

| Outcome | hallway | movement-feel | kinematic-platform |
|---|---|---|---|
| shaft-clear tile (no ray) | 697.0 M (24.1%) | 67.56 M (67.7%) | 6.54 M (88.9%) |
| lit early-out | 37.26 M (1.3%) | 6.42 M (6.4%) | 0.464 M (6.3%) |
| dark early-out | 2,141.4 M (73.9%) | 24.52 M (24.6%) | 0.142 M (1.9%) |
| full set, probes disagreed | 1.82 M (0.06%) | 0.896 M (0.9%) | 0.198 M (2.7%) |
| full set, a probe clipped | 19.72 M (0.68%): 16.32 M stored 0 | 0.354 M (0.35%) | 0.013 M (0.17%) |
| sum = covered | yes | yes | yes |

Where the dark early-outs sit, by the texel centre's leaf:

| | hallway | movement-feel |
|---|---|---|
| open leaf, inside flood B | 407.8 M | 4.74 M |
| open leaf, outside flood B | 1,732.5 M | 0 |
| centre in solid (off-face texels buried in a brush) | 1.12 M | 18.89 M |
| centre in exterior | 0 | 0.89 M |

The hallway's darkness is light through walls: 81% of its dark early-outs lie outside their light's ball flood. Movement-feel's is not: 81% of its dark early-outs are chart texels whose centre lies in solid or exterior space (77% in solid), off the face polygon, where no flood reaches. Kinematic-platform is one room with almost no dark texels.

## 4. Dark pairs the cull would skip

Flood (A) skips nothing on any map, at either granularity: each map is one runtime component, so every light floods every cell.

Flood (B):

| Variant | hallway | movement-feel | kinematic-platform |
|---|---|---|---|
| per texel (centre leaf), dark early-outs skipped | 1,732.5 M (80.9% of dark) | 0 | 0 |
| per texel, clipped full sets stored 0 skipped | 12.48 M | 0 | 0 |
| per (light, chart), relaxed: pairs skipped | 44,575 of 81,788 (54.5%) | 1 of 1,735 | 0 |
| relaxed: texel visits in skipped pairs | 2.917 B (51.4%) | 0.53 M | 0 |
| relaxed: dark early-outs skipped | 1,733.2 M (80.9%) | 6,823 | 0 |
| relaxed: clipped full sets stored 0 skipped | 12.51 M | 0 | 0 |
| per (light, chart), strict: pairs / dark skipped | 3,775 / 72.4 M (3.4%) | 0 / 0 | 0 / 0 |

The relaxed chart test loses nothing against the per-texel ceiling on the hallway: charts sit wholly on one side of a wall. The strict test fails almost everywhere because a chart's padded rectangle overhangs into the brush it lies on.

## 5. Soundness (skipped texels whose stored visibility > 0)

| Variant | hallway | movement-feel | kinematic-platform |
|---|---|---|---|
| A, per texel and per chart | 0 | 0 | 0 |
| B per texel (centre leaf) | **21** (18 lit early-out, 3 partial) | 0 | 0 |
| B per chart, relaxed | 0 | 0 | 0 |
| B per chart, strict | 0 | 0 | 0 |

Skipped clipped full sets all stored exactly 0, so they keep bytes; they are full sets only because a probe was clipped.

## 6. Projected and measured savings (hallway)

**Projection** (per-texel costs from the 2026-10-25 Windows study, not re-measured here): 1,733 M dark early-outs × 1.56 µs ≈ 2,700 CPU-s, plus 12.5 M clipped full sets × ~17 µs ≈ 210 CPU-s. Emitter pairing is inside the 1.56 µs. Per skipped pair the clip pre-pass and shaft-test setup also go; the earlier study puts the clip pre-pass at 0.1% of the stage.

**Measured** (prototype, one run, `cmp`-identical): Lightmap Bake 860.17 → 491.39 s (−368.8 s, −42.9%); Total 1078.56 → 704.21 s. SH Bake (control) 115.93 → 115.96 s. The prototype still walks every texel of a skipped pair, evaluates its Lambert term and writes a 0 record: 2.92 B visits, 1.75 B of them covered. Dropping a skipped pair whole would save more, but needs a check that no record and a 0 record compose to the same bytes downstream (shadowmask coverage, owner estimate).

movement-feel and kinematic-platform: nothing to save from either flood. A valid null result.

## Doubts about (B)'s exactness

The argument: a segment from any emitter point to any covered footprint point stays inside the ball, and leaves an open leaf only through a portal or through solid. A receiver leaf outside the ball flood is therefore reached only through solid, so every ray is blocked. Where it can break:

- **Cracks.** The argument needs every open/solid boundary to carry a triangle the hit test cannot miss. The hallway reports 557 open edges and movement-feel 147. A side variant measured directly how far that holds: texels whose whole footprint slab touches no open leaf (deep in solid) should all be dark. On movement-feel, 188 of 19.6 M such texels were not: 160 partial (every sampled example at 1/32) and 28 lit early-outs, with emitter spheres well clear of geometry, 9–18 m from their lights. Rays leak through geometry the BSP calls solid. The hallway showed none, but the same leak through a wall would make a flood skip change bytes. The brief needs an exactness standard like the shaft cull's, plus a verify mode that re-traces skipped pairs.
- **Relaxed rule and buried texels.** The relaxed test ignores the solid leaves a chart overhangs. Its buried texels are dark only if traces from inside solid always hit a face on the way out, which is the same crack assumption. The strict rule avoids it but keeps only 4% of the win.
- **Centre leaf.** The per-texel centre leaf is unsound in practice (the 21 texels above). A per-texel production test must use every leaf the texel's footprint slab touches.
- **Edge and vertex crossings.** A segment can pass from one open leaf to a diagonal neighbour through a shared edge with no portal between them. When the leaves beside that edge are open, the flood reaches the neighbour through them, since the edge lies on their portals' closed polygons. When they are solid, the segment grazes an edge between two solids, which the hit test resolves by rounding. Not observed.
- **Trace-start slab.** Leaves come from three lifted rectangles (0.25, 1, 2 mm). A leaf thinner than the gap between lifts could be missed. Clipping uses a 1 µm slack.
- **Exterior.** The flood never enters exterior leaves, as flood (A) does not. That relies on a ray into exterior space first crossing an interior-facing hull face.
- **Topology.** (B) runs on the full tree. Runtime cells would need the ball test against runtime portal polygons; not measured. A sanity counter confirmed no covered texel lies outside the ball (falloff has a hard cutoff in all three models).

## Appendix: Build Summaries (clean)

hallway (`content/dev/maps/stress-warren-hallway-inspection.map`): Parsing 0.56, Partitioning 0.57, DataScript 0.69, Visibility 0.10, Geometry 0.01, BVH Build 0.01, Cell Visibility 0.12, NavMesh 2.39, TexValidation 0.01, Materials 3.07, SH Bake 115.93, Delta SH Bake 16.46, Direct SH Bake 6.91, Animated Direct SH Bake 0.60, EntityShadowLights 0.02, Direct SH Delta Bake 2.88, Billboard Direct Scatter Bake 4.89, ChunkLightList 1.29, Atlas Preparation 0.29, Cell Residency Set 0.35, Lightmap Bake 860.17 (TexelSteps 98.24), ShadowmaskAtlas 16.94, AnimLightChunks 1.37, AnimWeightMaps 8.52 (TexelSteps 2.68), TextureMips 0.00, Reflection Probes 0.00, ClusterDirectory 0.41, Packing 13.64, Total 1078.56 s.

movement-feel: Parsing 0.01, Partitioning 0.03, DataScript 0.00, Visibility 0.01, Geometry 0.00, BVH Build 0.00, Cell Visibility 0.00, NavMesh 0.37, TexValidation 0.01, Materials 0.02, SH Bake 3.07, Delta SH Bake 0.00, Direct SH Bake 0.18, Animated Direct SH Bake 0.00, EntityShadowLights 0.00, Direct SH Delta Bake 0.04, Billboard Direct Scatter Bake 0.15, ChunkLightList 0.01, Atlas Preparation 0.00, Cell Residency Set 0.03, Lightmap Bake 30.75 (TexelSteps 2.97), ShadowmaskAtlas 4.64, AnimLightChunks 0.00, AnimWeightMaps 0.00 (TexelSteps 0.00), TextureMips 0.00, Reflection Probes 0.00, ClusterDirectory 0.01, Packing 0.83, Total 41.05 s.

kinematic-platform: Parsing 0.01, Partitioning 0.00, DataScript 0.00, Visibility 0.00, Geometry 0.00, BVH Build 0.00, Cell Visibility 0.00, NavMesh 0.06, TexValidation 0.01, Materials 0.59, SH Bake 5.11, Delta SH Bake 0.40, Direct SH Bake 0.29, Animated Direct SH Bake 0.03, EntityShadowLights 0.00, Direct SH Delta Bake 0.04, Billboard Direct Scatter Bake 0.22, ChunkLightList 0.00, Atlas Preparation 0.00, Cell Residency Set 0.00, Lightmap Bake 12.85 (TexelSteps 0.42), ShadowmaskAtlas 1.37, AnimLightChunks 0.00, AnimWeightMaps 1.79 (TexelSteps 0.14), TextureMips 0.01, Reflection Probes 0.00, ClusterDirectory 0.02, Packing 1.32, Total 26.37 s.
