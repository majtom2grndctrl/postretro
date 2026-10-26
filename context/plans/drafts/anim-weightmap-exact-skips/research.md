# anim-weightmap-exact-skips — research

Read at 020cf04b5 (`origin/main`). The precedent's brief, research, plan and code were read on `claude/lightmap-exact-skips` at 6e0c28578, which is not yet on main. "Precedent" below means that brief; its sections are cited as precedent research §….

## Premises

| Claim | Ground |
|---|---|
| The weight walk traces with the static walk's tracer. `segment_clear` is `segment_clear_remembering` with a fresh, never-read cache cell: same origin lift, same `max_distance`, same hit predicate. | `lightmap_bake.rs::segment_clear` |
| It samples with the static walk's sampler, on the same site: `chart_texel_world_position`, `chart.normal`, `chart_texel_seed`, `TexelFrame::for_chart`, and `may_clip` from `ClipReach::rasterized` on the same chart. Shadow and clip segments both go to `segment_clear`; lead-ins to `lead_in_clear` with the chart's `ClipBand`. | `animated_light_weight_maps/face_bake.rs::FaceSetup::bake_light` |
| So precedent research §Hull containment and §Margin derivation hold as written: both depend only on `TexelSampler`'s segments, the tracer's endpoint arithmetic and `ray_triangle_hit`. | the two rows above |
| No occluder cache. Each texel's visibility is a pure function of light, site and scene. | `FaceSetup::bake_light` (`clear` closure) |
| The direct term and weight gate run before any ray. `light_contribution_and_direction`, then `contribution_to_weight`, and the texel is skipped at `weight <= WEIGHT_EPSILON`. | `FaceSetup::bake_light` |
| A clear tile's stored value is unchanged. The entry stores `weight * visibility`; at visibility exactly 1.0 that is `weight`'s bits. The detector grid stores the same 1.0. | `FaceSetup::bake_light` |
| A (face, light) whose every texel fails the weight gate stores no entry and reports no step. Its grid stays NaN, and `scan_chart` returns `None` when nothing steps. | `FaceSetup::bake_light`; `texel_step_detector/report.rs::scan_chart` |
| Walk order: one governed task per (face, light). It walks every chunk of the face that lists the light, in face order, each chunk's span row-major in chart interior coordinates. | `bake_weight_maps_on`; `FaceSetup::bake_light` |
| Chunks of a face are disjoint whole-texel rects. Siblings can list different lights. | `animated_light_chunks.rs::recurse`; `static_atlas_frame.rs::assert_no_overlapping_rects_per_layer` |
| The clip pre-pass is per face, shared by every light on it. The static walk builds it per (light, chart). | `FaceSetup::new`; precedent `chart_walk.rs::bake_light_layer_chart_clipped` |
| The stage bakes no sun. The chunk builder drops lights with an unbounded influence radius. The Quake adapter normalizes an animated directional light to static. | `build_animated_light_chunks` (`radius != f32::MAX`), test `directional_animated_light_skipped`; `format/quake_map.rs` (`[SH-DENSITY-DIRECTIONAL-STATIC]`) |
| `sdf` lights are never baked. They are filtered out of a face's light list. | `FaceSetup::new` |
| Baked animated lights never move. Lights in the stage are `!is_dynamic && animation.is_some()`. `LightAnimation` holds brightness, colour, spot aim, period, phase and start state, with no origin or size. Moving lights are dynamic-tier. | `WeightMapInputs::lights` doc; `map_data.rs::LightAnimation`; `build_pipeline.md` §Buried lights ("Dynamic lights can move") |
| Spot cones bake at the rest aim. The bake reads `cone_direction`, never `animation.direction`, so a cone test that reads the same light is exact against the bake. | `light_contribution_and_direction`; no `animation` read in `face_bake.rs`, `texel_footprint/` or `lightmap_bake.rs` |
| Embedded emitters: classified once per stage with the floor on, and passed as `ProbeEarlyOut::for_light`. | `animated_light_weight_maps.rs::embedded_emitters`; `bake_animated_light_weight_maps_with` |
| Cache: one whole-stage `"animated_lm_weight_maps"` entry. The key folds `STAGE_VERSION` (13), the whole `MapLight` list, the prepared geometry, density, atlas dimensions, layer count, chunk-section bytes, sample count, layout fingerprint, floor switch, step threshold and cut map. The entry is the step report followed by the section. | `pipeline/animated_atlas_stage.rs::weight_map_cache_key`, `bake_or_load_weight_maps` |
| Output: id-25 weight maps (`chunk_rects`, `offset_counts`, `texel_lights`: light index, weight, octahedral direction), and the step report. `cull_unlit_chunks` then drops chunks with no entry, on both cache paths. | `bake_weight_maps_on`; `cull_unlit_chunks`; `pipeline.rs` |
| The oracle is cull-free. `texel_major_reference` runs its own texel-major walk on `segment_clear`, `TexelSampler` and `ClipReach`, or `soft_visibility` with the floor off. | `animated_light_weight_maps/tests.rs::texel_major_reference` |

