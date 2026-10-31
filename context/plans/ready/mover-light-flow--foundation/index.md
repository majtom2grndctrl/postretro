# mover-light-flow--foundation

Brief · compact · Lighting and GI · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/rendering_pipeline.md` §4 · `context/research/gi-rtx-lite/index.md` §Build order, §Door classification · read at 56ec2e6

## Problem

Owner-requested groundwork for theatrical movers (a sliding wall, a room rising from the floor). Baked light is computed with every mover absent (`build_pipeline.md` id 43: mover brushes are not in lightmaps, SDF or probes). So a closed mover leaks baked light, and a mover at its other end of travel casts no baked shadow. The fix is per-state visibility masks for gated lights, blended by mover state (research build steps 5–7, plus step 12's mover rest-pose channel). It is unsized: nobody knows how many lights each mover gates, how many texels and probes change, or what masks would cost. When this is done, every compile prints a mover-lighting line in its Build Summary, and `-v` lists, for each mover at each end of its travel, the baked lights it blocks. The report serves the engine owner deciding step 5, and a mapper seeing where light leaks through a closed mover today. Nothing about the shipped level changes.

## Decisions

- **Compile-only.** No PRL section, loader, runtime or shading change. Portal-gated light reach, the door influence graph and the research's light → cell reach table are split out. Reach defined by lit texels misses entity receivers lit only through direct SH, so a runtime filter built on it could drop character shadows. Research step 3 owns redefining reach before any runtime consumer.
- **Unit is (mover, terminus).** Termini are the first and last waypoint of the mover's path. Doors, the sliding wall and the rising room share this one mechanism.
  - A pose at terminus k is the authored brushes translated by (waypoint k − waypoint 0), tested as brushes, not as their hull.
  - Each mover is posed alone, with every other mover absent, as in the bake.
  - Movers with a non-zero spin axis are not analysed: authored or scripted spin makes the parked rotation unknowable at compile time.
  - Intermediate waypoints (`go_to_path_node`) and mid-segment stops (`stop`, command or block policy) are not analysed, because step-5 masks exist only at termini. What this reports is what step 5 would store.
- **Lights analysed.** Static lightmap lights (per-texel visibility from the lightmap walk) and animated-baked lights (per-texel weights, id 25). SDF-shadow lights are not analysed, because neither source carries them. Dynamic-tier lights belong to pool and slab shadows; they appear only in the side count, never in a gated row or the not-analysed list.
- **Gating measure.** A texel counts at a terminus when the bake lit it (visibility or weight above zero) and its segment to the light hits a posed brush.
  - The segment follows the bake's convention: it starts at the texel centre the bake anchored its footprint on, lifted along the normal as the bake lifts it.
  - A lifted centre inside or flush with a posed brush counts (covered means dark).
  - Directional lights use the bake's reversed-direction segment.
  - A light whose position is inside a posed brush is flagged on that row, and all its lit texels count.
  - Counts are estimates, labelled so. One segment misses soft-emitter penumbrae, which biases counts low. At a static edge, a texel lit only by off-centre samples can count without the mover blocking those samples.
- **Probes.** A valid SH probe counts at a terminus when the light reaches it and its segment to the light hits a posed brush. Reaching it means inside the light's influence, inside a spot's cone, and on a segment clear of static geometry. Probes are counted at their centres, because the step-6 door factor they size is stored per probe.
- **Rows, shared and differing.** A row is one (mover, terminus, light) with gated texels or probes. For each (mover, light), texels and probes gated at both termini are shared; those gated at one are differing. Only differing ones need a state mask. The shared set is a candidate for a darker base bake, not a guarantee, because travel between termini can uncover a texel.
- **Size estimate, labelled *est.*:** 1 B per gated texel plus 8 B per touched (row, lightmap cell block). It is given both ways: every gated texel, and differing texels only. It is compared against the same PRL's packed lightmap-family bytes (ids 22, 25, 42), as a proxy for the camera-dependent residency mandatory set (`rendering_pipeline.md` §4). The percentage prints as n/a when that total is zero.
- **Side count.** For each mover, the dynamic lights whose influence reaches its swept bounds (every waypoint's pose), in two groups: carried by this mover, and all others. Slab shadows (research step 7) price them differently, since a carried light moves with its occluder.
- **Report.** Never `warn`, because a leak through a closed mover is today's normal.
  - **Build Summary block**, always printed: movers analysed and not analysed, rows, gated texels and probes (shared and differing), side-count totals, the estimate both ways, and the percentage.
  - **`info` (`-v`)**: one line per (mover, terminus) with rows. It names the mover by `name`, else its brush-bounds centre, and its gated lights by origin, at most four, then "and N more".
  - **`debug`**: each row, with texel and probe counts (shared and differing), the inside flag and the estimate both ways. Also each mover's side count and each not-analysed reason.
  - Order is by `mover_id`, then terminus (first, last), then light in authored entity order across both tiers. The text is identical across worker counts and light-window sizes.
- **Cache.** Gating results (per-row texel, probe and touched-block counts) are memoized, keyed on everything that sets gating. Labels, side counts, not-analysed lists and the percentage are recomputed every build from current inputs. Gating never re-traces a partition the per-light layer cache holds; a missing entry re-bakes, as the ShadowmaskAtlas pass does.
- **Non-goals.**
  - The light-door override KVP: nothing consumes it until masks exist.
  - Bounce and door bounce: owner ruling, doors change direct light. Bounce gating waits for the radiance cache's per-light layers and stays outside '451's indirect elements (`ip-check.md`).
  - Combined states of several movers: masks are per mover.
  - A runtime debug view: it needs PRL data.

## Acceptance

Report rows run `prl-build -v`; rows that read per-row counts set `RUST_LOG=debug`.

### Automated
- [ ] `mover-setpiece`: `setpiece_wall` closed has a row for each of the five baked lights other than the shaft light at (992, 320, 288).
  - Arena lights (256, 256, 256), (640, 320, 192) and the spot each gate back-room texels and probes.
  - Back-room lights (240, 1376, 256) and (1000, 1376, 256) each gate arena texels and probes.
  - The shaft light appears in no `setpiece_wall` row.
  - Open, the wall gates pocket texels it covers. Each one whose segment crosses the closed pose is shared; any open-only texel lies within one texel column of the pocket's mouth (pin P1).
- [ ] `mover-setpiece`: `setpiece_room` lowered gates the arena lights' lit texels and valid probes inside the shaft. Raised, it gates arena-floor texels in the room's shadow. Shaft texels gated at both termini are shared.
- [ ] A hollow mover is posed as its brushes: a light between its brushes and touching none is not flagged as inside, and a segment through its doorway that touches no brush is not counted.
- [ ] Each mover is posed alone: a texel whose segment crosses two movers' posed brushes counts in both movers' rows.
- [ ] `mover-setpiece`: `setpiece_room` reports one carried dynamic light (its lamp) and no other; `setpiece_wall` reports none. No dynamic light appears in a row or the not-analysed list.
- [ ] For any pair of terminus gated sets, shared plus differing equals the union, and the differing-only estimate never exceeds the every-gated estimate (property test). A mover whose last waypoint coincides with waypoint 0 reports zero differing and a 0 B differing-only estimate (pin P5).
- [ ] When one row's gated texels fall in a single cell block, its every-gated estimate is the gated texel count in bytes plus 8 B. The percentage divides by the packed sizes of ids 22, 25 and 42 in the same PRL, and prints n/a when they total 0 B.
- [ ] A light whose lit texels and reached probes never meet a mover appears in no row of that mover. A light that lights no texel but whose reached probes are gated appears with zero texels and its probe count.
- [ ] A valid probe behind a posed brush is not counted when it is outside the light's falloff range, outside a spot's outer cone, or behind static geometry on its segment to the light. An invalid probe is never counted.
- [ ] A directional light gated by a mover is counted. An animated baked light gated by a mover is counted from its id-25 weights, ordered among the static lights. Static and animated SDF-shadow lights are listed as not analysed.
- [ ] A mover with a non-zero spin axis and zero spin speed, and a one-waypoint spinner, are each not analysed, with their reason, and the build completes. A mover with a zero spin axis is analysed.
- [ ] A three-waypoint mover whose middle waypoint alone would block a light has no row for that light, and its termini are its first and last waypoints.
- [ ] A baked light inside a posed brush at one terminus only is flagged on that row alone. Its texels lit and not gated at the other terminus are differing (pin P6).
- [ ] Levels with movers and: no baked lights; only SDF-shadow baked lights; only animated baked lights. Each prints the summary with every mover analysed. The animated-only level reports its gated texels and probes (pin P2).
- [ ] The `info` line for each (mover, terminus) with rows names the mover and its gated lights by origin. With more than four, it lists four and ends "and N more", N being the remainder. An unnamed mover is named by its brush-bounds centre.
- [ ] Report text follows the pinned order and is identical across two cold builds and across worker counts. It is also identical across light-window sizes, checked in-process because the window has no CLI flag (pin P4). A (mover, terminus) without rows logs no `info` line. Per-row counts never appear at `info`.
- [ ] No analysis line is logged at warn or above on any fixture these rows compile. The check matches the analysis's subsystem tag, because `mover-setpiece` already warns from ShadowmaskAtlas.
- [ ] Moving one mover's last waypoint: a warm build hits the lightmap-section and ShadowmaskAtlas memos, traces no lightmap visibility, and prints the cold report of the edited map (pin P3). Each of these edits gives a warm report equal to the cold report of the edited build:
  - editing one static light, and moving one animated light (pin P7);
  - moving a dynamic light into a mover's swept bounds;
  - adding an SDF-shadow light, and renaming a mover;
  - toggling `--no-penumbra-floor`, and retexturing one face so its lightmap layers change;
  - toggling `--uncompressed-irradiance`, which also hits the gating memo and changes only the percentage.
- [ ] The packed PRL of `campaign-test` and `mover-setpiece` is byte-identical to the parent commit's. Both build cold (`--no-cache`) with default flags and the same `--jobs`, in an opt-in ignored test that fails when either baseline is missing and reports two maps compared.
- [ ] A level with no movers prints only the Build Summary line, reading zero movers.

### Manual
- [ ] Cold (`--no-cache`) `stress-warren-hallway-inspection` (345 lights, 11 movers), against the parent commit on the same machine class and worker count. The analysis's whole cost adds ≤ 3% to Lightmap Bake wall time, so bake iteration stays fast. Peak compile RAM growth is reported against the same baseline.
- [ ] Report totals recorded in `research.md` for `mover-setpiece`, `stress-warren-showcase`, `kinematic-platform`, `campaign-test` and `stress-warren-hallway-inspection`, with a one-line read on the step-5 gate that names the estimate's biases.

## Path

- **Static visibility:** it exists only inside the fused lightmap walk. `LightmapLayer` partitions per (light, layer) are consumed in global light order by `IncrementalLayerAccumulator::fold_partition` and `FusedShadowmaskPlan::consume_partition`, from `pipeline/lightmap_stage.rs`.
  - A third partition consumer is the likely shape. Alternatively, gate after `AnimatedWeightMaps` and re-read partitions through `LayerPartitions`, as the ShadowmaskAtlas pass does after a section hit.
  - `chart_texel_world_position` gives the anchored centre; `texel_footprint.rs` shows the lift.
  - The walk returns early with no static lights, and runs with no layer lights when every static light is SDF-shadow.
- **Cells:** texel → chart → `BlockLayout.chart_blocks` → `CellBlock.cell_id`.
- **Animated:** finished id-25 weights (`TexelLight`, `AnimatedBlock.lightmap_block`), after `AnimatedWeightMaps`.
- **Movers:** `MapKinematicMover.brush_volumes` (world space, waypoint 0) and `map_data.kinematic_waypoints`; `resolve_kinematic_path` walks the chain and drops the names. Carried links are resolved in `parse.rs` (`resolve_carried_light_links`, `MapData.carried_light_links`).
- **Probes:** validity, range and cone are in `sh_bake.rs` (`probe_grid_layout`, `light_reaches_point`, `spot_cone_parameters`).
- **Build Summary:** printed by `reporter.rs`, with a twin in `tui.rs`; both need the line. Per-section packed bytes are in `pack_output.rs`.
- **Rival shape:** a separate stage that re-traces visibility from scratch. It is simpler to cache, but it duplicates the walk and costs a full lightmap trace.
- **First slice:** the `setpiece_wall` closed rows on `mover-setpiece`. They falsify the texel positions and the posed-brush test before the cache and animated paths.
- `lightmap_layer.rs` and the lightmap stage are large. Add a sibling module.
- **Tests:** compiler integration tests already run `prl-build` as a subprocess and read stderr. The parent-commit PRL precedent is `tests/oversize_lightmap_maps.rs`, which skips silently; this brief's must fail instead.
