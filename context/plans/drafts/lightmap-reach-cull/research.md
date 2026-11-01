# lightmap-reach-cull — research

Derivation behind the brief's Decisions. Read at 343ff98b6. Evidence:

| Doc | Origin |
|---|---|
| `flood-cull-findings.md` (here) | branch `spike/lightmap-flood-cull-sizing` at 8b2eb15a9: sizing, prototype, soundness |
| `leak-diagnosis-findings.md` (here) | same branch: why rays cross solid, the epsilon fix, the span classifier |
| `drafts/lightmap-stage-sizing/findings.md` | `main`: the earlier study that led to the exact skips |
| `done/lightmap-exact-skips/` | `main`: the shaft and chart culls, and the exactness standard this brief matches |
| `fix/face-extract-split-epsilon` contract | in flight: the prerequisite fix |

## Exactness argument

The ball holds every segment the sampler traces for a lit texel: from any emitter point (within `light_size` of the centre) to any footprint point (within the footprint radius of a texel centre within `falloff_range`). A segment starting in an open leaf leaves it only through a portal or into solid. Inside the ball, a portal within R is crossed by the flood. So a receiver leaf outside the flood is reached only through solid, and the ray hits a face on the way in, unless:

- the boundary has no face there (a hole). The precondition, checked by the hole check.
- the ray enters solid inside its skipped first millimetre (`RAY_EPSILON`). Only texels within that reach of a closed leaf can do this, and the per-texel rule traces them.
- it starts in solid (a buried texel). Traced, as above.
- it slips through a crease crack at entry *and* exit. Named gap 1. Cracks are 1–250 µm wide, on non-axial creases only.
- it lies in a wall's plane. Named gap 2. The spike found one such ray on hallway-mini.
- it crosses from one open leaf to a diagonal neighbour through a shared edge with no portal. When the leaves beside the edge are open, the flood reaches the neighbour through them. When they are solid, the ray grazes an edge between solids, which rounding decides. Not observed.

The exterior is never entered. A ray into exterior space from inside crosses solid through an interior-facing face, which exterior culling keeps.

Movers, fluid, trigger and other peeled-off brushes are in neither the tree nor the static BVH (`build_pipeline.md` §Compiler pipeline step 1), so the flood and the rays agree about them. A surface in the BVH but not solid in the tree would only shrink the cull. The unsafe direction, solid in the tree with no face in the BVH, is the hole case.

## Occluder caches

`segment_clear_remembering` tests the remembered triangle before traversing. Its doc says the answer is unchanged "barring a hit the traversal's box test misses by rounding, which the cache then reports as blocked". The shaft cull kept bytes because a clear tile's rays never hit, so the caches end a skipped tile as they began.

Here, the unculled walk's skipped texels are dark: their rays hit, and each hit writes `last_occluder` (and `clip_occluder` on the clip path). With the cull, those writes do not happen. The next traced texel in the pair tests a different cached triangle first. The answers differ only when one of the two cached triangles catches a hit the traversal's box test misses by rounding: the same edge case the exact skips accepted, now reachable by a new path. The prototype skipped every texel of a culled pair, so it could not meet it. Verify mode, comparing whole pairs, does.

Exact alternatives, and why they lose:
- *Cull only pairs with no traced texel.* Charts border the walls their light is behind, so almost every culled pair has a traced border. Close to the strict rule's 4% of the win.
- *Replay the cache writes without tracing.* The triangle a skipped texel's ray would hit is unknown without the trace.
- *Make the traversal conservative (chosen, owner 2026-10-31).* Pad the bounds the traversal tests so it reaches every triangle the direct test would hit. The answer then no longer depends on the cached triangle, so skipping cache writes can't change it. It moves bytes once wherever the old traversal missed a hit by rounding. Its cost is a slightly wider traversal, gated at 2% of hallway Lightmap Bake; past the gate, the fallback is the owner's call.

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
- Hole check: span classification ran under 3 s on the hallway at 557 spans; post-fix the hallway has 26.
- Memory: a per-light bitset over the hallway's 9,624 leaves is 757 × 1.2 KB, under 1 MB, alive from before Lightmap Bake to its end.
