# Lightmap stage sizing — findings

Measurement spike, 2026-10-25. Sizes the exact speed-ups left in `prl-build`'s Lightmap Bake after the penumbra floor (`context/plans/done/lightmap-penumbra-floor/`). No production change landed. The temporary instrumentation and both prototypes are in `instrumentation.patch` beside this note (applies to 00c9d8362).

**Recommendation: build both exact skips: tile shaft culling and a chart-level coverage cull.** Prototyped together, they cut hallway-mini's Lightmap Bake from 374.7 s to 210.4 s (−44%, 1.78× faster), byte-identical. That reclaims ~84% of the floor's +197 s over the pre-floor 178.1 s. The shaft cull is the larger lever (−129 s). The chart cull is small and simple (−32 s). The next lever is dark texels; size it before drafting.

## Setup

- Map `content/dev/maps/stress-warren-hallway-inspection-mini.map`: 381 static lights, every one soft (`_light_size 0.75`), no suns, no flagged embedded emitters. Hard lights were sized on `shadowmask-groups-capture`, which is hard-light only.
- Build: dev profile (the standard configuration and the one the floor plan's timing history uses), `CARGO_INCREMENTAL=0`, 5 workers. Bake: `prl-build <map> -o <out> --release` (cold: `--release` implies `--no-cache`), one job at a time on the Windows 1660 Super machine.
- Bytes: every counted and prototype PRL is `cmp`-identical to the clean run's.
- Counters: `POSTRETRO_LM_SIZING=<report path>` turns on per-(light, chart) counters in `bake_light_layer_chart_clipped`. They record the outcome per covered texel, call counts and times for every ray kind, and a real shaft test over 4-, 8- and 16-texel tiles. One timer pair costs 78 ns here, about one shadow ray. The counted run took 569.8 s against 374.9 s clean, so the time split below is corrected for timer cost and is approximate. The prototype wall times are the authoritative numbers.

## 1. How (light, texel) pairs end

Covered texels only (the light has a direct term there). Floored path, default 32 samples.

| | hard (groups-capture) | soft (hallway-mini) |
|---|---|---|
| covered texels | 3.65 M | 426.5 M |
| lit early-out (9 probes clear) | 93.05% | 33.68% |
| dark early-out (9 probes blocked) | 5.77% | 65.26% |
| full set, probes disagreed | 0.11% | 0.07% |
| full set, a probe clipped | 1.07% | 1.00% |
| full set, embedded emitter | 0 | 0 |

- Rays per outcome: 9.0 per early-out, 32.0 per disagreeing full set, 28.4 per clipped one (dropped samples trace nothing).
- 65.8% of hallway-mini's shadow rays are answered by the remembered occluder, almost all of them on dark texels.

**Shaft test.** A tile is clear when no triangle can cross the convex hull of three parts: the tile's texel-centre rectangle, padded by the footprint radius plus 4 mm, in the slab 0.5–2.5 mm above the receiver; the emitter sphere, at its exact support; and, for a sun, the tile swept along the cone out to the world diagonal. The verdict comes from a separating-axis test over the receiver normal, the triangle normal, and the triangle's edges crossed with the chart axes, the normal and the shaft direction. A `clear` verdict is sound, though it may miss tiles that are in fact clear. In every counted run, every clear tile held only lit early-outs (0 unsound).

| tile | soft: covered texels in clear tiles | soft: share of lit | hard: covered in clear tiles | hard: share of lit |
|---|---|---|---|---|
| 4×4 | 32.7% | 97.0% | 89.9% | 96.7% |
| 8×8 | 31.8% | 94.3% | 86.6% | 93.1% |
| 16×16 | 30.2% | 89.6% | 80.7% | 86.7% |

Clear shafts catch nearly every lit early-out at 8×8. The test is cheap: 2.3 µs per covered tile at 8×8, against ~4.4 µs for one lit texel's footprint.

## 2. Does the lightmap still iterate every static light per texel?

No. Out-of-scope item 3 (`lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md`) describes `bake_face_chart`. That is now `lightmap_bake::reference::bake_face_chart`, the monolithic oracle that only tests call. Production runs the per-light layer walk:

- `LayerPartitions::plan_charts` (`pipeline/lightmap_stage/partitions.rs`) keeps a chart for a light only when `light_may_reach(light, chart_texel_bounds(chart))` holds. That is a falloff-range test against the chart's texel bounds, padded 0.5 m.
- Per texel, `light_texel_contribution_and_visibility_with` returns before any ray when the Lambert term is zero (range, back face, spot cone).
- `layer_influence_aabb` culls nothing. It bounds the geometry the layer cache key hashes.

The chart test is loose, though. 84.3% of the (light, chart) pairs it keeps (87,321 of 103,547) cover no texel at all, and 88.9% of the 3.84 B texel visits are uncovered. Most of that waste is back faces and spot cones, which a range test cannot see. See lever 2 below.

## 3. Where Lightmap Bake time goes now

Clean hallway-mini: 374.9 s and 374.5 s on two runs, about 1,376 CPU-s at the 3.67 workers the counted run kept busy. Shares are corrected for timer cost (78 ns per pair, ~72 ns inside each timed region) and are good to a few points.

By outcome:

| | CPU-s | share | per texel |
|---|---|---|---|
| lit early-out texels | ~630 | ~46% | 4.39 µs |
| dark early-out texels | ~434 | ~32% | 1.56 µs |
| full-set texels | ~75 | ~5% | ~17 µs |
| uncovered texel visits (Lambert + walk setup) | ~140 | ~10% | ~41 ns (pinned by the chart-cull run) |
| texel-step detector (`TexelSteps`) | 23 | ~2% | — |
| rest (partition fold, chart setup) | ~70 | ~5% | — |

By kind of work:

| | CPU-s | share |
|---|---|---|
| shadow rays (`segment_clear_remembering`) | ~380 | ~28% |
| emitter pairing (`emitter_pairing` + counter ring) | ~200 | ~14% |
| clip segments + lead-ins | ~96 | ~7% |
| clip pre-pass (`ClipReach::rasterized`) | 1.8 | 0.1% |
| uncovered visits | ~140 | ~10% |
| other per-texel sampling math (targets, offsets, rotations), bookkeeping | remainder, ~40% | |

Lit texels are the expensive class. Their nine rays all run unblocked to the emitter, and none can be served by the remembered occluder. The clip pre-pass is negligible, so floor lever A's clip sharing across lights has nothing left to save. Pairing costs ~0.46 µs per covered texel, ~30% of a dark texel's cost.

## 4. Levers and measured savings

Each prototype is exact: same PRL bytes as the clean bake.

| Run | Lightmap Bake | Total | SH Bake (control) |
|---|---|---|---|
| A clean | 374.93 s | 678.99 s | 223.38 s |
| A2 clean | 374.50 s | 640.36 s | 221.96 s |
| D chart cull | 341.78 s | 631.39 s | 246.98 s |
| D2 chart cull | 343.78 s | 614.00 s | 224.45 s |
| E shaft cull, 8×8 | 245.47 s | 511.14 s | 219.38 s |
| F shaft 8×8 + chart cull | 210.41 s | 473.41 s | 217.15 s |

groups-capture (hard), three runs each: off 2.24 / 2.37 / 2.30 s; shaft 4×4 1.25 / 1.29 / 1.26; 8×8 1.13 / 1.30 / 1.24; 16×16 1.35 / 1.35 / 1.36. That is −46% at 4×4 and 8×8 (median 2.30 → ~1.25 s, against the pre-floor 0.96 s). Chart cull: one run, 2.21 s (17 empty pairs).

**Lever 1: tile shaft culling. Build.** The roadmap's definition holds: skip every ray of a tile whose padded shaft to the emitter no triangle crosses, so visibility is exactly 1. Measured −129 s (−34%) on hallway-mini and −46% on groups-capture. 8×8 is the pick: 4×4 catches 3% more lit texels for 4× the tests, and 16×16 loses 5%. The prototype tests a tile lazily, on its first covered texel, and keeps the clip pre-pass. Notes for the brief:
- A skipped texel leaves the remembered occluder untouched. That changes only which triangle a later ray tests first, and bytes stayed identical. The occluder cache can still catch a box-rounding miss (`segment_clear_remembering`'s doc), so the brief needs byte-parity tests with and without the cull.
- A flagged embedded emitter stays exact: a clear shaft means its full set is all clear too. No such light was measured.
- Suns are implemented but unmeasured (neither map has one). The floor-off path and the animated weight-map bake (0 s on hallway-mini) could reuse the test; unmeasured.
- Output bytes do not move, so no cache key needs a version bump.

**Lever 2: chart-level coverage cull. Build; can fold into the same brief.** In `plan_charts`, drop a (light, chart) pair when no texel can have a direct term:
- a point or spot light more than 1 cm behind the chart's plane;
- a sun with N·L ≤ 0, which is the same at every texel, so exact;
- the light beyond `falloff_range` of the texel-centre rectangle (exact nearest distance, not the 0.5 m-padded box);
- the chart's bounding sphere outside a spot's outer cone, with a 1 mrad margin.

It culled 82,950 pairs on hallway-mini (behind 45,423, cone 35,385, range 2,142): 95% of the empty pairs and 2.87 B texel visits. Measured −32 s (−8.5%) on two interleaved pairs, and −35 s on top of the shaft cull. Cheap: no rays, one test per pair. Byte-identical with no key change. The texel-step detector and the cache key's footprint pad still use `light_may_reach`, and they can stay that way.

**Next: dark texels, size before drafting.** After levers 1 and 2, dark early-outs are about 434 CPU-s of a remaining ~770, so roughly 118 s of the 210 s. Two exact candidates:
- (a) Cell-level reach: skip a (light, chart) pair whose receiver cells the light's cells cannot see through the baked cell-visibility relation. Exact only if the relation is a conservative superset of segment visibility, with the emitter sphere's and footprint's cells unioned in. Hallway lights reaching through walls into neighbouring corridors are the expected source, but this is unmeasured.
- (b) Cheaper emitter pairing (bytes unchanged), ~0.46 µs of every covered texel. An optimisation rather than a skip.

**Not worth building:** sharing the clip pre-pass across lights (0.1%). Lever B stays declined, as the owner ruled.

## Roadmap

`context/plans/roadmap.md` carries both levers as one entry, "lightmap exact skips", and the dark-texel follow-up as "lightmap dark-texel cost". Out-of-scope item 3 of `lighting-scale--cold-bake-reaching-light-spike/` is marked superseded.
