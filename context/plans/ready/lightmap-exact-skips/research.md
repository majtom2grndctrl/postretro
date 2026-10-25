# lightmap-exact-skips — research

Read at d116329d7. Numbers, method and lever notes: `../lightmap-stage-sizing/findings.md`. Notation: ε = `RAY_EPSILON` = 1e-3 m, p = texel centre, n = `chart.normal`, q_k = p + footprint offset k.

## Hull containment

The hull from the brief's shaft-cull Decision contains every segment piece the unculled sampler tests, for points and spots.

| Segment | Sampler | Tested piece | Inside the hull because |
|---|---|---|---|
| Shadow trace, every k | floored (`footprint_visibility`) | q_k + n·ε → light centre (hard, and k = 0) or a sphere-surface point (`area_sample_target`). The tracer skips the first ε and stops strictly before the target. | Origin at height ε, inside the 0.5–2.5ε slab, and offset within the pad. Target inside `light_size` + 1 mm. The hull is convex. |
| Clip segment, k > 0, when `may_clip` | floored | p + n·ε − ô_k·ε → q_k + n·ε, in-plane at height ε | Within the pad (radius + 4ε) |
| Lead-in, k > 0, after a clear trace | floored (`lead_in_clear`) | origin − along·0.25ε → origin + along·1.25ε, tested over its full length, front faces only | Back end: height 0.75–1.25ε, in-plane ≤ radius + 0.25ε. Forward end: within 2.25ε of q_k with the lift and 1.25ε along the receiver (`ClipReach` doc). So it is inside slab and pad when along·n ≥ 0, and otherwise on the traced segment, inside the hull by convexity (segment ≥ 1.25ε). |
| Shadow trace | floor-off (`TexelSampler::Centre`, `soft_visibility_with`) | p + n·ε → centre (hard) or the 4 probes and then the full set on the sphere (soft) | Pad 4ε is enough; there are no offsets, clips or lead-ins |
| Sun trace | both | q_k → q_k + dir·10 km (`DIRECTIONAL_LIGHT_RAY_LENGTH_METERS`), from the unlifted point | **Not contained** by a world-diagonal sweep. This is the reason suns are excluded. |

## Margin scale

