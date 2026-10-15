# reflection-probe-seams — research

Read at 949a79411, on top of `feature/reflection-probes`.

## Grouping evidence
`stress-env-volumes.map` comes from `tools/gen_stress_map.py --grid 8 6 3` with `tools/stress_environment.py`. Every fluid recipe draws its own top height (`floor_pool`: `zf + randint(48, 160)`; `floating_block`: random `z0`; `multi_brush_pool`: random depth, a step 64 u higher), so two fluid entities in one room rarely share a plane. A parse of the map text (flat tops only; sloped tops bind nothing) found:

| Grouping | Groups (upper bound, ignores walls and box match) |
|---|---|
| Per (entity, height), today's rule before doorway splits | 511 |
| Same height across entities, XY within 1024 u | 501 |
| Same height across entities, XY within 2048 u | 483 |
| Any height within one storey, XY within 768–1024 u (≈ per room) | 119–147 |

Cross-entity merging at one height saves at most a few percent there, so it does not relieve the cap; the stress map's drops are real per-height demand plus doorway splits. It is chosen for consistency in authored content (water beside sludge, abutting pools), not for count, so it merges only touching entities. Merging every box-matching entity in a room would put two separate fountains in one courtyard on one capture point at the edge of one of them, costing the other parallax with no seam between them to fix. Merging across heights would fit the stress map under the old cap but serves a higher plane from a capture point a storey below it, and authored `reflection_volume` boxes already offer that trade per room, with a warning.

## Rivals
- **One stretched box per canal.** Box projection is exact only on the box's walls; a union of two rooms puts each far wall at the union's extent, the geometry jump `reflection-probes`' canal row forbids.
- **Pairwise ramps per shared edge.** A neighbour weighs ½ at the shared edge and falls to 0 at W inside the face. Exact for two probes, but a doorway piece D narrower than W between rooms A and B steps along the A–D edge: A's side mixes ½A + ½D, D's side ½A + (½ − w_B)D + w_B·B, a step of w_B·(B − D). The parent binds a doorway-spanning pool to three probes (`reflection-probes` `plan.md` Corrections), and walls are thinner than any useful band, so this is the common case, not a corner.
- **Lagarde influence volumes.** Authored or fitted influence boxes larger than projection boxes, up to N overlapping per fragment. It needs an influence box per probe, and weights from overlapping boxes do not guarantee continuity at the binding boundary. Region-distance weights come from the binding itself, so continuity follows by construction.
- **Fold narrow doorway pieces into a room's probe.** Keeps pairwise ramps exact by making every crossing two probes, but changes the parent's per-piece binding and still breaks at corners.
- **Blending in the capture (bake the neighbour into the tile edge).** A tile is a direction map at one point; position-dependent mixing cannot live in it.

## Blend math
A probe's region Ωₚ is the water bound to it at one plane height. For a fragment at x: fₚ = max(0, 1 − dist(x, Ωₚ) / W), with fₚ = 1 inside Ωₚ, and wₚ = fₚ / Σ f. The weights are a function of x and the regions alone, so two faces meeting at a boundary compute identical weights there, provided each stores every probe whose region is within W of that boundary point. With at most two stored besides its own, that holds wherever at most three regions lie within W of the point; a fourth breaks continuity there, the accepted bound.

Doorway piece D (width < W) between A and B: on A's side at the A–D edge, f_A = 1, f_D = 1, f_B = 1 − width/W; on D's side the same three values. Continuous. A region's distance is measured to its stored nearest boundary segment; for a fragment outside the region that equals the true distance while the nearest boundary point lies on that segment, and overestimates it near a bend, delegated in the brief. Each sample is capture × its own ratio; the mix is R = Σ wₚ · Rₚ; Fresnel then weights R once, so the composite rules of `reflection-probes` are unchanged.

Pixel style evaluates dᵢ at the cell's world-space centre, the same point its single sample already uses.

## Cost levers
The parent prices one reflecting fragment at one tap, about 20 ALU of lobe weights and, per present light, a 48 B descriptor load and about 90 ALU; the worst case is K = 4 with full curves, about 600 ALU and 68 storage loads (`reflection-probes` `plan.md` Corrections, live-lighting cost). A three-probe band triples that inside the band. On e24 the reflecting shader's mere presence cost 1.29 ms from NVIDIA codegen (`plan.md` §Task 9b), so instruction count is not a safe proxy, and the spike measures. Levers if it is over budget:
- **Per-probe ratio precompute**: each probe's ratio per lobe evaluated once per frame (a small compute or CPU pass into the slot table), leaving the fragment one tap and a lobe blend per probe. It is the parent's recorded follow-up lever and makes extra samples nearly free; it costs a per-frame write.
- **Stochastic per-cell pick**: each water texel cell samples one probe chosen by its weights from a hash of the cell. One sample always; the crossfade reads as a dither, which fits pixel style and needs a stable hash so it never shimmers.
- **Third pipeline variant**: draws whose faces carry no nearby probe keep the single-sample reflecting variant. It confines cost to band draws, at the price of a third shader variant under the DX12 FXC test.

## Pin table
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| B1 | Two-room boundary | Fragment pair straddling the A–B edge, ε apart | Outputs differ ≤ pinned epsilon, both styles |
| B2 | Narrow doorway piece | Piece width < W between rooms A and B; fragment pairs straddling the A–D and D–B edges | Continuous along both edges; weights in [0,1], sum 1 |
| B3 | Four regions within one band | Four probes' regions within W of one point | Each face keeps its two nearest; the break is confined to points within W of all four |
| B4 | Light in one probe only | Light stored only by Q switches off | P's face darkens only within the band, by Q's weight |
| B5 | Old section | Section 54 at an earlier version | No nearby probes; output identical to `reflection-probes` |
| B6 | Two heights touching | Faces at two plane heights meet in plan view | No nearby probe recorded, no blend |