## Precedent API fit

| Precedent symbol (6e0c28578) | Weight-walk input | Fit |
|---|---|---|
| `ShaftCull::new(light, chart, sampler, frame, scene, bvh, primitives, geometry)` | `FaceSetup::{chart, frame}`, the task's `TexelSampler`, `WeightMapInputs::{bvh, primitives, geometry}` | One-for-one. It returns `None` for a sun, which this stage never sees. |
| `ShaftCull::tile_clear(tile_x, tile_y)`, tiles of `SHAFT_TILE_TEXELS` from the (sub-)chart's first interior texel | `tx`, `ty` in chart interior coordinates | One-for-one |
| `ShaftScene::new(primitives, geometry)` | Same inputs | Build once per stage, from that stage's own inputs. The lightmap stage takes `&mut geo_result`, so do not assume one scene serves both stages. |
| `chart_may_cover(light, chart)` | `FaceSetup::chart` | Whole-chart test. It is exact for any sub-region, since every chunk span lies inside the chart interior. |
| `ExactSkips { chart_cull, shaft_cull, verify, force_clear, stats }` | No seam yet | Reusable. Its verify helper compares cache copies; the weight walk has none, so its verify checks 1.0 only. |
| Re-exports | `ShaftCull` and `SHAFT_TILE_TEXELS` are crate-visible only under `cfg(test)`, inside `lightmap_layer` | Needs a production re-export or a move |

**Where shared code lives.** Hull containment is a property of `TexelSampler`'s segments. The precedent's `texel_footprint.rs` already carries keep-in-step notes pointing at `lightmap_layer::shaft_cull`. The natural home for the test and margins is beside the sampler, in `texel_footprint/`, with `chart_may_cover` alongside or re-exported. Leaving them in `lightmap_layer` with crate re-exports also works. One copy either way.

## Orderings

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| W1 | A tile straddles two chunks that both list the light | The walk finishes chunk A's span, then walks chunk B's. The tile's texels arrive in two runs, separated by every other texel of A. | One test per (face, light). The verdict is reused across the split. Bytes equal. |
| W2 | A tile straddles a chunk that lists the light and one that does not | The unlisted chunk's texels are never visited for this light | The hull spans the whole tile. The unlisted chunk gains no entry. Bytes equal. |
| W3 | A cone edge, range edge or weight-epsilon fringe crosses a tile | The tile's first texel past the weight gate is not its first texel | Tested on that texel. The hull spans the tile. Bytes equal. |
| W4 | Interior not a multiple of 8; a one-texel chart; a chunk span clipped to the interior by outward rounding | The last tile on an axis is narrow; a rect texel past the interior is never walked | The hull covers exactly the tile's texels. An occluder within the pad of the last column keeps that tile blocked. |
| W5 | Cut sub-chart with a non-zero window origin | Tiles anchor at the sub-chart's first interior texel, on `exact_texel_local` over the window grid | Bytes and step report equal |
| W6 | Two lights on one face, baked as separate tasks on different workers | Each task holds its own verdicts and its own blocker hint | Identical across worker counts |
| W7 | A stepped (light, face), with a clear tile beside the step | The detector reads the grid after the walk, and traces only surface segments | The report is identical with culls on and off |
| W8 | A coverage-culled (face, light), if the coverage cull is built | The task returns before the walk, after the governor permit | No entries, no steps; progress still advances by one task |
| W9 | Cache filled by the base binary, read by the new one | Keys are unchanged | Hit; PRL identical to cold |

The precedent's O1 (a remembered occluder carried across a skipped run) and O5 (that occluder tested first in a clear tile) have no analogue: the walk has no cache. Without a cache the box-rounding leak cuts one way only. The traversal can call a real boundary hit clear, and the unculled walk can store that clear. A clear tile admits no accepted hit on any triangle, so the culled walk stores 1.0 exactly where the unculled one would.

## What the chunk lists already drop

- Chunk lists are a range filter: each light's `falloff_range` sphere against the chunk's world AABB, by closest point, in f32. It is per chunk with no pad, so it is tighter than the static walk's 0.5 m-padded `light_may_reach`. On a tilted face, the AABB is looser than the texel rectangle.
- Not dropped: lights behind the face, spots aimed away, and range misses within the AABB's slack. `cull_unlit_chunks`' doc says most chunks on long-range maps "sit behind walls or face away". Facing away is uncovered. Behind a wall is covered and dark.
- In this walk an uncovered texel costs no ray: the weight gate comes first. A coverage cull therefore saves only per-texel direct-term arithmetic, the task's grid allocation and its detector scan over an all-absent grid. In the static walk that class cost ~41 ns per visit (`lightmap-stage-sizing/findings.md` §3).

