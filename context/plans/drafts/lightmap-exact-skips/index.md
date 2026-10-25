# lightmap-exact-skips

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Build Cache · read at d116329d7

## Problem

The owner accepted the penumbra floor's Lightmap Bake cost (`plans/done/lightmap-penumbra-floor/`) and wants the time back. A sizing spike (`plans/drafts/lightmap-stage-sizing/findings.md`) found two kinds of work whose answer the scene already decides:
- Fully lit texels trace nine footprint rays each, and no occluder cache can serve them. Most sit in tiles no triangle could shadow.
- `plan_charts` filters (light, chart) pairs by range alone, so most of the pairs it keeps light no texel.

When this is done, the static layer walk skips both. A tile that provably sees its emitter stores visibility 1 and traces nothing. A pair that provably lights no texel is never walked. Output bytes and cache keys stay as they are on every gate.

## Decisions

- **Exact by construction, verified by digest.** A clear tile is a geometric proof that every ray from it is unblocked. A dropped pair has a zero direct term at every texel. The standard is `plans/done/bake-ray-speed-contract.md`'s: byte identity checked by fixture parity and whole-PRL `cmp` gates, not proven for every float input.
  - The known gap: on a grazing sliver beside a clear hull, the hit test's rounding can exceed any finite margin. There the unculled bake can report a hit the geometry does not have, which the cull would store as lit.
  - A gate that moves a byte stops the build and goes to the owner as a measured amendment, as the contract's step 2 did. The Manual rows say what the measurement holds. No version bump is pre-authorized.
  - With no byte moved, no version or key moves, and existing caches stay valid.
- **Tile shaft cull: points and spots, in the static layer walk, on both samplers.** For a tile of texels to test clear, no triangle may cross the convex hull of two parts:
  - the tile's texel-centre rectangle, padded and lifted;
  - the emitter sphere at `light_size` + 1 mm, or the emitter point.

  The pad covers every footprint offset on the floored path. It shrinks to the ray lift's margin on `--no-penumbra-floor`, which the owner noted this cull also helps.
