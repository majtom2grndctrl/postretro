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

## Margin derivation

The premise holds: a per-triangle margin makes a clear tile exact, outside the two gaps the Decisions accept, rays under the angle floor and slivers past the derivation (§Candidates and the sliver gap). Separation along a triangle's own normal needs no ray-angle term, but it keeps a shape term ρ. Coordinates do not enter the hit test's rounding. They enter once per hull, through the sampler's own point arithmetic.

Randomized f32 searches checked each bound (scratch scripts, not landed). The worst observed errors are 18%, 5% and 42% of the unscaled bounds below.

### Notation

| Symbol | Meaning |
|---|---|
| u, γ_n | u = 2⁻²⁴, γ_n = nu/(1 − nu) |
| a, b, c | The triangle's f32 vertices in index-buffer order, a = `indices[tri]` |
| e1, e2, s | `b − a`, `c − a`, `origin − a` rounded to f32 exactly as `ray_triangle_hit` forms them. T′ = (a, a+e1, a+e2) and o′ = a + s are their exact images. |
| N′, ρ, e_T | N′ = e1 × e2 in exact (f64) arithmetic. ρ = ‖e1‖‖e2‖/‖N′‖ = 1/sin(angle at a) ≤ 1/sin(narrowest angle), and ∞ when N′ = 0. e_T = max(‖e1‖, ‖e2‖). |
| o, d, t_max | The f32 ray MT sees. d = delta/length, a unit vector to within 3u. t_max is `max_distance` (`segment_clear_remembering`) or `length` (`front_faces_clear`). |
| θ | Angle between d and plane(T′) |
| B, S_T, L | B is H's box. S_T = ‖a − centre(B)‖ + ½·diag(B), which is ≥ ‖x − a‖ for every x ∈ H. L = diag(B), which is ≥ every t_max. |
| R_i, ε_i, e_H | Per world axis i, defined below the table |

- R_i is the larger of two quantities:
  - abs(`chart.origin`ᵢ) + abs(u_axisᵢ)·(abs(uv_min_u) + uv_extent_u) + abs(v_axisᵢ)·(abs(uv_min_v) + uv_extent_v);
  - the largest abs(xᵢ) over B.
- ε_i = 10u·R_i + 6u·L.
- For a unit axis a, e_H(a) = Σ abs(aᵢ)·ε_i ≤ ‖ε‖.

### What must hold

- **The accepted hit.** It needs abs(det) ≥ 1e-8, u ∈ [0, 1], v ≥ 0, fl(u + v) ≤ 1 and 0 < t < t_max, all in f32.
  - `segment_clear_remembering` starts ε along the segment and stops strictly before its target.
  - The lead-in starts at `from` and accepts `dist < length`.
- **Every triangle, every segment.** Traversal pruning, the occluder cache, the `ClipBand` and the front-face filter only drop triangles. So it suffices that no triangle in the scene gives an accepted hit on any tested segment. Every segment then answers clear, whatever the traversal and cache state.
- **The tested quantity.** The cull tests dist(T, H), certified from below by a gap along a unit axis a: g_a = min over T of a·x − max over H of a·x, or the mirror.

### Lemma: Cramer residuals

- **The accepted values.** On an accepted hit, let ū = num_u/det, v̄ = num_v/det and r̄ = num_t/det, taken over the computed f32 numerators and det in real arithmetic.
  - `inv_det` followed by the multiply rounds by at most γ_2 and keeps sign.
  - So ū, v̄ ≥ 0, ū + v̄ ≤ 1 + 4u, and 0 < r̄ < T′ = t_max/(1 − γ_2).
- **The residual.** Let X′ = o′ + r̄·d and W = a + ū·e1 + v̄·e2, with W within 4u·e_T of T′. Let r = X′ − W.
- **The bounds.** MT's numerators are Cramer's rule for o′ + t·d = a + u·e1 + v·e2. By linearity:

| Residual | Equals | Bound |
|---|---|---|
| N′·r | num_t − r̄·det | E_t + r̄·E_d |
| (d × e2)·r | num_u − ū·det | E_u + ū·E_d |
| (d × e1)·r | −(num_v − v̄·det) | E_v + v̄·E_d |

