# mover-light-flow--foundation

Brief · compact · Lighting and GI · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/rendering_pipeline.md` §4 · `context/research/gi-rtx-lite/index.md` §Build order, §Door classification · read at e3b0772

## Problem

Owner-requested groundwork for theatrical movers (a sliding wall, a room rising from the floor). Baked light is computed with every mover absent (`build_pipeline.md` id 43: mover brushes are not in lightmaps, SDF or probes), so a closed mover leaks baked light and a mover at its other end of travel casts no baked shadow. The fix — per-state visibility masks for gated lights, blended by mover state (research build steps 5–7, plus the mover rest-pose channel of step 12) — is unsized: nobody knows how many lights each mover gates, how many texels change, or what masks would cost. When this is done, every compile reports, for each mover at each end of its travel, which baked lights it blocks, how many lit texels and SH probes change, which cells it darkens fully, and an estimated mask size against the level's lightmap bytes. Rows for the authored pose double as a leak report. Nothing about the shipped level changes.

## Decisions

- **Compile-only.** No PRL section, loader, runtime or shading change. This is research step 2 ("sizes step 5"), with the report as its consumer. Runtime consumers come later: portal-gated light reach, masks, the direct-SH door factor, slab shadows. A runtime promotion-ranking filter was considered and deferred: it needs light reach for entity receivers (direct SH), which this analysis does not measure. Filtering on texel reach alone could drop shadows from characters in cells where the light touches no surface.
- **Narrower than research step 2.** Step 2 also builds a door influence graph and a light → cell reach table. Both are split out, because the research defines reach by lit texels, and that definition misses entity receivers. The later brief that builds portal-gated reach (research step 3) owns redefining reach to cover direct-SH receivers, and the influence graph lands with its first consumer.
- **Unit is (mover, terminus), not door.** Termini are the first and last waypoint of a finite, non-cyclic path of at least two waypoints. This generalizes the research's door classification into the rest-pose channel, so doors, the sliding wall and the rising room share one mechanism. Rows whose authored pose seals portals (`sealed_portal_ids_for_mover`) are marked as doors for continuity with `E17--doors-as-occluders`. Intermediate waypoints are not rest poses here.
- **Pose = authored brushes translated by (waypoint k − waypoint 0).** The compiler poses movers only at waypoint 0 today, so this is new code. Movers with spin are not analysed: the parked rotation is unknown at compile time. Movers with one waypoint or a cyclic chain are not analysed either. Each such mover is reported with its reason.
- **Lights analysed.** Static lightmap lights (per-texel visibility from the lightmap walk) and animated-baked lights (per-texel weights, id 25). Dynamic-tier lights are excluded, because pool and slab shadows own them. SDF-shadow lights are reported as not analysed, because they are absent from the lightmap layers.
- **Gating measure.** A texel counts when the light lights it (visibility or weight above zero) and its single segment to the light hits the posed mover. Directional lights use a segment along the reversed light direction to the world bounds. A single segment is enough for sizing; mask fidelity and soft edges belong to the mask step. A valid SH probe counts when it lies inside the light's influence and its segment to the light hits the posed mover; this sizes the direct-SH door factor (step 6). A cell is *fully gated* when every texel the light lights in it counts. The single-segment count is a **lower bound** on the region a mask must cover: it misses the soft fringe that the penumbra floor guarantees. The report labels it so, and gives a fringe estimate: texels within one penumbra-floor footprint of a counted texel.
- **Shared versus differing texels.** For each (mover, light), texels gated at every terminus are reported apart from texels whose gating differs between termini. Only the differing ones need a state mask. Being gated at every terminus is necessary, not sufficient, for being always dark, because travel between termini can uncover a texel. So the shared set is a candidate for a darker base bake, not a guarantee. The size estimate is given both ways: masks for every gated texel, and masks for differing texels only.
- **Side counts, nearly free from the mover-to-light cull.** Dynamic-tier lights whose influence reaches each mover's swept bounds; this sizes slab shadows (research step 7). Movers with more than two waypoints are flagged, because `go_to_path_node` or a waypoint wait can park them at an intermediate pose this analysis does not cover.
- **Size estimate, pinned and labelled *est.*:** 1 B per changed texel plus 8 B per touched (mover, terminus, light, lightmap cell block). It is compared against the level's lightmap-family bytes (ids 22, 25, 42) rather than the residency mandatory set, which depends on where the camera is (`rendering_pipeline.md` §4). The research gate (masks ≤ ~10% of the mandatory set) is checked against this proxy, and the report says so.
- **Report.** Follows the compiler's info/debug split (draft `compiler-log-hygiene`):
  - one `info` summary per build: movers analysed and skipped, gated (mover, terminus, light) rows, changed texels (shared and differing), probes, est. KiB both ways, % of lightmap-family bytes;
  - one `info` line per (mover, terminus) that gates anything;
  - per-light detail at `debug`;
  - no `warn`, because leaks through closed movers are today's normal.
- **Determinism and cache.** Report text is identical across cold builds. The analysis keeps its own memo, keyed on the lightmap layer fingerprints, the animated weight-map output, the mover brushes and waypoints, and the probe set, so a warm build replays the report. A `lightmap_section` memo hit skips reading layers, so the report cannot ride on that cache entry (`build_pipeline.md` §Build Cache; the `shadowmask_atlas` memo is the pattern).
- **Non-goals.** The light-door override KVP: its name and default are an open owner decision in the research, and nothing consumes it until masks exist. Bounce and door bounce. A runtime debug view of gating: it needs PRL data. Camera occlusion at the far terminus.

## Acceptance

### Automated
- [ ] `mover-setpiece`: `setpiece_wall` at its closed terminus gates every baked light whose lit texels lie on the far side of the opening, with changed texels on the arena side and the back-room side. At its open terminus (in the pocket) it gates zero texels.
- [ ] `mover-setpiece`: `setpiece_room` lowered gates the arena lights' lit texels inside the shaft, and fully gates the shaft-floor cells. Raised, it gates arena-floor texels in the room's shadow. Shaft-floor texels gated at both termini are reported as shared, not differing.
- [ ] A mover whose two termini gate disjoint texel sets reports zero shared texels. The differing-only estimate is never larger than the every-gated-texel estimate.
- [ ] A mover with three or more waypoints is flagged as able to park at an intermediate pose. Each mover's row reports how many dynamic lights reach its swept bounds.
- [ ] A light whose lit texels never cross a mover appears in none of that mover's rows. Two movers gating one light each get their own row, independent of the other.
- [ ] A directional light gated by a mover is counted. An SDF-shadow light and a dynamic light are listed as not analysed, with the reason.
- [ ] A spinning mover, a one-waypoint mover and a cyclic-path mover are each reported as not analysed, with the reason. None aborts the build.
- [ ] A baked light inside a mover's posed hull at a terminus is flagged in its row and counts every lit texel in reach. No panic.
- [ ] Two cold builds produce identical report text. A no-edit warm build replays it from the memo. Moving one mover's last waypoint re-runs the analysis without re-baking lightmaps.
- [ ] The packed PRL is byte-identical with and without the analysis on `campaign-test` and `mover-setpiece`.
- [ ] A level with no movers prints only the summary line, reading zero movers.
- [ ] Cold `stress-warren-hallway-inspection`: the analysis adds ≤ 3% to Lightmap Bake wall time, measured and reported. Peak compile RAM growth is reported.

### Manual
- [ ] Report totals recorded in `research.md` for `mover-setpiece`, `stress-warren-showcase`, `kinematic-platform`, `campaign-test` and `stress-warren-hallway-inspection`. Each records movers analysed, gated rows, changed texels (shared and differing), fringe estimate, probes, dynamic lights in reach, est. KiB both ways and % of lightmap-family bytes, with a one-line read on the step-5 gate. The read states that counts are lower bounds.

## Path

- **Static visibility** exists only inside the fused lightmap walk: `LightmapLayer` partitions per (light, layer), consumed in global light order by `IncrementalLayerAccumulator::fold_partition` and `FusedShadowmaskPlan::consume_partition`, orchestrated from `pipeline/lightmap_stage.rs`. Texel → chart → `BlockLayout.chart_blocks` → `CellBlock.cell_id` resolves cells. Texel world positions come from the chart frames the bake already reconstructs. A third partition consumer beside the shadowmask plan is the likely shape.
- **Animated reach** comes from the finished id-25 weights (`TexelLight`, `AnimatedBlock.lightmap_block` → id-22 block → cell), in a step after `AnimatedWeightMaps`.
- **Mover data:** `MapKinematicMover.brush_volumes` (world space, waypoint 0) and `map_data.kinematic_waypoints` (`resolve_kinematic_path` walks the chain but drops the names). The posed-hull segment test is new; cull by the posed AABB against each light's influence sphere first.
- **Probes:** SH probe positions and validity from the SH bake inputs (`sh_bake.rs`).
- **Rival shape:** a separate stage that re-traces visibility from scratch. It is simpler to cache, but it duplicates the walk's visibility and costs a full lightmap trace.
- **First slice:** the `setpiece_wall` closed-terminus row on `mover-setpiece` alone. It falsifies the texel-position reconstruction and the posed-hull test before the cache and animated paths.
- `lightmap_layer.rs` and the lightmap stage are large; add a sibling module rather than extending them.

## Open questions

- How a texel's world position is reconstructed for the segment, and whether the chart-plane drift (`drafts/lightmap-chart-plane-drift`) matters at segment scale — **delegated**
- Whether counted probes include the four-probe interpolation footprint or only probe centres — **delegated**; report the choice
