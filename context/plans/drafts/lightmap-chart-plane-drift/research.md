# lightmap-chart-plane-drift — research

Read at 076470c8c. Notation: ε = `RAY_EPSILON` = 1e-3 m, n = `chart.normal` (the decoded stored normal), n_f = the true face normal, lv = a texel's v coordinate measured from `chart.origin`.

## Mechanism

| Claim | Ground |
|---|---|
| The chart normal is the stored vertex normal of the face's first vertex when that normal is present. Otherwise it is `edge1 × edge2`. | `lightmap_bake/charts.rs::plan_charts` |
| `u_axis = normalize(edge1)` lies in the face plane. `v_axis = normalize(n × u_axis)` does not, unless n = n_f. | same |
| The encoder maps [−1, 1] to [0, 65535] with `round`. An axis-aligned normal's zero component encodes at 32767.5 → 32768 and decodes to 1.53e-5, so the decoded normal is ~1.5e-5 rad off-axis. An oblique normal's error is up to ~one code step per component (~3e-5). | `crates/level-format/src/octahedral.rs::encode`, `decode` |
| Texel centres are `origin + u_axis·lu + v_axis·lv`, so their distance from the face plane is lv·(v_axis·n_f). That is linear in lv and changes sign across `chart.origin` (vertex p0). | `chart_raster.rs::chart_texel_world_position` |
| Sub-charts of a cut face reconstruct on their parent's grid and frame, so the drift runs across the whole parent extent. | `chart_raster.rs::chart_grid`; `lightmap_bake/face_cut.rs` |
| Vertex lightmap UVs come from projecting vertices, which lie on the face, onto the same axes, then quantising to u16. The runtime mapping is consistent; only reconstruction leaves the plane. | `atlas_layout.rs::assign_lightmap_uvs`, `chart_texel_position` |
| The soft-visibility seed is keyed on the frame's exact f32 bits. | `chart_raster.rs::chart_texel_seed` doc |
| The default density is 0.04 m per texel. Charts reach 2048 texels, about 82 m. | `build_pipeline.md` worldspawn `_lightmap_density`, step 12 |

## Self-hit threshold

- The ray origin sits at texel centre + n·ε. A centre d below the face puts the origin ε − d above it, which is below the face once d > ε.
- `segment_clear_remembering` skips the first ε along the ray and `ray_triangle_hit` is double-sided. So a ray rising at elevation φ hits its own face when (d − ε)/sin φ > ε, that is when d > ε(1 + sin φ). That is 1–2 mm.
- Reach: about 66–132 m on axis-aligned faces, and about half that on oblique ones. An oblique face at the atlas's ~82 m limit drifts about 2.5 mm (~3e-5 rad × 82 m), past the threshold.
- Above the face (d < 0) the origin is higher than intended. That matters only for occluders within a millimetre or two of contact, which the floor's contact fixtures sit just above (penumbra-floor plan, Delegated answers).
- The footprint clip and lead-in segments start from the same lifted centres (`texel_footprint.rs`), so they inherit the drift. Iteration 0's fixture should report whether any clip segment hits its own face.

## Frame readers (starting inventory; iteration 0 confirms)

| Reader | Uses the frame for | Role |
|---|---|---|
| `chart_raster.rs::chart_texel_world_position` | texel-centre reconstruction | geometric |
| `texel_footprint/clip_reach.rs::ClipBand::for_chart` | the band plane, `u_axis × v_axis` | geometric |
| `texel_footprint.rs` (`TexelFrame`) | footprint offset directions and pitch | geometric |
| `animated_light_chunks.rs` | chunk world AABB | geometric |
| `chart_raster.rs::chart_texel_seed` | seed keyed on the frame's f32 bits | identity |
| `atlas_layout.rs::assign_lightmap_uvs` / `chart_texel_position` | vertex lightmap UVs | identity |
| `lightmap_bake/face_cut.rs` | sub-chart grid coordinates | identity (check: does it also place cut geometry?) |
| `lightmap_layer/cache_keys.rs` (`atlas_layout_fingerprint`) | layout key hash | identity |
| `working_set.rs` | sizing estimate | check |

## Candidates

Primary family (R: split the frame by role):

| Id | Variant | Expected blast radius | Expected churn |
|---|---|---|---|
| R1 | Add n_f and v′ = normalize(n_f × u_axis) to the chart. Route each geometric reader to (u_axis, v′, n_f). | Lightmap-family only; UVs and seeds untouched | Texels whose drift or clip result is above rounding |
| R2 | As R1, but the true-plane frame differs from the stored frame only where a chart's worst drift exceeds f32 rounding | As R1 | Only long charts; short charts stay bit-identical |
| R3 | As R1, with the clip band taking n_f directly instead of u_axis × v′ | As R1 | As R1 |

Notes:
- The reference bake shares the chart readers, so reference parity holds by construction and cannot rank candidates.

Fallback (C1: build the frame in the face plane):

| Id | Variant | Expected blast radius | Expected churn |
|---|---|---|---|
| C1 | `v_axis = normalize(n_f × u_axis)`, with n_f oriented to agree with the stored normal. Shading keeps the stored normal. | Lightmap-family; some quantised vertex UVs may move (geometry section) | Global: the frame bits change, so every seed changes |

Notes:
- Gram–Schmidt of v against n_f gives the same axis up to rounding, so it is not a separate candidate.

Excluded before iteration:
- **Position-only correction**, rejected in Decisions: the `ClipBand` plane stays tilted.
- **n_f for shading.** The Decisions keep shading on the stored normal.
- **Moving `chart.origin` to the chart centre.** It only halves the reach.
- **Own-face exclusion and lifting the origin along n_f.** Rejected in Decisions.
- **The encoder fix.** A non-goal.

## Measurement maps

| Map | Why |
|---|---|
| `shadowmask-groups-capture` | Hard lights; fast; the BC5 fixture |
| `soft_shadow_test` | Soft lights; a BC5 fixture |
| `gate-heavily-lit` | Sun, spot, large soft points; a BC5 fixture |
| `combat-demo` | Embedded emitters |
| `stress-warren-hallway-inspection-mini` | Wall time only, final pair |