- **The rounding.** Each numerator and det is an f32 cross product, then a dot.
  - Its error is ≤ γ_5·Σᵢ abs(xᵢ)(abs(y_j z_k) + abs(y_k z_j)) ≤ γ_5·√2·‖x‖‖y‖‖z‖, by Cauchy–Schwarz on the second factor.
  - This holds for glam's scalar `Vec3` in any summation order, with or without FMA.
  - So E_d ≤ γ_5√2‖e1‖‖e2‖, E_t ≤ γ_5√2‖e1‖‖e2‖‖s‖, E_u ≤ γ_5√2‖s‖‖e2‖ and E_v ≤ γ_5√2‖s‖‖e1‖. The bound ‖d‖ ≤ 1 + 3u is folded into γ.
- **The three subtractions.** Their rounding is carried as geometry. ‖o′ − o‖ ≤ u‖o − a‖ ≤ u·S_T, and T′ lies within u·e_T of T.

### Normal certificate: no angle term

- **The bound.** Row 1 alone gives abs(N′·(X′ − a))/‖N′‖ ≤ (E_t + T′·E_d)/‖N′‖ ≤ 5√2·u·ρ·(S_T + L).
  - The 1/sin θ in t's error cancels the 1/sin θ distance along the ray, because the identity bounds height, not t.
  - It covers near-parallel rays and a sign flip in det.
- **The certificate.** So some tested point lies within δ_N = 5√2·u·ρ(S_T + L) + u·S_T of plane(T′). Suppose all of H lies on one side of plane(T′), with gap g_N ≥ κ(δ_N + e_H(n̂′)). Then no hit is accepted.
- **ρ remains.** MT forms s × e1 component by component, so its error scales with ‖e1‖‖e2‖, not with ‖N′‖. ρ is exact per triangle and needs no floor. §Margin scale's expectation holds for θ, not for the shape term.
- **Measured.** Over 1.7 M accepted hits on segments off the plane, the worst g/(uρ(S_T + L)) is 1.46, against a bound of 8.07. It stays flat down to sin θ = 6e-5.

### Other axes: angle floor θ0

- **The reach.** Write r = α·d + β·e1 + γ·e2 (γ here is a coefficient, not γ_n).
  - The residuals give abs(α) = abs(N′·r)/abs(det), abs(β) = abs((d × e2)·r)/abs(det) and abs(γ) = abs((d × e1)·r)/abs(det), with abs(det) = ‖N′‖·sin θ.
  - So dist(X′, T′) ≤ abs(α) + abs(β)‖e1‖ + abs(γ)‖e2‖ + 4u·e_T ≤ 5√2·u·ρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ + 4u·e_T.
- **Why there must be a floor.** Every term grows as 1/sin θ. As θ → 0, all three residual normals turn perpendicular to d, which pins the line but not the point.
  - Measured in-plane reach, in units of uρ(S_T + L): 2.7, 14, 180 and 1670 at sin θ ≈ 0.1, 0.01, 1e-3 and 1e-4.
  - The bound itself held: the worst observed reach is 0.36 against 7.07.
- **The certificate.** Suppose θ ≥ θ0 for every tested segment. Let δ_A = 5√2·u·ρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ0 + u(S_T + 5e_T). Then g_a ≥ κ(δ_A + e_H(a)) excludes a hit.
- **Below θ0 is the accepted gap.** Write δ_A = K/sin θ0 + (terms free of θ). A triangle certified at gap g can still give a false hit only on a segment that satisfies both conditions:
  - sin θ < K/g, which is ≤ sin θ0;
  - the segment crosses the triangle's plane within K/sin θ of it.

### Segment endpoints against the exact hull

- **The exact hull.** H is the exact-maths hull built from the f32 inputs: the chart frame, the uv bounds, the light centre `light.origin as f32` and `light_size`. §Hull containment places every exact-maths segment inside it.
- **What is not counted.** Rounding inside the footprint offset, the lift and the sphere direction is left out. Their exact-maths values stay inside the pad, inside the slab, and inside the 1 mm sphere slack.
- **The count.** Each f32 segment MT sees differs from its exact-maths segment, per world component, by at most:

| Step | f32 operations | Error per component, first order |
|---|---|---|
| p (`chart_texel_world_position`) | lu and lv, 3 each, acting along the chart axes. Then origin + u·lu + v·lv, 4. | 7u·R_i |
| q, `from` | + offset, + lift | + 2u·R_i |
| Target | centre + dir·size | u·R_i; size·u falls inside the 1 mm slack |
| The ray | delta, length, dir, origin = from + dir·ε, `max_distance`, and t up to T′ | + u·R_i + 6u·L |
| Clip `from`, lead-in ends | One more add, over a short L | Within the same bound |

