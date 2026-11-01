# lightmap-reach-cull — research

Derivation behind the brief's Decisions. Read at 343ff98b6. Evidence:

| Doc | Origin |
|---|---|
| `flood-cull-findings.md` (here) | branch `spike/lightmap-flood-cull-sizing` at 8b2eb15a9: sizing, prototype, soundness |
| `leak-diagnosis-findings.md` (here) | same branch: why rays cross solid, the epsilon fix, the span classifier |
| `drafts/lightmap-stage-sizing/findings.md` | `main`: the earlier study that led to the exact skips |
| `done/lightmap-exact-skips/` | `main`: the shaft and chart culls, and the exactness standard this brief matches |
| `fix/face-extract-split-epsilon` contract | in flight: the split-epsilon prerequisite |
| `watertight-tracer` | planned: the tracer prerequisite |

## Exactness argument

The ball holds every segment the sampler traces for a lit texel: from any emitter point (within `light_size` of the centre) to any footprint point (within the footprint radius of a texel centre within `falloff_range`). A segment starting in an open leaf leaves it only through a portal or into solid. Inside the ball, a portal within R is crossed by the flood. So a receiver leaf outside the flood is reached only through solid, and the ray hits a face on the way in, unless:

- the boundary has no face there (a hole). The precondition, checked by the coverage proof.
- the ray enters solid inside its skipped first millimetre (`RAY_EPSILON`). Only texels within that reach of a closed leaf can do this, and the per-texel rule traces them.
- it starts in solid (a buried texel). Traced, as above.
- it slips, at entry *and* exit, where edges share no exact vertex: a crease crack (1–250 µm wide, on non-axial creases only) or a T-junction. The tracer's tolerance τ closes those narrower than it (`watertight-tracer`). The coverage proof reads any crack it cannot prove within τ as a hole, which switches the cull off. Named gap 1.
- it crosses from one open leaf to a diagonal neighbour through a shared edge with no portal. When the leaves beside the edge are open, the flood reaches the neighbour through them. When they are solid, the ray grazes an edge between solids, which the tracer's watertight test and tolerance close. Not observed.
- it lies in a face's plane. Named gap 2. The spike found one such ray on hallway-mini.

The exterior is never entered. A ray into exterior space from inside crosses solid through an interior-facing face, which exterior culling keeps.

Movers, fluid, trigger and other peeled-off brushes are in neither the tree nor the static BVH (`build_pipeline.md` §Compiler pipeline step 1), so the flood and the rays agree about them. A surface in the BVH but not solid in the tree would only shrink the cull. The unsafe direction, solid in the tree with no face in the BVH, is the hole case.

## Coverage proof

Why the parity check can't switch the cull (`partition/manifold.rs` `check_watertight`):
- *The weld hides narrow cracks.* Edges group by their line's direction, rounded per component to `DIR_QUANTUM` (1e-6), and anchor, the line's closest point to the origin, rounded to `ANCHOR_QUANTUM` (0.1 mm). A crack's two edges on one key share a parity count and cancel. Anchor rounding hides up to √3 × 0.1 ≈ 0.17 mm; direction rounding adds about 1.7 µm per metre from the origin, passing τ's starting 0.5 mm near 190 m.
- *"Covered" is sampled.* The spike's classifier probed each odd span with rings at its quarter points and called it covered when no chord there opened (leak findings §Method). A crack that varies along a crease slips between them: of movement-feel's 51 crease-crack leak crossings within 1 mm of a span, 22 sat on covered spans (§Open-edge link).

Why the proof is sound:
- Distance from a point to a convex polygon is convex in the point, so along a segment it peaks at an end. A sub-span whose ends lie within τ of one face lies within τ of it throughout. BVH triangles serve as candidates: each is convex, and a point within τ of a triangle is within τ of its face.
- Any split is sound so long as each piece's ends share a face; splitting where the nearest face changes keeps pieces few. Only ends are evaluated, in f64 with rounding counted toward a hole, and no quantum merges anything, so the proof holds at any coordinate.
- It bounds the tracer's crack-width metric directly: every point of every edge lies within τ of another face.