| Coordinate magnitude | f32 ulp | Against the 0.5 mm nominal receiver clearance |
|---|---|---|
| < 128 m (Quake's ±4096-unit grid is 104 m) | ≤ 1.5e-5 m | Tens of ulps fit, so the cull fires |
| 1 024 m | 1.2e-4 m | A few ulps fit |
| 4 096 m | 4.9e-4 m | One ulp fills it, so the cull declines |

- The hit test is Möller–Trumbore with no barycentric epsilon (`ray_triangle_hit`). Its out-of-triangle reach is about ulp·(|coord| + |segment|) / (sin α · sin θ), where θ is the ray's angle to the triangle's plane and α is the triangle's narrowest angle. The per-triangle margin covers the numerator. The acceptance rows pin θ ≥ 10° and α ≥ 5° near the origin; at larger coordinates, fewer such triangles clear.
- Below those angles, the margin cannot bound the reach for every input. A grazing sliver beside a clear hull can make the unculled tracer report a hit the geometry does not have. This is the gap the brief's exactness Decision accepts, as `BoundedRay` does (`ray_traversal.rs` doc; `plans/done/bake-ray-speed-contract.md`).
- The reach grows with the distance from the ray origin to the triangle's vertices and with its edge lengths, not with the hull's coordinates alone (`s = origin − a` in `ray_triangle_hit`; `prune_limit`'s doc). A tile near the origin can face a 60 m triangle. So the margin is per candidate triangle.
- Expected, to be confirmed by the derivation: separation along a triangle's own normal needs no angle term. A segment that ends at least d short of the plane has hit distance t past its end by d/sin θ, and t's rounding also scales as 1/sin θ, so the ratio is angle-free. The receiver's triangles separate this way, which keeps a large floor triangle from blocking every tile at 104 m.
- `prune_limit` (`bound + |bound|·1e-4 + 1e-2`) bounds a hit distance against a box entry, not barycentric reach. Only its form carries over.

### Receiver drift under quantised normals

- `chart.normal` is the decoded octahedral vertex normal (`crates/level-format/src/octahedral.rs`). An axis-aligned normal decodes about 1.5e-5 rad off-axis. The encoder rounds 32767.5 up.
- `v_axis = normalize(n × u_axis)` (`lightmap_bake/charts.rs::plan_charts`) therefore tilts out of the face plane. Texel centres (`chart_raster.rs`) drift off it by lv·(v_axis·n_face): about 0.18 mm across a 12 m chart, and 0.5 mm at lv ≈ 33 m, sooner on oblique faces. The drift is linear in lv and changes sign across `chart.origin`.
- The hull is built from texel centres, so containment is unaffected. Only the receiver's clearance below the slab changes: 0.5ε·(n·n_face) + lv·(v_axis·n_face). On one side of a chart past ~33 m, tiles test blocked.
- The fixture `Receiver` (`texel_footprint/test_fixtures.rs`) uses an exact normal and cannot show this.

## Exactness premises

| Claim | Ground |
|---|---|
| Visibility is a count ratio. All clear and unclipped gives exactly 1.0f32. | `footprint_visibility` (`clear as f32 / surviving as f32`); `soft_visibility_with` |
| The caches write only on a hit. | `segment_clear_remembering` sets `last_occluder` only inside its hit branch. `lead_in_clear` has no cache. |
| No other per-texel side effect: outcome flags are dropped, pairing is texel-local, and the embedded flag and `ClipReach` are per light or per chart. | `bake_light_layer_chart_clipped`, `EmitterPairing`, `texel_footprint/embedded_emitter.rs` |
| The hit test has no barycentric epsilon, so it can report hits within f32 rounding outside a triangle. It rejects \|det\| < 1e-8. | `ray_triangle_hit` |
| The hit test's rounding grows with the distance from the ray origin to the triangle's vertices and with its edge lengths, not with the hull's coordinates alone. | `ray_triangle_hit`; `prune_limit` doc |
| An AABB BVH query cannot miss a primitive by rounding: inclusive compares on exact face boxes. Only the ray query can, which is the cache's catch. | bvh 0.12 `Aabb::intersects_aabb`; `BoundedRay` |
| The direct term reads the light centre only. `light_size` enters visibility only. | `light_contribution_and_direction`, `light_texel_contribution_and_visibility_with` |
| Falloff is zero only beyond `falloff_range.max(1e-4)`. The inverse models are strict, so they are nonzero at exactly the range. | `falloff` |
| The spot term is zero at cos θ ≤ cos outer. Angles are half-angles in radians. Defaults: inner 0, outer inner + 0.01, aim −Y. | `spot_cone`, `MapLight` docs |
| Every texel of a chart uses `chart.normal`. That normal comes from a vertex normal, with `u_axis = edge1`, so texel centres can sit off the plane through `chart.origin`. | `for_each_light_layer_chart_texel`; `lightmap_bake/charts.rs::plan_charts` |
| The walk visits interior texels only, at `chart_texel_world_position`. That position is affine in exact maths, and f32 can put it a few ulps outside the corner rectangle. | `chart_interior_dims`; `chart_texel_bounds` pads 0.5 m for this |
| `plan_charts` has one consumer (`Window::start`). The layer key (`footprint_radius`), the detector (`detect_steps`) and the shadowmask estimate each call `light_may_reach` themselves. | `partitions.rs`, `lightmap_layer/cache_keys.rs`, `shadowmask_bake/estimate.rs` |
| The reference walk has no `light_may_reach` and no fused walk, so it independently checks both culls. It shares `TexelSampler`, `segment_clear_remembering` and `ClipReach`. | `production_stage_matches_the_reference_bake_at_each_floor_setting`, `frozen_oracle_has_no_fused_walk_dependency` |

## Orderings

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| O1 | Clear tile B between shadowed tiles A and C along the walk's rows, one occluder | The walk is row-major, so each texel row walks A's run, skips B's, then walks C's. The remembered occluder crosses B's skipped run eight times. | Bytes equal with culls on and off. Verify passes at each skipped texel, with the caches as the walk holds them there. |
| O2 | Open floor, soft point, cone covering part of the chart | A tile is tested once, on its first covered texel. Its verdict holds across its eight row runs. | One test per tile with a covered texel, none per tile without. A clear tile's texels trace no segment. |
| O3 | A spot cone edge or the range edge crosses a tile | The tile's first covered texel is not its first texel in walk order | The hull spans the whole tile. Bytes equal; verify passes. |
| O4 | Interior not a multiple of 8; a one-texel chart (the interior is floored at 1×1) | The last tile on each axis is narrower than 8 | The hull covers every texel the tile holds and none it lacks. An occluder within the pad of the last column keeps that tile blocked. |
| O5 | Shadowed tile walked just before a candidate-clear tile; its occluder lies outside the clear tile's hull bounds by less than the margin | The remembered occluder carries into the clear tile, where it is tested first | That triangle is a candidate, and the tile does not clear |
| O6 | Cut face; a light whose range or plane leaves one sub-chart dark | Planning tests each sub-chart's own corners. Tiles anchor per sub-chart, so overlap twins fall in different tiles. | That sub-chart is dropped and the others kept. Twins stay bit-identical with both culls on. |
| O7 | The chart cull drops every chart of a (light, layer) | Planning runs only on a cache miss, and dropped charts advance progress there. An empty plan still finishes and caches an empty partition. | Progress completes at its published total. The cached partition equals the unculled one. |
| O8 | Cache filled by the base binary, read by the new binary | A section-memo hit skips planning and the walk. A layer hit skips the walk. | Keys are unchanged, so every memo and layer hits, and the PRL is identical |

## Existing tests the chart cull flips

- `chart_cull_keeps_chart_at_padded_bounds_range_and_drops_it_one_ulp_beyond` (`pipeline/lightmap_stage/tests/window_tests.rs`) keeps a chart the light reaches only through the 0.5 m pad. The exact range test drops it.
- The neighbouring spot-aimed-up fixture in the same file asserts every quad bakes. The cone test drops those pairs.
- Both go through a helper that asserts progress (`build_pipeline.md` makes progress a contract).

## Excluded paths

- **Animated weight maps** (`animated_light_weight_maps/face_bake.rs`).
  - Same sampler, but plain `segment_clear` with no cache.
  - Walks chunk-ordered spans per (face, light) and filters through chunk light lists (`animated_light_chunks.rs`).
  - Its oracle is `texel_major_reference`.
  - Maps with baked animated lights: about a third of `content/dev/maps/` by authored keys, including the full hallway, `stress-warren-mini` and `campaign-test`. Stage cost unmeasured; 0 s on hallway-mini.
- **Shadowmask cut-face estimate** (`shadowmask_bake/estimate.rs`). Its own sparse walk, with `segment_clear_remembering` and `light_may_reach`.

## Fixture maps

| Map | Lights |
|---|---|
| `stress-warren-hallway-inspection-mini` | 338 spots, 43 points, all `_light_size 0.75` |
| `shadowmask-groups-capture` | 4 hard points |
| `gate-heavily-lit` | the only `light_sun`, 1 spot, 15 points (size 10) |
| `combat-demo` | 8 spots; entities 9, 14, 15 flag as embedded emitters |
| `kinematic-platform` | 17 spots, 4 points, one style light |
