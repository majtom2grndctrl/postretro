# light-reach-cull

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline steps 3, 5, 6 and 12, §Build Cache · `context/lib/development_guide.md` §6.4 · read at 343ff98b6

## Problem

The roadmap's "lightmap dark-texel cost", sized by two developer-run spikes (`flood-cull-findings.md`, `leak-diagnosis-findings.md`, beside this brief). Lightmap Bake is about 80% of a cold full-hallway build: 860 s of 1079 s on the owner's Mac. About 74% of its covered (light, texel) pairs end as dark early-outs, nine probes blocked, mostly by light reaching through walls. The cause: the walk bounds a light only by range, cone and plane. Nothing bounds it by where light can travel through open space. The one flood the compiler has, `affinity_grid::light_portal_reach`, fills the whole connected component of runtime cells, so on a single-component map it rules nothing out. When this is done, Lightmap Bake skips a light's charts that open space cannot carry it to, through one bounded per-light reach index that later bakes can share. Output bytes and cache keys stay as they are. The compile-time watertightness check finds real holes, so the cull's precondition can be checked.

## Decisions

- **Prerequisite: `face-extract-split-epsilon` lands first** (branch `fix/face-extract-split-epsilon`). It closes every hole that let a ray cross solid from one open leaf to another, and it moves PRL bytes once. Every byte gate here compares against `main` after that fix. This brief does not re-specify it.
- **Exact by construction, verified like the shaft cull.** The standard is `plans/done/lightmap-exact-skips/`'s: byte-identical to the unculled walk, checked by fixture parity, a verify mode and whole-PRL `cmp` gates, not proven for every float input. A gate that moves a byte stops the build and goes to the owner as a measured amendment. No version bump is pre-authorized, and no cache key moves.
  - *Precondition:* every boundary between open space and solid inside a culling light's reach carries a face the hit test cannot miss.
  - *Named gaps:* a ray that crosses solid only through two sub-millimetre crease cracks that line up, at entry and exit; a ray lying in a wall's plane, which the hit test rejects as parallel while the BSP rounds it into solid.
- **One bounded reach index, shared across bakes.** A new compiler module owns it, not lightmap code (proposed: `light_reach`, `LightReachIndex`). For each eligible light it holds the set of open leaves its ball-bounded flood reaches. Its query takes a set of receiver leaves and answers whether the light reaches any of them. Lightmap Bake is its first consumer.
  - *The seam for later consumers:* direct SH and billboard scatter (through `direct_sh_bake::ReachIndex`), delta SH, animated direct SH, the entity-shadow direct deltas, and ChunkLightList all reach `light_portal_reach` today. Each moves onto this index in its own measured change, and the last move deletes `light_portal_reach`. Their savings are unmeasured, so this brief promises none.
  - *Why they do not move here:* they flood runtime cells, and this index floods the full tree. The two graphs can differ in connectivity (the hallway has two components in the full tree and one in runtime cells), so swapping them now would move SH-family bytes unmeasured (research §Consumers).