## Occluder caches

`segment_clear_remembering` tests the remembered triangle before traversing. Its doc says the answer is unchanged "barring a hit the traversal's box test misses by rounding, which the cache then reports as blocked". The shaft cull kept bytes because a clear tile's rays never hit, so the caches end a skipped tile as they began.

Here, the unculled walk's skipped texels are dark: their rays hit, and each hit writes `last_occluder` (and `clip_occluder` on the clip path). With the cull, those writes do not happen. The next traced texel in the pair tests a different cached triangle first. On `main` the answers differ when one of the two cached triangles catches a hit the traversal misses: by box-test rounding, or by the distance prune near a grazing sliver's end (`ray_traversal` prune limit). The prototype skipped every texel of a culled pair, so it could not meet this. Verify mode, comparing whole pairs, does.

Exact alternatives, and why they lose:
- *Cull only pairs with no traced texel.* Charts border the walls their light is behind, so almost every culled pair has a traced border. Close to the strict rule's 4% of the win.
- *Replay the cache writes without tracing.* The triangle a skipped texel's ray would hit is unknown without the trace.
- *A watertight tracer (chosen, the `watertight-tracer` prerequisite).* The traversal reaches every triangle the watertight hit test would hit, and the remembered triangle is tested only when its leaf box passes the traversal's own predicate. The answer then no longer depends on the cached triangle, so skipping cache writes can't change it. Its byte move, cache epoch and cost belong to that plan.

## Other consumers

`affinity_grid::light_portal_reach` floods runtime cells from the light's cell, unbounded. Callers:
- `affinity_grid::decompose_affinity_for_lights_with_policies`, behind `decompose_affinity` (delta SH), `decompose_affinity_for_lights` (direct SH via `direct_sh_bake::build_reach_index` → `ReachIndex`; billboard scatter; the entity-shadow selected-direct plan in `pipeline.rs`), and animated direct SH.
- `chunk_light_list_bake`, directly. Its output is a runtime light list, not an SH bake.

All read `runtime.cells.tree`, `runtime.cells.exterior_cells` and `runtime.portals` (`pipeline.rs`). Moving any of them onto the lightmap index is a non-goal, because each needs a different bound:
- *Delta SH bounce (id 27)* traces from gather-ray hits to the light (`build_pipeline.md` step 12, Soft-shadow bake cost). A probe outside the ball can see a lit surface inside it, so a direct-light ball is not conservative. It needs a bound grown outward from the surfaces the light reaches.
- *Direct SH, billboard scatter and the probe-point deltas* receive at probes read trilinearly, so their R drops the texel footprint and widens by about one probe spacing (`drafts/lighting-scale--sh-delta-receiver-reach-cull/`).
- *Runtime filtering* (chunk light lists, and portal-gated reach for runtime lights and fog) needs the runtime-cell graph and runtime portal ids. Runtime never holds the full tree, and id 43's closed-mover associations name runtime portals. `done/occluder-first-partition` put light floods on runtime cells for this reason.

Mover and door reach is `ready/mover-light-flow--foundation/`'s research step 3 (`research/gi-rtx-lite/index.md` §Build order), which redefines light → cell reach to cover direct-SH receivers (§Bake-only artefacts). The lightmap index serves none of it. Its natural second consumer is the animated weight-map walk (`drafts/anim-weightmap-exact-skips/`): same charts and R, and no occluder cache, so §Occluder caches does not arise there.

## Graph choice

| | Full BSP leaves | Runtime cells |
|---|---|---|
| Measured | yes: flood (B) | no |
| Hallway open nodes / portals | 3,600 leaves / 9,370 | 1,857 interior cells |
| Hallway components | 2 | 1 |
| Exact sub-leaf solidity for the per-texel rule | native | needs the full tree anyway |
| Ball bound inside a node | leaf-sized | up to the cell-size cap |
| Portals at bake time | regenerate or keep (46 ms on the hallway) | already kept |