## Cost shape

- **Lit texels** (9 probes clear) are the shaft cull's whole saving. In the static walk they were the costliest class per texel, since no cache serves them. The same holds here.
- **Dark texels** are unaffected by either skip. Here they are probably dearer than in the static walk: with no remembered occluder, each of a dark texel's nine probes takes a full BVH traversal. In the static walk the cache answered 65.8% of hallway-mini's shadow rays, nearly all on dark texels.
- **Tile tests on dark-heavy faces** add cost and clear nothing. The precedent measured test cost at 4.6% of walk CPU, before its blocker hint. A dark-dominated stage could get slower, which the wall-time row guards.
- **Measured so far** (dev, cold `--release`, 5 workers): `stress-warren-mini` AnimWeightMaps 13.4 s vs Lightmap Bake 133.8 s, total 244 s; `kinematic-platform` 2.4 s vs 11.5 s; hallway-mini 0 s. The whole-stage entry re-bakes on any keyed edit, so warm iteration pays this cost too.

## Maps with baked animated lights

Counting authored keys (`_animated`, `style` ≥ 1, `brightness_curve`, `color_curve`, `direction_curve`) undercounts. `scripts/stress-warren.ts` pulses tagged baked lights through `setLightAnimation`, and script-derived membership makes them animated without a key (`stress-warren-mini`, `stress-warren-showcase`). Read the light count from the `[AnimatedLightChunks] … animated lights` log line instead.

| Map | Why it is a candidate |
|---|---|
| `stress-warren-hallway-inspection` | Full hallway: three `brightness_curve` lights. Its mini alone bakes cold in ~11 min, so the owner may launch the full map in their own terminal. |
| `stress-warren-showcase` | Script pulse plus curves |
| `stress-env-volumes` | Largest map source; three curves |
| `campaign-test` | Four `style 2` lights; shipped-content shape |
| `stress-warren-mini`, `kinematic-platform` | Already measured; byte gates |
| `test_animated_weight_maps_{single,mixed,occluded,cap}`, `animated-layer-spill` | Small byte gates |

## Adjacent findings

- **The key folds what the bake never reads.** `weight_map_cache_key` hashes whole `MapLight`s, animation curves included. Weights strip colour and intensity, and nothing in the stage reads `animation`. So a curve edit re-bakes the whole stage to identical id-25 bytes. Narrowing the key is a key change, not an exact skip.
- **An occluder cache for the weight walk** would cut dark-texel cost. It is not exact by the precedent's standard, though. The cache catches a boundary hit the traversal's box test misses, so it can flip clear to blocked (`segment_clear_remembering` doc), and the cull-free oracle would need it too. That is a measured byte decision for the owner, like the static walk's.
- **Chart plane drift** (`drafts/lightmap-chart-plane-drift/`) would move texel reconstruction onto a true-plane frame. The hull is built from the same exact chart formula as texel centres, so whichever frame lands, the hull must follow it. That re-check is the precedent's "a sampler change … must re-check" rule, now covering two walks.

## External sources

- Haines and Wallace, "Shaft Culling for Efficient Ray-Traced Radiosity" (Eurographics Workshop on Rendering, 1991). It introduced the shaft as the hull of a receiver box and an emitter box, with a candidate list per shaft. Its payoff is empty shafts. A non-empty shaft's candidate list could also narrow traversal, but here that changes traversal and so risks bytes. Only the empty-shaft skip is exact.
- Woop, Benthin and Wald, "Watertight Ray/Triangle Intersection" (JCGT 2(1), 2013). A watertight test would close the grazing and sliver gaps. Swapping `ray_triangle_hit` moves bytes in every bake stage, so the gaps stay accepted, as `BoundedRay`'s are.
- Pharr, Jakob and Humphreys, *Physically Based Rendering*, 3rd ed., §3.9 "Managing Rounding Error". It is the source of the γₙ bounds the precedent's margins use. Nothing here changes them.

## Measurement method (task 1)

- Configuration: base commit, dev profile, `CARGO_INCREMENTAL=0`, 5 workers, `prl-build <map> -o <out> --release`, one bake at a time on the Windows 1660 Super. Read the stage lines `AnimWeightMaps`, `Lightmap Bake`, `SH Bake` (control) and the total, plus the `[AnimatedLightChunks]` and `[AnimatedLightWeightMaps]` info lines.
- Split, if the gate passes: one counted run of the highest-share map with temporary counters, as `lightmap-stage-sizing/instrumentation.patch` did for the static walk. Counters: per (face, light) task, texels visited, past the weight gate, lit early-out, dark early-out and full set; and the CPU time for each class and for the detector. The counters never land.