- **The total.** ε_i = 10u·R_i + 6u·L. The worst measured ratio is 4.2.
  - A tested point lies within e_H(a) of H along a unit axis a.
  - It lies within ‖ε‖ of H in Euclidean distance.
- **Coordinate magnitude.** It enters only here. MT subtracts a first, so its own rounding does not change under translation.
  - This replaces the per-triangle "|coord|" numerator in §Margin scale.
  - The coordinate term is per hull and per axis. For a floor it reads only the floor's coordinate along its normal axis.

### Margins and constants

- m_N(T) = κ·[5√2·u·ρ(S_T + L) + u·S_T + e_H(n̂′)]
- m_A(T, a) = κ·[5√2·u·ρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ0 + u(S_T + 5e_T) + e_H(a)]
- A candidate is separated when g_N ≥ m_N(T), or when g_a ≥ m_A(T, a) along any other tested axis a. One unseparated candidate blocks the tile.

| Constant | Value | Ground |
|---|---|---|
| κ | 2 | Covers second-order terms (relative error < 1e-6), ‖d‖ ≠ 1, and f64 evaluation of g and m. The proven factor is 1 + 1e-5; the rest guards against an operation miscount. At κ = 1.1, the receiver crossover below moves from 315 m to 658 m. |
| MT ulp multiple | 5√2 ≈ 7.07 per numerator | γ_5 per cross-then-dot, and √2 from Cauchy–Schwarz |
| Sampler ulp multiples | 10·R_i, 6·L | The endpoint table |
| θ0 | 2° | m_A scales as 1/sin θ0. For a 3 m triangle at the origin, m_A is 1.1 mm at 2°, 0.44 mm at 5° and 0.24 mm at 10°. The acceptance rows' 10° wall sits above the floor, so its clear is exact. |
| ρ_cap | 100, so the angle at a is ≥ 0.57° | Sizes the query growth only (§Candidates and the sliver gap) |
| Absolute floor | None | Every term is relative to S_T, L or R_i, each ≥ ε. f32 underflow adds less than 1e-44 m. |

Worked check. The setup:
- θ0 = 2°, L = 5 m, and the scene translated by T along all three axes;
- 3 m triangle: right isosceles, ρ = 1.41, S_T = 6 m;
- 5° trim: ρ = 11.5, edges 3 m;
- 60 m floor: right triangle with 60 m legs, ρ = 1.41, S_T = 88 m.

| Margin (m) | Origin | 104 m | 1 km | 4 km |
|---|---|---|---|---|
| m_N, 3 m triangle | 2.3e-5 | 1.5e-4 | 1.2e-3 | 4.9e-3 |
| m_N, 60 m floor | 1.3e-4 | 2.6e-4 | 1.3e-3 | 5.0e-3 |
| m_A, 3 m triangle | 1.1e-3 | 1.3e-3 | 3.1e-3 | 9.5e-3 |
| m_A, 5° trim | 8.1e-3 | 8.3e-3 | 1.0e-2 | 1.7e-2 |
| m_A, 60 m triangle | 1.4e-2 | 1.5e-2 | 1.6e-2 | 2.3e-2 |

- **Receiver clearance.** It is 0.5ε = 5e-4 m, and the receiver separates only by m_N.
- **Origin rows.** Separated off-normal, the 10° wall clears just over 1.1 mm and the 5° trim just over 8.1 mm.
  - Fixtures should call the margin function and place each triangle at 1.05·m to clear and 0.95·m to stay blocked.
  - With the trim's narrow angle at index 0, ρ = 11.5. At either other index, ρ ≈ 1.
- **104 m.** m_N for the 60 m floor is 0.26 mm, under the 0.5 mm clearance, so the open floor records clear tiles. The fixture `Receiver` has no drift.
- **4 km.** m_N is about 5 mm, ten times the clearance. Every receiver blocks, and the cull declines, correctly.
- **Crossover.** A receiver blocks every tile once its coordinate along its normal axis passes about 315 m (60 m triangle) or 405 m (3 m triangle).
  - The limit is per axis: a floor at modest height clears anywhere in x and y.
  - §Margin scale's 1 km row ("a few ulps fit") is therefore optimistic for receivers.