`RuntimeCells` (pipeline/runtime_cells.rs) states the split: every cell-id reader takes runtime cells; the full tree and its exterior flood stay with readers of exact sub-leaf solidity. The reach cull is the second kind.

## Slab containment

The chart slab is the spike's: the chart's padded texel rectangle at lifts 0.25, 1 and 2 mm, padded by the footprint radius × 1.05 + 2 mm, against every leaf it touches. It must contain every shadow-ray origin and footprint point. The lightmap-chart-plane-drift draft measures texel centres drifting off the face plane, linearly across a chart, up to 2.5 mm on long oblique faces. A slab built from the true face plane can then miss the leaf a drifted origin sits in. Build the slab from the reconstructed texel positions the walk uses, or keep pairs whose drift exceeds the slab's lift. The per-texel slab uses the same pad and lifts around one texel.

## Cost

- Prototype (whole pairs skipped, 0 written): hallway Lightmap Bake 860.2 → 491.4 s, `cmp`-identical. Flood build 0.5 s summed over 757 lights; full-tree portals 46 ms.
- Per-texel rule (estimate, unmeasured): it traces about a one- or two-texel border of each culled chart, plus overhang texels. That is roughly 4% of the 1,733 M skipped dark early-outs, about 100 CPU-s against about 2,700 saved. The buried share alone is at most 1.12 M texels.
- The epsilon fix restores faces, so the shaft cull clears less: instrumented Lightmap Bake +5–8% on the small maps. The hallway's post-fix baseline is unmeasured. The wall-time gate is a ratio against that baseline.
- Coverage proof: unmeasured. Each edge tests the faces within τ of it, so cost grows with edge count times local face density; the hallway's time is a gate row.
- Memory: a per-light bitset over the hallway's 9,624 leaves is 757 × 1.2 KB, under 1 MB, alive from before Lightmap Bake to its end.

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Verify mode re-walks a culled pair | The re-walk runs after the culled walk of the same pair. It starts both occluder caches empty at the pair's first texel and takes every exact skip production takes except the reach cull. Traced texels inside the culled walk take the same skips. | Records equal bit for bit. A difference can come only from the reach cull, never from the shaft cull or the clip pre-pass. |
| P2 | Verify mode on, one governor permit, a map with culled pairs | The re-walk runs inside the culled walk's permit and does not enter the governor again. The per-chart texel mask builds inside the first walk that needs it, takes no lock or permit, and serves later lights read-only. | The bake completes. Each culled pair holds one permit and advances progress once. |
| P3 | Per-chart texel mask shared across lights | Lights walk the same chart on different workers in any order. Whichever walk comes first builds the mask. | The mask is a function of the chart alone. Bytes and cull tallies are identical at one worker and many, cold and warm. |
| P4 | Traced texel after skipped ones in a culled pair | Skipped texels write no occluder. The next traced texel tests the cache the culled walk holds, which differs from the unculled walk's. | The two walks' remembered occluders differ at that texel, and its record equals both the unculled walk's and a brute-force test of every triangle. |
| P5 | Verify mode on over a filled cache | The baseline binary fills every lightmap layer and section entry. The new binary then builds warm with verify on. | No lightmap layer or section entry is read. Every culled pair is re-walked, pairs re-walked equal pairs culled, and the PRL equals the cold bake. |
| P6 | Crease crack wider than τ, narrower than 2τ | The tracer blocks a ray through its middle, which passes within half the width of an edge. The coverage proof cannot prove the edge within τ of another face. | It reads a hole and the map culls nothing. Bytes match; only bake speed is lost. |
| P7 | Coverage proof against the traced faces | Exterior culling, then the oversize-face cut and, when it cuts, its BVH rebuild, run before the proof. Test seams that move or remove faces act before both the proof and the BVH. The walk's first pair starts after the proof. | The proof reads exactly the faces the lightmap walk traces (`watertight-tracer` C11): a T-junction the cut adds is proven, a removed face leaves an unproven span, and the cull's switch is settled before any pair is walked. |
