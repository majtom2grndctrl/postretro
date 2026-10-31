# mover-light-flow--foundation

Brief · compact · Lighting and GI · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/rendering_pipeline.md` §4 · `context/research/gi-rtx-lite/index.md` §Build order, §Door classification · read at a6a40a1

## Problem

Owner-requested groundwork for theatrical movers (a sliding wall, a room rising from the floor). Baked light is computed with every mover absent (`build_pipeline.md` id 43: mover brushes are not in lightmaps, SDF or probes). So a closed mover leaks baked light, and a mover at its other end of travel casts no baked shadow. The fix is per-state visibility masks for gated lights, blended by mover state (research build steps 5–7, plus step 12's mover rest-pose channel). It is unsized: nobody knows how many lights each mover gates, how many texels and probes change, or what masks would cost. When this is done, every compile reports, for each mover at each end of its travel, which baked lights it blocks and what masks for them would cost. The report serves two readers: the engine owner deciding step 5, and a mapper seeing where light leaks through a closed mover today. Nothing about the shipped level changes.

## Decisions

- **Compile-only.** No PRL section, loader, runtime or shading change. This is research step 2 ("sizes step 5"), with the report as its consumer. Portal-gated light reach, the door influence graph and the research's light → cell reach table are split out. Reach defined by lit texels misses entity receivers lit only through direct SH, so a runtime filter built on it could drop character shadows. Research step 3 owns redefining reach before any runtime consumer.
- **Unit is (mover, terminus), not door.** Termini are the first and last waypoint of the mover's path; the compiler already rejects cyclic paths and non-spinning movers with fewer than two waypoints. This generalizes the research's door classification into the rest-pose channel, so doors, the sliding wall and the rising room share one mechanism. A pose at terminus k is the authored brushes translated by (waypoint k − waypoint 0), posed as brushes, not as their hull. Movers with a non-zero spin axis are not analysed: authored or scripted spin makes the parked rotation unknowable at compile time. Every mover can also be parked mid-travel (`stop` command or policy, `go_to_path_node`). Those poses are not analysed; blending between termini belongs to the mask step.
- **Lights analysed.** Static lightmap lights (per-texel visibility from the lightmap walk) and animated-baked lights (per-texel weights, id 25). SDF-shadow lights are listed as not analysed, because neither carries them. Dynamic-tier lights belong to pool and slab shadows and only appear in the side count.
- **Gating measure.** A texel counts at a terminus when the bake lit it (visibility or weight above zero) and its segment to the light hits a posed brush. The segment follows the bake's own convention: it starts at the texel centre the bake anchored its footprint on, lifted along the normal as the bake lifts it. A lifted centre inside or flush with a posed brush counts (covered means dark). Directional lights use the bake's reversed-direction segment. A light whose position is inside a posed brush is flagged on that row, and all its lit texels count. Texel counts are lower bounds on a mask's region, because one segment misses soft-emitter penumbrae; every count is labelled so.
- **Probes.** A valid SH probe counts at a terminus when the light reaches it: inside its influence, inside a spot's cone, a segment clear of static geometry. It must also have a segment that hits a posed brush. Probes are counted at their centres, because the step-6 door factor they size is stored per probe.
- **Shared versus differing.** For each (mover, light), texels and probes gated at both termini are reported apart from those gated at one. Only the differing ones need a state mask. The shared set is a candidate for a darker base bake, not a guarantee, because travel between termini can uncover a texel.
- **Size estimate, labelled *est.*:** 1 B per gated texel plus 8 B per touched (mover, terminus, light, lightmap cell block), summed per row. It is given both ways: every gated texel, and differing texels only. It is compared against the same PRL's packed lightmap-family bytes (ids 22, 25, 42), as a proxy for the camera-dependent residency mandatory set (`rendering_pipeline.md` §4).
- **Side count.** For each mover: the fixed dynamic lights whose influence reaches its swept bounds, and separately its carried dynamic lights. This sizes slab shadows (research step 7).
- **Report.** It is all `info` or `debug`, never `warn`, because a leak through a closed mover is today's normal. It follows the compiler's info/debug split (draft `compiler-log-hygiene`).
  - `info`: one summary with totals per row, the not-analysed and skipped counts, the estimate both ways, and the percentage.
  - `info`: one line per (mover, terminus) that gates anything. It names the mover and its gated lights (targetname, else origin), capped with "and N more", so a mapper can find the leak.
  - `debug`: per-light counts and skip reasons.
  - Lines are ordered by mover, then terminus (first, last), then light, with one light order across both tiers. They are logged on the compile thread and are identical across worker counts and light-window sizes.
- **Cache.** Only the gating counts are memoized: static and animated each get their own memo, keyed on everything that sets gating. That is the light layer fingerprints and the weight-map output, the mover brushes and waypoints, and the light and probe inputs. Labels, side counts, not-analysed lists and the percentage are recomputed every build from current inputs. On a static memo miss after a lightmap-section hit, gating reads the per-light layer partitions back from the cache. It never re-traces visibility.
- **Non-goals.**
  - The light-door override KVP: nothing consumes it until masks exist.
  - Bounce and door bounce: owner ruling, doors change direct light. Bounce gating waits for the radiance cache's per-light layers and stays outside '451's indirect elements (`ip-check.md`).
  - A runtime debug view: it needs PRL data.

## Acceptance

### Automated
- [ ] `mover-setpiece`: `setpiece_wall` closed has a row for each of the five baked lights other than the one over the shaft at (992, 320, 288), with gated texels on the arena and back-room sides and gated back-room probes. The shaft light appears in no `setpiece_wall` row. At its open terminus the wall gates only the pocket texels it covers, and each of them is also gated closed, so all are shared (pin P1).
- [ ] `mover-setpiece`: `setpiece_room` lowered gates the arena lights' lit texels and valid probes inside the shaft. Raised, it gates arena-floor texels in the room's shadow. Shaft texels gated at both termini are shared, not differing.
- [ ] A hollow mover is posed as its brushes: a light between its brushes and touching none is not flagged as inside, and a segment through its doorway that touches no brush is not counted.
- [ ] `mover-setpiece`: `setpiece_room` reports one carried dynamic light (its lamp), and `setpiece_wall` reports no dynamic light in reach.
- [ ] For any pair of terminus gated sets, shared plus differing equals the union, and the differing-only estimate never exceeds the every-gated estimate (property test). A mover whose last waypoint coincides with waypoint 0 reports zero differing and a 0 B differing-only estimate (pin P5).
- [ ] When one light's gated texels at one terminus fall in a single cell block, that row's every-gated estimate is the gated texel count in bytes plus 8 B. The percentage divides by the packed sizes of ids 22, 25 and 42 in the same PRL.
- [ ] A light whose lit texels and reached probes never meet a mover appears in none of that mover's rows. Two movers gating one light each get their own row. A light that lights no texel but whose reached probes are gated appears with zero texels and its probe count.
- [ ] A directional light gated by a mover is counted. An animated baked light gated by a mover is counted from its id-25 weights in the same row as static lights. Static and animated SDF-shadow lights are listed as not analysed.
- [ ] A mover with a non-zero spin axis is reported as not analysed, with its reason, and the build completes.
- [ ] A baked light inside a posed brush at one terminus only is flagged on that row alone. Its texels lit but not gated at the other terminus are differing (pin P6).
- [ ] A level with movers and no baked lights, and a level whose only baked lights are SDF-shadow, each print the summary with movers analysed and zero gated rows (pin P2).
- [ ] Report lines follow the pinned order, are captured on the compile thread, and are identical across two cold builds, across worker counts and across light-window sizes (pin P4). A (mover, terminus) that gates nothing logs no info line. Per-light counts never appear at info.
- [ ] No analysis line is logged at warn or above on any fixture these rows compile. The check matches the analysis's subsystem tag, because `mover-setpiece` already warns from ShadowmaskAtlas.
- [ ] Moving one mover's last waypoint: a warm build hits the lightmap-section and ShadowmaskAtlas memos, traces no lightmap visibility, and prints the cold report of the edited map (pin P3). Editing one light, moving a fixed dynamic light into a mover's swept bounds, adding an SDF-shadow light, renaming a mover: each warm report equals the cold report of the edited map.
- [ ] The packed PRL of `campaign-test` and `mover-setpiece` is byte-identical to the parent commit's.
- [ ] A level with no movers prints only the summary line, reading zero movers.

### Manual
- [ ] Cold (`--no-cache`) `stress-warren-hallway-inspection`, against the parent commit on the same machine class and worker count. The analysis's whole cost (walk consumer, animated step, probe pass) adds ≤ 3% to Lightmap Bake wall time, so bake iteration stays fast. Peak compile RAM growth is reported against the same baseline.
- [ ] Report totals recorded in `research.md` for `mover-setpiece`, `stress-warren-showcase`, `kinematic-platform`, `campaign-test` and `stress-warren-hallway-inspection`, with a one-line read on the step-5 gate that states the counts are lower bounds.

## Path

- **Static visibility:** it exists only inside the fused lightmap walk. `LightmapLayer` partitions per (light, layer) are consumed in global light order by `IncrementalLayerAccumulator::fold_partition` and `FusedShadowmaskPlan::consume_partition`, from `pipeline/lightmap_stage.rs`. A third partition consumer is the likely shape. `chart_texel_world_position` gives the anchored centre; `texel_footprint.rs` shows the lift.
- **Cells and early returns:** texel → chart → `BlockLayout.chart_blocks` → `CellBlock.cell_id` resolves cells. The walk returns before any consumer when there are no static lights. The ShadowmaskAtlas memo, resolved before the walk and re-reading only the partitions it needs, is the replay pattern.
- **Animated:** gating comes from the finished id-25 weights (`TexelLight`, `AnimatedBlock.lightmap_block`), after `AnimatedWeightMaps`.
- **Movers:** `MapKinematicMover.brush_volumes` (world space, waypoint 0) and `map_data.kinematic_waypoints`. `resolve_kinematic_path` walks the chain and drops the names. Carried lights are linked in `kinematic_geometry.rs`.
- **Probes:** positions and validity are in `sh_bake.rs`; spot cones are in `spot_cone_parameters`.
- **Rival shape:** a separate stage that re-traces visibility from scratch. It is simpler to cache, but it duplicates the walk and costs a full lightmap trace.
- **First slice:** the `setpiece_wall` closed row on `mover-setpiece`. It falsifies the texel positions and the posed-brush test before the cache and animated paths.
- `lightmap_layer.rs` and the lightmap stage are large. Add a sibling module instead of extending them.
- **Test harness:** compiler integration tests already run `prl-build` as a subprocess and read stderr. In-process log capture sees only the compile thread.