### Receiver separation

- **Only the normal separates.** The receiver and its coplanar neighbours take the normal certificate. The receiver's in-plane extent covers the tile, and its plane is within 0.5 mm of H everywhere, so no other axis separates it.
- **The gap is measured on the hull as built.** g_N is the height of H's lowest support point above plane(T′): 0.5ε·(n·n̂′) plus the signed drift lv·(v_axis·n̂′) at the tile's worst corner.
  - The §Receiver drift term is not a margin term. The hull is built from the drifted centres, so g_N already includes it.
- **The drift budget is 0.5ε − m_N.** For the 60 m floor, that is 0.37 mm at the origin and 0.25 mm at 104 m.
  - At a 1.5e-5 rad tilt, that allows abs(lv) ≤ 25 m and 16 m, on the side where texels drift toward the face.
  - The other side gains clearance and never blocks.
- **A sphere crossing the plane.** An emitter sphere that crosses the receiver plane leaves no certificate. Those tiles test blocked, which costs speed only.

### Candidates and the sliver gap

- **Uncandidated triangles still need separating.** A triangle the query does not return must still be separated.
  - A triangle at distance D from B has S_T ≤ D + 2e_T + diag(B).
  - So D ≥ m_A once D ≥ D*(T) = κ·[5√2·u·ρ(3·diag(B) + L + 8e_T)/sin θ0 + u(diag(B) + 7e_T) + ‖ε‖] / (1 − κ(15√2·u·ρ/sin θ0 + u)).
- **The requirement.** Test every triangle whose box lies within D*(T) of B. Two realizations:
  - Query B grown by G = max D*(T), using the map's largest ρ and ρ·e_T over triangles with ρ ≤ ρ_cap. G is about 2 mm for 3 m triangles, and 0.11 m in a map holding a 3 m sliver at ρ = 100.
  - Use a small fixed growth, plus a per-map list of the triangles whose D* exceeds it, box-checked each tile.

  Which one costs less is a measurement.
- **Exact AABB queries.** An AABB query cannot miss a returned box by rounding (§Exactness premises). The query box must be rounded outward when narrowed to f32. A box gap is a lower bound on D.
- **Degenerate triangles inside the query.** A triangle with ρ > ρ_cap or N′ = 0 has no certificate there, so it blocks the tile. That is correct.
- **Degenerate triangles outside the query.** Such a triangle is the accepted sliver gap, and it is not confined to "beside" the hull.
  - A near-degenerate f32 sliver (ρ ≈ 2e7, 19 m long) gave accepted hits on segments 37 m away. Whenever its line nearly meets a ray's line, u, v and t are noise.
  - The unculled bake already reports those phantom hits.
  - Counting the triangles with ρ > ρ_cap on the gate maps sizes this gap. Zero means the gap is empty there.

### Implementation requirements

1. Read the inputs the sampler reads:
   - the stored f32 vertices in index order, a = `indices[tri]`;
   - the light centre as `light.origin as f32`, as `hard_ray_target` uses it;
   - the f32 chart frame.
2. Round e1 and e2 to f32 as MT does. Then evaluate N′, ρ, the edge lengths, S_T, the supports and the gaps in f64. Never narrow g or m to f32. κ absorbs f64 rounding, so no directed rounding is needed.
3. Build H's corners in f64 from the exact formula: chart.origin + u_axis·lu + v_axis·lv, with lu and lv from the grid integers. Corners taken from f32 `chart_texel_world_position` instead add 7u·R_i to ε_i.
4. The normal axis is N′, from the triangle's own f32 edges. It is not the face normal and not `chart.normal`. All of H's support lies strictly on one side: the 8 slab corners, and the sphere at c·n̂ ± r.
5. Normalize every other SAT axis in f64. A zero axis gives no certificate, and so does any non-finite value.
6. The emitter is a sphere of radius `light_size` + 1 mm about the f32 centre, or the point for a hard light.
7. Candidates meet the D* requirement in §Candidates and the sliver gap. Degenerate triangles inside it block.
8. A sampler change that adds an f32 operation to any segment endpoint re-counts ε_i, alongside the §Hull containment re-check.
9. A unit test feeds the margin function to a randomized f32 MT search like the one behind this section. It asserts no accepted hit beyond m_N, and none beyond m_A for θ ≥ θ0. Verify mode remains the oracle.

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