- **The flood runs on the full compiler BSP, not runtime cells.** It is the only graph the spike measured. The per-texel rule below needs exact sub-leaf solidity, and `RuntimeCells` leaves that to readers of the full tree. A cell can be as wide as the cell-size cap, so a ball flood over cells bounds less. The full tree's portals, dropped today once the exterior flood is done, now live until Lightmap Bake ends. This is compiler-internal; undoing it is cheap.
- **The flood.** Seeds: the open leaves the emitter sphere (`light_size` + 1 mm) overlaps. A portal is crossed only when its polygon comes within R of the light centre. The flood never enters a solid or exterior leaf. R bounds every segment the sampler traces for any texel the light lights: `falloff_range` + `light_size` + the footprint radius + a margin above rounding. Distances are exact, in f64.
- **Eligible lights: points and spots.** Suns, lights the embedded-emitter test flags, and lights whose origin sits in a solid or exterior leaf are never culled.
- **Chart test, relaxed.** A (light, chart) pair is culled when every open leaf the chart's padded, lifted texel slab touches lies outside the light's reach. Solid and exterior leaves the slab touches are ignored. The slab contains every ray origin and footprint point the sampler uses for that chart, including texel-centre drift off the face plane (`drafts/lightmap-chart-plane-drift/`). Where containment cannot be shown, the pair is kept.
- **Per-texel rule inside a culled pair.** A texel stores 0 without tracing only when its own padded footprint slab touches open leaves only. Every other texel of the pair traces as today. This covers buried texels, which crease cracks reach, and texels whose first millimetre of ray can enter a wall. Never key on the texel centre's leaf: that test skipped 21 lit hallway texels.
- **A culled texel keeps its record.** The walk, the Lambert term and a 0 record stay, as in the prototype.
- **The watertightness check reports holes, not parity spans.** Each odd-covered span is probed with exact chord tests, as the spike's span classifier does. A *hole* is open at any probe ring coarser than the finest: a missing strip of surface. A *crack* is open only at the finest ring: a sub-millimetre crease seam. A *covered* span is a parity artefact. Holes warn with location and brush provenance; cracks are counted at info; covered spans are not reported. The check stays a diagnostic and never fails the build (`partition/manifold.rs` header).
- **A map with any hole culls nothing.** The stage warns that the reach cull is off and why. Holes come from face extraction, not from authored intent, so a loud, slower, still-correct bake beats a second proximity test per light. Cracks do not switch it off; they are a named gap.
- **Verify mode is an invariant check compiled into every build** (`development_guide.md` §6.4). It is on in tests, and `POSTRETRO_LIGHT_REACH_VERIFY=1` turns it on in any build. It re-walks every culled pair unculled and requires every record to match bit for bit, skipped texels' 0 included. A mismatch fails the bake, naming the light, chart, texel and both values.
  - This departs from the shaft cull's test-only verify. That cull's correctness lives in its own geometry test; this one's precondition lives in authored maps, which fixtures cannot cover.
- **The stage logs one info line:** pairs culled, texels skipped, texels traced inside culled pairs, and whether the cull was switched off. These are stage totals, not the per-texel sizing counters the shaft-cull brief kept out. They do not touch the cache key.
- **Production always culls.** Culls-off is a test seam only, as for the exact skips. No prl-build flag.
- **Non-goals:**
  - *Moving the SH-family consumers onto the index:* above. Roadmap entry at landing.
  - *Dropping culled pairs whole* instead of writing 0 records. It saves more, but first needs proof that a missing record and a 0 record compose to the same bytes downstream (shadowmask coverage). Roadmap entry at landing.
  - *Closing crease cracks* by compiler edge snapping. Only if verify mode ever fires (leak findings §Recommendation 5).
  - *The animated weight-map walk.* It is a separate walk, which the exact skips also left alone.
  - *`drafts/geometry-integrity-validation/`.* Its compile-time hole check is this brief's, and the leak diagnosis answers its root-cause question. Retiring or rescoping it is the owner's call.

## Acceptance

### Automated

Test suite. Each fixture asserts the pair is culled or kept, and that lightmap and shadowmask bytes with the cull on equal the cull off, at penumbra floor on and off. Verify mode runs in every fixture.
- [ ] Two rooms split by a solid wall, a light in one, a chart on the far wall within range: the pair is culled. Texels whose footprint touches the wall's solid are traced, the rest are skipped.
- [ ] The same rooms joined by a doorway whose portal lies within R: kept. The portal moved just past R: culled.
- [ ] A chart whose texel centres sit in an unreached leaf while its padded slab reaches into a reached one: kept.
- [ ] In the far room, a chart on a brush top whose slab overhangs into that brush's solid: culled under the relaxed rule.
- [ ] A long oblique chart whose texel centres drift off its plane by more than the slab's smallest lift: bytes equal, culled or kept.
- [ ] Inside a culled pair, a traced texel walked right after a run of skipped texels, where the unculled walk's skipped texels would leave a different remembered occluder: bytes equal.
- [ ] An emitter sphere straddling a portal seeds both leaves.
- [ ] A sun, a flagged embedded emitter, and a light whose origin sits in solid: nothing culled.
- [ ] Hole check, on hand-built face sets: a missing 2 cm strip reads one hole at that strip's location; two crease edges missing each other by under a millimetre read a crack, not a hole; a T-junction and an overlap read covered and do not warn.
- [ ] A map with a hole culls nothing and warns. The same map closed culls.
- [ ] A planted fault, one texel forced to skip that the walk lights, makes verify mode fail and name that texel.
- [ ] Existing parity holds: production matches the reference oracle at each floor setting, warm equals cold, output is identical across worker counts, and overlap texels match across every face cut.