- **A clear tile leaves the walk unchanged.** A clear verdict needs every triangle whose box comes within its own margin of the hull's box to be separated from the hull by at least that margin. Then every segment the unculled sampler would trace from the tile misses, and visibility is exactly 1.0. The occluder caches write only on a hit, so they end the tile as they began. Later rays, and the cache's box-rounding catch (`segment_clear_remembering`'s doc), see no difference.
- **The margin is per candidate triangle.**
  - It bounds the hit test's rounding for that triangle. The rounding grows with the triangle's vertex magnitudes, its edge lengths, its distance from the hull, and, across its edges, its narrowest angle and the rays' grazing angle.
  - A large receiver triangle must not stop tiles clearing at authored scale. The expected route is that separation along a triangle's own normal needs no angle term (research §Margin scale).
  - Rays grazing a triangle below a fixed angle floor, and slivers past the derivation, are the accepted gap.
  - The derivation goes in `research.md` before code, and its constants go in the plan of record.
  - Where a triangle comes within its margin, the tile tests blocked and only speed is lost. That happens at large coordinates, and on long charts whose texel centres drift off the face plane (research §Receiver drift).
  - Speed is owed at authored scale (inch-unit TrenchBroom maps, hundreds of metres across). Correctness is owed everywhere.
- **Suns get no shaft cull.**
  - Sun segments run a fixed 10 km, so a hull would need a world-bound argument and a margin valid at that scale.
  - A slanted shaft's query box spans most of the map, so the test could cost more than the rays it saves.
  - Neither measured map has a sun.

  Suns still take the chart cull.
- **Embedded-emitter lights take the shaft test unchanged.** Geometry inside the sphere always crosses the hull, so their tiles never test clear. They stay correct and save nothing.
- **Chart coverage cull before the walk.** It drops a pair only when every interior texel's direct term is exactly zero:
  - a point or spot light behind the plane at all four corner texel centres;
  - a sun whose N·L, computed with the bake's own f32 expression, is ≤ 0;
  - a light whose exact nearest distance to the texel-centre rectangle exceeds `falloff_range` by a margin;
  - a chart whose bounding sphere lies outside the spot's outer cone by a margin.

  Each test bounds an affine or monotone quantity at its extremes. Its margin stays above f32 rounding of texel positions.
  - The plane test reads all four corners, because the chart normal comes from a vertex normal and need not be orthogonal to the u axis.
  - The cone test runs only for an outer angle strictly between 0 and a right angle and a non-zero aim. Any other spot is kept: declining is exact, and authored spots sit well inside that range. When the light lies inside the chart's bounding sphere, the pair is kept.
- **Other consumers keep `light_may_reach`.**
  - The layer key's footprint padding keeps it so keys do not move.
  - The texel-step detector and the shadowmask estimate keep it because changing them saves nothing.
- **Culls-off is a test seam only.** It adds no CLI flag, env var or key input, and production always culls.
- **The sizing instrumentation does not land.** It stays in `lightmap-stage-sizing/instrumentation.patch`.
- **Non-goals:**
  - **The animated weight-map bake.** It is a separate chunk-ordered walk on plain `segment_clear` with its own oracle, and its cost has never been measured. This brief measures it. If it is material, it gets a roadmap entry rather than widening this scope.
  - **The shadowmask cut-face estimate walk.** Its share of the stage was never separated. The same measurement covers it.
  - **Sharing the clip pre-pass across lights** (0.1% of the stage).
  - **Lever B** stays declined (penumbra-floor plan, owner ruling).
  - **Floor-off wall time.** `--no-penumbra-floor` is a compiler diagnostic. Its bytes are gated; its speed is not measured.
  - **Dark-texel cost:** the roadmap's "lightmap dark-texel cost", to be sized first.
  - **Moving texel centres back onto the face plane.** It would move bytes; the drift costs this cull speed only on long charts.
  - **The reference oracle** (`lightmap_bake::reference`). It stays cull-free so it checks both culls.

## Acceptance

### Automated

Chart coverage cull, both sides of every test. Each row asserts the pair is dropped or kept, and that lightmap and shadowmask bytes with culls on equal culls off:
- [ ] A point light 1 cm behind a chart's plane is dropped, and one just in front is kept. A soft light whose centre is behind but whose sphere crosses the plane is dropped.
- [ ] A chart whose normal is not orthogonal to its u axis, with the light behind the plane at one corner and in front at another, is kept.
- [ ] A spot just outside its outer cone is dropped, and one just inside is kept. A spot with no authored aim or angles culls by the bake's defaults.
  - Kept: a spot whose outer angle is at or beyond a right angle, or negative.
  - Kept: a spot with a zero aim.
  - Kept: a spot inside the chart's bounding sphere, even aimed away from the chart.
- [ ] For each falloff model, a light just past `falloff_range` of the nearest texel centre is dropped, and one exactly at range is kept.
- [ ] A sun with N·L ≤ 0 is dropped, and one with N·L just above 0 is kept.
- [ ] The existing range-cull fixtures that pin a pair the new tests drop now assert the drop: a spot aimed away from every chart, and a light whose range reaches only the padded bounds. Both still match the unculled bake, and progress completes at its published total (research O7).

Tile shaft cull. Every fixture's bytes match with culls on and off, at floor on and off. The verify assertions run in each fixture as well:
- [ ] An open floor under a soft point light, where tiles test clear, and the same floor with an occluder shadowing part of the chart.
- [ ] A clear tile between two shadowed tiles that share one occluder, side by side along the walk's rows, so the remembered occluder crosses the skipped run on every texel row (research O1).
- [ ] A tile that a spot's cone edge or the range edge crosses, so its first covered texel is not its first texel (research O3).
- [ ] A chart whose interior is not a multiple of 8 on either axis, and a one-texel chart, each with an occluder just past its last texel column inside the pad (research O4).
- [ ] A triangle outside the padded hull by less than the margin keeps the tile blocked, and one outside by more clears it. Each is also the remembered occluder of a shadowed tile walked just before (research O5).
- [ ] Near the origin: a wall the tile's rays meet at 10°, and a trim triangle whose narrowest angle is 5°. Each lies outside the padded hull by just over its margin, and the tile tests clear (research §Margin scale).
- [ ] Each ordering fixture records the clear tile its ordering needs:
  - the middle tile between two shadowed tiles;
  - the tile a cone or range edge crosses;
  - a narrow last tile that clears once its occluder moves past the pad.
- [ ] The occluder fixture translated to coordinates of at least 4 km, where the cull may decline every tile.
- [ ] A hard point light, a spot, and a flagged embedded emitter.
- [ ] A sun bakes with no tile tested.
- [ ] On a cut face, a light whose range or plane leaves one sub-chart dark drops only that sub-chart through the stage. Overlap texels stay bit-identical with both culls on, and the fixture records a clear tile beside an overlap (research O6).
- [ ] Verify mode: each clear tile is also traced normally. Every covered texel returns 1.0, and both occluder caches end equal to the skipped walk's.
- [ ] In the open-floor fixture, each tile with a covered texel is tested once and a tile with none is never tested. A clear tile's texels trace no shadow segment. With the clip pre-pass off, a clear tile's texels also trace no clip or lead-in segment (research O2).
- [ ] Permit side. A cull that never fires, or a margin too wide for authored map scale, fails here.
  - The open-floor fixture records at least one clear tile at the origin. It also does so translated to ~104 m (Quake's ±4096-unit grid) with the floor a single triangle tens of metres long.
  - The behind-the-plane fixture records at least one dropped pair.
  - The reference-parity fixture records at least one clear tile and one dropped pair at each floor setting, so the cull-free reference checks both culls.
- [ ] Existing parity holds:
  - production matches the reference bake at each floor setting;
  - an embedded emitter bakes alike on every path;
  - mixed-light bakes match across worker counts, with warm equal to cold;
  - overlap texels bake bit-identical across every face cut.

### Manual

- [ ] Cold whole-PRL `cmp`, base-commit binary against the new one, identical:
  - floored: hallway-mini, `shadowmask-groups-capture`, `gate-heavily-lit` (sun, spot), `combat-demo` (embedded emitters), `kinematic-platform` (animated light), `soft_shadow_test`;
  - `--no-penumbra-floor`: `shadowmask-groups-capture`, `soft_shadow_test`.
- [ ] Warm on `shadowmask-groups-capture` and `soft_shadow_test`, the new binary over a cache the base binary filled (research O8):
  - it logs a hit on both section memos;
  - with the section memo re-keyed by another `--direction-texel-scale`, it logs a hit on every lightmap layer;
  - both runs write a PRL identical to the cold bake.
- [ ] If any gate moves a byte, the build stops and reports the moved-texel count, the largest change, and each moved texel's blocking hit re-tested in double precision.
- [ ] Grep gate on the diff: no new prl-build flag, no `POSTRETRO_LM_` env var, no sizing counters in production code, and no cache stage version constant changed.
- [ ] Wall time, against a same-session base-commit baseline. Idle machine, one bake at a time, dev profile, default 5 workers, SH Bake reported as a control. The owner may launch the long runs in their own terminal.
  - hallway-mini cold Lightmap Bake ≤ 0.62× baseline (prototype 0.56×);
  - `shadowmask-groups-capture` median of three ≤ 0.65× (prototype ~0.54×).
- [ ] Measure and report, on one cold bake of a map with baked animated lights (`stress-warren-mini` or `campaign-test`):
  - AnimWeightMaps wall time against Lightmap Bake;
  - the shadowmask estimate's share, if the stage log separates it, or a note that it does not.

  Add a roadmap entry for either if it is a material share.

## Path

- **Prototypes.** `chart_may_cover` and `shaft_clear` in `lightmap-stage-sizing/instrumentation.patch` (against 00c9d8362), wired into `LayerPartitions::plan_charts` and `bake_light_layer_chart_clipped`. Treat them as a starting point; the Decisions supersede their tolerances and tests.
- **Separation test.** The prototype uses a separating-axis test over the receiver normal, the triangle normal, and each triangle edge crossed with the chart axes, the normal and the shaft direction. A distance test against the per-triangle margin would also satisfy the Decision.
- **Tiles.** Fixed 8×8 tiles are the measured optimum (findings §1).
  - The prototype tests a tile lazily on its first covered texel, inside the visibility closure, so uncovered texels never test.
  - Tile state sits beside the per-(light, chart) `last_occluder` / `clip_occluder` cells. The walk runs one chart on one thread, so it needs no synchronisation.
  - The floored pad reads the texel frame's radius; the `Centre` sampler has no footprint.
- **Chart cull placement.** In `plan_charts`, after the existing `light_may_reach` filter, which also advances progress for dropped charts.
- **Seam.** The prior art is the `skip_unreachable_clips` argument on the clipped walk, plus the partition window's `#[cfg(test)]` knobs. Production `bake_chart` reaches the walk through `bake_light_layer_chart_controlled`, which hardcodes that argument, so the knob needs a path through it.
  - The cull-free "off" side can be `Fixture::replay` or the knob.
  - Verify can reuse `segment_clear_remembering` with copies of both caches.
- **Margin.** See research §Margin scale. `ray_traversal::prune_limit` shares the form only; do not copy its constants.
- **Hull containment.** Derivation is in `research.md` §Hull containment. A sampler change that moves a ray origin, target, clip or lead-in segment must re-check it.
- **Rival.** The strongest alternative works per (light, chart): rasterise the texels each nearby triangle could shadow into a mask, modelled on `ClipReach` (`texel_footprint/clip_reach.rs`), and skip every texel outside it. It loses. Tiles already catch nearly every lit texel, so it saves little more, and its exactness proof is much harder.
- **Spot cone precedent.** `affinity_grid.rs::direct_light_may_reach_cell` is an existing conservative sphere-against-cone test, with the inside-sphere and non-finite guards. Reuse it if its domain and tolerance fit; otherwise say why in the plan of record.
- **One production walk.** `bake_layered_section_controlled` has only test callers. prl-build bakes through `bake_fused_windowed`.
- **First slice.** The chart cull with its parity fixtures. It is small, and it builds the culls-off seam the shaft fixtures reuse.
- **File size.** `lightmap_layer.rs` is past 800 lines. Split the clipped chart walk out first, behavior-preserving, in its own commit.
- **Docs at landing.** `build_pipeline.md` step 12 gains both skips and their exactness standard. Tick the roadmap's "lightmap exact skips" entry.

## Open questions

None.