Gate runs. Machine-checked, launched by hand; the owner may run the long ones. Baseline: the post-fix `main` binary, same session, idle machine, one cold `--release` bake at a time.
- [ ] Whole-PRL `cmp`, identical: hallway, hallway-mini, movement-feel, kinematic-platform, `gate-heavily-lit` (sun), `combat-demo` (embedded emitters), `soft_shadow_test`; with `--no-penumbra-floor`, hallway-mini and `soft_shadow_test`.
- [ ] Verify mode on, zero mismatches: hallway, hallway-mini, movement-feel.
- [ ] Hole check reads no holes on hallway and hallway-mini, and the cull runs on both.
- [ ] Hallway Lightmap Bake ≤ 0.65× baseline, SH Bake reported as a control (prototype: 0.57× against the pre-fix baseline). Permit side: the stage log shows at least 40% of the hallway's (light, chart) pairs culled.
- [ ] Border cost: on the hallway, texels traced inside culled pairs are at most 10% of texels skipped (estimate: about 4%). Above that, stop and report before landing.
- [ ] movement-feel and kinematic-platform: Lightmap Bake within measurement noise of baseline.
- [ ] Warm: the new binary over a cache the baseline binary filled hits every lightmap layer and writes a PRL identical to the cold bake.
- [ ] Grep gate on the diff: no new prl-build flag, no spike instrumentation or `POSTRETRO_LM_` variable, and no cache stage version constant changed.

### Manual

None. No rendered pixel changes: every output byte is gated by `cmp` above.

## Path

- **Prototypes on `spike/lightmap-flood-cull-sizing`.** `lm_flood_sizing::ball_flood`, `ball_radius`, `sphere_leaves`, `point_polygon_distance`, `chart_leaves` and `chart_deep` (its inverse is the per-texel rule); `lm_leak_trace::classify_open_spans` for holes. Starting points only; the Decisions supersede their tolerances. No spike instrumentation lands.
- **Full-tree portals.** `RuntimeCells::of_partition` generates and drops them. Return them, or regenerate (46 ms on the hallway).
- **Hole check placement.** `check_watertight` runs before the exterior flood; classification needs the tree and the flood. Move the warning after `of_partition`.
- **Cull placement.** A culled pair is still walked, so the flag belongs at the walk (`lightmap_layer::chart_walk::bake_light_layer_chart_clipped`), not in `plan_charts`. The per-texel mask depends only on the chart, so compute it once per chart and share it across lights.
- **Seams.** `lightmap_layer::ExactSkips` and the test tallies in `lightmap_layer/skip_stats.rs`. Its `verify` field and negative-control knob are `#[cfg(test)]`; this verify mode needs its switch outside `cfg(test)`, and the knob is the planted-fault precedent. The culls-off seam serves the re-walk.
- **Rivals.** The baked cell-visibility relation (id 46), which the roadmap line suggested: its components are the unbounded flood, which skips nothing on single-component maps. The strict chart rule keeps 4% of the win.
- **First slice.** The hole classifier and the index, with the two-rooms fixture under verify mode. Then one counted hallway run to confirm the permit side before tuning anything.
- **File size.** `pipeline.rs` is past 2,000 lines: wire through `pipeline/lightmap_stage`, not inline. `lightmap_layer/tests.rs` is past 2,700: new fixtures get their own test module.
- **Docs at landing.** `build_pipeline.md` step 12 §Exact skips gains the reach cull and its contract; step 3 gains the hole check. Tick the roadmap's "lightmap dark-texel cost" and add the two follow-ups.

## Open questions

- **Remembered-occluder divergence: a third named gap?** — owner: Dan — **blocks build**. Inside a culled pair, skipped texels do not update `last_occluder` or `clip_occluder`. The unculled walk's skipped texels would. A traced texel after them then tests a different cached triangle first, and `segment_clear_remembering`'s box-rounding catch can answer differently (research §Occluder caches). The prototype skipped whole pairs, so it never met this. Recommendation: accept it as a third named gap, since verify mode compares whole pairs and checks it on every gate map. Consequence: the contract names three gaps, not two. The only exact alternative is to cull a pair only when every texel in it is skippable, which keeps little of the win, because charts border walls.
- **R's footprint term,** per chart or the map's largest — **delegated**.
- **Whether texels traced inside a culled pair still take the clip pre-pass and shaft cull.** Both are exact — **delegated**.
- **Index storage,** per-light leaf bitset or sorted list; under 1 MB on the hallway either way — **delegated**.
