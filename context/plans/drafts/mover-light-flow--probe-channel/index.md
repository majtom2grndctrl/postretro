# mover-light-flow--probe-channel

Brief · resumable · Lighting and GI · reads: `context/lib/rendering_pipeline.md` §4 (Baked direct for dynamic receivers, Promoted static lights, Promoted animated lights, Sampled-row compose) · `context/lib/build_pipeline.md` §Compiler pipeline step 12, §PRL section IDs, §Build Cache · `context/lib/networking.md` §Presentation events vs. replicated state · `context/research/gi-rtx-lite/index.md` §Build order steps 5–6 · `context/research/gi-rtx-lite/ip-check.md` §Piece 5, §Piece 7, §For door and mover light flow, §Bake position against '451 · read at fd8cf76 (indirect half added 2026-11-03 at 33ac590)

## Problem

Owner-observed defect on `mover-setpiece`, brief 1 of 2 in mover light flow. Every probe bake runs with every mover absent (id 43): the direct-SH bakes movers and entities light themselves from (ids 35, 41, 45), and the SH base (id 34) every surface takes its static bounce from. The compose has no input from mover state. So the risen room's interior is lit by the arena's baked lights as if its walls weren't there. An entity behind the closed sliding wall stays lit too. And a dark room sealed by a closed door glows with the bright room's bounce near the doorway, as if the door were open: the monster-closet case the product makes first-class (`context/lib/index.md` §1). When this is done, each gated baked or animated-baked light's direct term reaches mover and entity receivers only where it would with the mover in place, and static bounce at probes near a mover matches a bake with the mover in place. Both ramp with the mover's position along its path, and back on reversal. Promotion still hands a gated light off without a pop.

## Decisions

- **Two halves, one mover-state input.** A direct half (per light, below) and an indirect half (per mover, below) share the blend scalar, the ramp keys, the section and the compose trigger. Owner added the indirect half 2026-11-03 (option A of the session), after confirming the sealed dark room leaks bounce.
- **Commitments this reverses (owner, 2026-11-03).** The indirect half overturns recorded rulings: the hybrid stack's premise that doors change direct light only (`gi-rtx-lite/index.md` §Hybrid stack), the foundation's non-goal "Bounce and door bounce: owner ruling, doors change direct light" (`plans/done/mover-light-flow--foundation/`), and the research's open decision on whether door bounce is worth its bytes after the door channel (§Open owner decisions), which this settles by taking static door bounce ahead of brief 2. The rest of the hybrid stack stands. Summing per-mover bounce differences is the accepted approximation for doors in series (research §Diagnosis), so redesign gate item 1, bounce that combines correctly across several doors (§When the redesign earns its place), stays not required.
- **Driven by mover state, never by light state.** Neither half changes when a light changes; a recompute fires only on a mover-state change. Any compose gate is framed as residency or visibility (`ip-check.md` §Piece 5 design-arounds, §For door and mover light flow). The channel stores per-light visibility and per-mover absolute bounce states, and never swaps in an ambient or AO layer (§Piece 7). On the 2026-11-03 claim chart and boundary report, '451 claim 1 needs a light-source change, (a), and a coefficient of the changed light, (b); both halves lack both, because their scalar is occluder state and their lights never change (`ip-check.md` §Bake position against '451, rows "mover state channels" and 11b). The indirect half sums only static lights, none of which carries a curve, so the "never on a light that also carries a curve" rule holds by construction. Engineering reading for counsel.
- **Direct half — representation: one scalar per state.** Per (mover, terminus, light, valid probe) the section stores q_k: the fraction of the light's baked soft visibility there that survives with the mover posed at terminus k, from the bake's own samples and seed. Warrant: `bake_probe_direct_rgb` stores one light at one probe as a single lobe × radiance × one scalar visibility, so the terminus state is exactly S_k = q_k × T (T being that light's per-probe tile), and a tile per state would only repeat T. Rival and byte estimates: `research.md`.
- **Direct half — runtime blend.** Compose builds each gated light's term as lerp(S₀, S₁, t′), a lerp of two stored terminus states of a light that does not change, and puts it in place of the light's mover-absent term. Algebraically this equals base + (lerp(q₀, q₁, t′) − 1) × T. The IP footing is not the formula's shape but that the scalar is occluder state, not a light-source change (above). T comes from the light's id-41 or id-45 entry at that cell when one exists, else the section carries it. With q ≡ 1 there is no residue: the atlas equals today's.
- **Several movers on one light.** Each mover is still baked alone (`plans/done/mover-light-flow--foundation/`). Where several movers gate one light at one probe, compose multiplies their lerped visibilities. The result is never brighter than any single mover leaves it, and never negative.
- **Indirect half — what is stored.** Per (mover, terminus k, affected valid probe), the absolute static bounce S^b_k: id 34's own content (every static light's bounce one plus `_bounce_fill` × later bounces, from the surface radiance cache) re-baked with that mover's brushes posed at terminus k, every other mover absent. Absolute, not a difference, at each light's fixed baked intensity. Animated lights' bounce (id 27) is not in it.
  - **Bake.** Posed brushes join the occluders for patch direct, for the later bounces and for the probe gather (`surface_radiance/stage.rs` `bake_sh_base` and its layers). Whether the posed brushes also become patches with their material albedo is **delegated**; the reference-bake row below judges it.
  - **Affected probes.** A probe stores entries when its re-baked value at either terminus differs from id 34's by more than id 34's own encode tolerance. The domain is derived, never sampled: probes whose gather can see a patch whose radiance changes at either pose, or whose rays cross the posed brushes. Its exact construction is **delegated**.
  - **Lifetime.** The patch cache now lives past the id-34 gather through the mover bounce bake, still freed before atlas preparation, so no pre-cut face identity crosses the cut. This diverges from `bake_sh_base`'s free-at-gather-end. The resident cache plus per-pose shading is charged to the SH memory gate before any ray; overage is the named error, never coarsening (`development_guide.md` §1.4).
- **Indirect half — runtime blend.** For one mover at a probe, the composed base is lerp(S^b_0, S^b_1, t′) in place of id 34. Where several movers affect one probe, compose adds each mover's lerped state minus id 34 to id 34; each mover is baked alone, as the foundation's movers are. Evaluated irradiance never goes negative in any direction. Id-27 deltas add on top as today.
- **Promotion.** The gated term lives inside the baked arm: a mover or mesh receives (1 − w) × lerp(S₀, S₁, t) + w × the runtime term, animated lights scaled by their curve. The runtime arm is unchanged, because promoted and dynamic slots already render movers as casters (`RigidOccluderDepthPass::record_kinematic_movers`, gated by `entity_occluder_eligible`). No receiver sums a light twice (`rendering_pipeline.md` §4).
- **Lights and movers in scope.** Gated lights are id 35's static lightmap lights, directional included, and id 45's animated-baked lights; SDF-shadow lights are absent from id 35, so nothing gates them. The unit is (mover, terminus), termini only, over the movers the foundation analyses (no spinners).
- **Blend scalar t.** t is the mover's arc-length fraction along its path: 0 at the first waypoint, 1 at the last. Intermediate waypoints are crossed linearly. Each peer derives t from the final post-tick phase the blocked-portal rebuild reads; phase already replicates (`WireKinematicMoverState`), so there is no wire change.
- **Author ramp (owner, 2026-11-03).** Two `kinematic_mover` keys reshape t into the blend input t′ = clamp((t − s) / (e − s), 0, 1): `light_ramp_start` (s, default 0) and `light_ramp_end` (e, default 1). Defaults give t′ = t. Example: a rising room whose doorway crests the floor at 60% of its rise sets s = 0.6. The keys are content, so every peer reads the same values; the game applies the remap before handing state to the renderer, so brief 2 inherits it. Parse rejects anything outside 0 ≤ s < e ≤ 1 with a compile error naming the mover. The ramp is keyed to position, never to a timer (owner chose position over a timed fade, 2026-11-03). A script method to change the ramp at runtime is deferred until a map needs one. Key names are a proposal; the owner confirms them at review.
- **Layers.** Physics derives t, since the path coordinate is private there (`path_coordinate`, `position_for_phase`). Game logic hands the renderer one (mover id, t) per present mover each frame, apart from the cull-split draw instances and naming no SH concept, because brief 2 reuses it unchanged; a mover with no supplied state is ungated. The renderer maps ids to section entries and owns every GPU write.
- **PRL section.** A new optional section takes the next free id. Wire format and boundary names are below.
  - Absent, the frame renders today's output. Malformed, or naming a mover or light that does not resolve, it is a load error.
  - It is whole-resident, outside id 50, as ids 47/48 are. Streaming it later is additive.
  - Its cells, direct and indirect, count as grid-matched delta entries for base storage levels (`sh_density::storage_level_ceilings`). So ids 34/35 pack finer under gated and affected probes. A map with no movers, or whose movers gate no baked light, packs byte-identical.
- **Cache.** The direct half uses a per-light memo shaped like `mover_light_probes`. A mover-only edit re-bakes only the lights that reach its old or new pose. The indirect half uses a per-mover memo keyed on its posed brushes at both termini, every static light's layer digest, the geometry hash, the probe grid, `_bounce_fill` and `TRACER_EPOCH`. A mover-only edit re-bakes only that mover's bounce states; a static-light edit re-bakes every mover's, since each state sums all static lights. The mover-absent bakes behind ids 34, 35, 41 and 45 never re-trace on a mover edit; only the storage levels under changed cells repack.
- **Per-frame cost.** Compose recomposes channel rows only when a mover's t changes, and then only the rows its entries cover. The budget is ≤ 0.2 ms per travelling mover on the 5300M (research step 5's gate). The game's t pass allocates nothing per frame.
- **Non-goals.**
  - World-surface masks (lightmap texels, shadowmask, id-25 weights) belong to brief 2. It consumes this brief's mover-state input.
  - Animated lights' bounce (id 27) behind a mover: an alarm light behind a closed door still leaks bounce. A per-light bounce state for a curve-driven light is the risky '451 shape (boundary report item 5), so it waits on counsel's answer on id 27.
  - Per-light bounce states for static lights: rejected. About 200× the indirect half's bytes for no visible gain, because static lights never change.
  - Light → cell reach, portal-gated runtime lights and fog reach.
  - Slab shadows.
  - Spinners, and intermediate waypoints as states.
  - A script-side binding to mover progress.
  - Fog: it samples no direct SH.
  - Scatter billboards (ids 47/48) sample no direct SH, so a scatter billboard behind the closed wall stays lit (owner, 2026-11-03). A roadmap follow-up beside brief 2 owns it.

## Acceptance

### Automated
Bake and format:
- [ ] Each stored q_k equals the bake's soft visibility for that probe and light, with the posed brushes added to its occluders, divided by its baked visibility, under the same seed (unit test against the direct-SH probe bake). A light whose samples never meet the posed mover stores no entry for that mover.
- [ ] `mover-setpiece`: `setpiece_wall` closed stores q < 1 at back-room probes for both arena point lights and the spot, and at arena probes for both back-room lights. It stores no entry for the shaft light (992, 320, 288). `setpiece_room` raised stores q < 1 at its interior probes for the shaft light.
- [ ] Dark-closet fixture (new, below): with the door closed, the composed base at closet probes valid in both bakes (S^b_0, or id 34 where a probe stores no entry) equals id 34 of a reference build where the door is a static brush at its closed pose, within id 34's encode tolerance. With the door open, S^b_1 equals today's id 34 within the same tolerance. The bright room's probes beyond the doorway's reach store no entry.
- [ ] `mover-setpiece`: `setpiece_room` raised stores indirect entries at its interior probes, each no brighter than today's id 34 there. A mover whose poses change no probe beyond tolerance stores no indirect entry.
- [ ] On the dark-closet fixture and `mover-setpiece`, a test-only re-bake of every valid probe at each terminus differs from id 34 beyond tolerance only at probes the derived domain admits.
- [ ] `mover-setpiece`: every id-34 and id-35 cell under a probe with a direct or bounce entry stores no coarser than a grid-matched delta entry, and cells elsewhere keep the parent build's levels.
- [ ] A directional light a mover blocks stores q < 1 under the posed brushes, and a spinner stores no entry.
- [ ] Adding an animated light to the dark-closet fixture's bright room leaves every bounce-state byte unchanged; its id-27 deltas compose unchanged at every t′.
- [ ] Section bytes are identical across `-j 1` and `-j 4` and across two cold builds.
- [ ] The SH gate's prediction covers the measured peak of the mover bounce bake on every fixture, counting the resident patch cache, per-pose shading, the accumulated bounce states and the memo writes. A map whose prediction exceeds the budget fails with the SH gate's named error before any bounce ray, at the same probe spacing.
- [ ] A cold build drops the patch cache exactly once, after the mover bounce bake and before atlas preparation, and the SH gate's prediction covers the measured peak of every stage it outlives (P21).
- [ ] Moving one mover's last waypoint: on a warm build, the bakes behind ids 34, 35, 41 and 45 trace nothing, only lights reaching the old or new pose re-bake direct entries, only that mover's bounce states re-bake, and the PRL equals the cold PRL of the edited map. Editing one static light's colour re-bakes every mover's bounce states and no direct entry of another light; the PRL equals cold.
- [ ] A warm mover-only edit whose id-34 volume hits still re-bakes that mover's bounce states equal to cold, within the SH gate's prediction (P20).
- [ ] Over one cache, a mover edit, then a static-light edit, then reverting both each build warm equal to cold, and the revert hits every direct, bounce and id-34 memo (P19).
- [ ] A map with no movers, and a map whose movers gate no baked light, emit no section and pack byte-identical to the parent commit's PRL. This is an opt-in ignored test that fails on a missing baseline, after the foundation's parity test.
- [ ] A malformed section, one naming a mover absent from id 43, a light outside its namespace, or an affinity grid unlike id 35's, each fail the load with a message naming the section.
- [ ] A grep gate keeps every SH, probe and section type out of the game-side mover-state collector and the renderer setter's signature. The closet fixture's bounce-only section and a direct-only section each load and compose.

Blend scalar:
- [ ] t reads 0 docked at the first waypoint and 1 at the last. It reads the arc-length fraction mid-travel, and that fraction at the middle waypoint of a three-waypoint path whose two segments differ in length. A wait or a blocked stop holds t.
- [ ] With default ramp keys, t′ = t at every sample above. With s = 0.6, e = 0.9: t′ is 0 at t = 0.6 and below, 1 at t = 0.9 and above, and 0.5 at t = 0.75, both rising and after a reversal. Parse accepts s = 0, e = 1 and s = 0.6, e = 0.9; it rejects s = e, s > e, s < 0, e > 1, s = nan, a non-numeric value, and e = 0 with s left at its default, each with an error naming the mover.
- [ ] A reversal mid-travel, in either direction, continues from the current t with no jump.
- [ ] Two movers advancing on one tick each report their own t for that frame.
- [ ] A mover that reaches its last waypoint on tick N reads t = 1 in frame N's state, the same frame its blocked-portal state changes.
- [ ] Host and client derive equal t from one predicted phase at a terminus, mid-travel and after a reversal, as `client_predicted_mover_phase_matches_host_door_occlusion_phase` does for the docked predicate.
- [ ] A door an auto-close drove home reads t = 0 docked, and a completed `once` mover reads t = 1 at its last waypoint, though neither has a segment left to traverse; neither is handed over as no state (P9).
- [ ] A mover that reaches its first waypoint on tick N reads t = 0 in frame N's state, and its sealed portals block from tick N + 1; no frame has them blocked while its t is above 0 (P11).
- [ ] Two frames presented between one pair of ticks hand the renderer the same t, and the second recomposes no channel row. A frame that runs two ticks composes only the second tick's t (P10).
- [ ] After a relevel whose new level reuses a mover id, and after a restart that re-seeds a mover, the first composed frame (held settling frames included) uses the new seeded t, never the old level's or the pre-restart value (P13).
- [ ] A client whose mover phase arrives mid-travel in a snapshot, with no local tick, composes that snapshot's t the frame it applies; a reconciliation that moves the phase back recomposes at the corrected t (P14).
- [ ] A mover outside the frame's drawn cells still hands the renderer its t, and a receiver it gates composes gated (P15).
- [ ] A mover whose path has zero length, or one degenerate interior segment, hands over a finite t, and no composed texel is NaN (P23).
- [ ] A `start_on_spawn` mover's first composed frame, before its first tick, is gated at t = 0 from its seeded phase (P4).
- [ ] A mover with s = 0.6 at t = 0.6 hands the renderer 0, and the gated light composes S₀.
- [ ] After warm-up, a frame's t pass over two travelling movers allocates nothing (allocation probe).

Compose (headless GPU, `POSTRETRO_REQUIRE_GPU`):
- [ ] At a probe gated only at terminus 0, the gated light's composed term is S₀ at t = 0, ungated at t = 1, and their exact linear mix at t = 0.5. A second light that the mover does not gate at that probe is unchanged at every t.
- [ ] Stripping the section from a PRL composes the same atlases as supplying no mover state, and both equal today's: direct and indirect.
- [ ] Indirect: at a probe one mover affects, the composed base is S^b_0 at t′ = 0, S^b_1 at t′ = 1, and their exact mix at t′ = 0.5; id-27 deltas add unchanged at every t′. Two movers affecting one probe compose id 34 plus both differences, and no direction's irradiance goes negative. A probe no mover affects is id 34 at every t′.
- [ ] Two movers gating one light at one probe compose their product. Neither alone is brighter than the pair, and no texel goes negative.
- [ ] A gated animated-baked light composes its curve × its gated term at every t. An SDF-shadow static light has no entry and composes as today.
- [ ] A promoted gated static light contributes (1 − w) × its gated term at every t, and nothing at w = 1. A promoted gated animated light also carries its curve.
- [ ] A frame where no mover's t changed and no other trigger fired recomposes no channel row. A changing t recomposes only rows its entries cover. A row that enters residency or the view gate while a mover travels composes with that frame's t before any consumer samples it.
- [ ] Streaming `off` and `sync-proof` compose identical channel output.
- [ ] A t change on a frame that records no compose, or whose encode fails, recomposes its rows on the next recording frame though t no longer changes, on the streamed planner and on the legacy compose (P12).
- [ ] With two movers, one held and one travelling, only rows carrying the travelling mover's entries recompose, and a shared row composes the held mover's unchanged t (P16).
- [ ] A slot re-tenanted the frame its row is evicted, while a mover travels, composes the new row's own channel entries at that frame's t, and today's term where it has none (P17).
- [ ] A travelling mover recomposes only the passes its entries feed: bounce entries the indirect pass, id-45 entries Pass B, static entries Pass A and then Pass B at those rows (P18).
- [ ] A gated light with no surviving id-41 or id-45 entry at a cell (not selected, or dropped by the exact-zero policy) carries its own tile there and composes S₀ at t′ = 0 as an id-41-backed light does (P22).
- [ ] Toggling the baked-direct light-term mask while a mover travels recomposes every resident channel row at that frame's t (P8).

### Manual
- [ ] Dark-closet fixture, door closed (owner judges the outcome; today's build and the static-wall reference render shown beside it): the closet reads dark. The closet floor and walls near the doorway show no bright-room glow, and the door's closet face reads dark. Opening the door ramps the glow in with its travel; closing ramps it out. This bar is not relaxed to "matches the reference": if the reference itself glows, the diagnosis step below stops the build.
- [ ] `mover-setpiece` station 2, raised room against today's build (owner A/B). The interior receives gated direct light and bounce only through the doorway, ramps while rising, and returns while lowering.
- [ ] A skinned-mesh entity on the enemy spot darkens while the wall is closed and relights as it opens (owner A/B against today).
- [ ] A mover's own faces, lit by a light it does not block from them, are no darker than today: the wall's south face under the arena lights, and the room's west face under (640, 320, 192).
- [ ] On a gated light, promotion w = 0 and w = 1 agree within `LIT_ENDPOINT_DRIFT` on the receiver's lit texels, at the mover's closed terminus and at mid-travel.
- [ ] 5300M, `mover-setpiece`, `POSTRETRO_GPU_TIMING=1`, streaming `async` and `off`: the `direct_sh_compose` plus `animated_direct_sh_compose` passes with both movers travelling, minus both docked, are ≤ 0.2 ms per mover.
- [ ] 5300M, `mover-setpiece`, `POSTRETRO_GPU_TIMING=1`: the `sh_compose` pass with both movers travelling, minus both docked, is ≤ 0.2 ms per mover.
- [ ] `research.md` records section bytes, direct and indirect halves apart, against id 35, ids 41 + 45 and id 34, for the foundation's five maps (measure and report).
- [ ] Full-hallway cold build against the parent, idle machine (class named), one named `-j`, `--no-cache`: SH Bake growth and peak RSS growth from the indirect half (RSS tool named), reported with the SH gate's logged prediction and its admission. The owner sets the gate at review (open question).

## Path

- **First slice, before format, streaming or cache work.**
  - Bake q for `setpiece_room` raised only, and force t = 1 through a dev override.
  - A/B station 2 and the room's own faces. This falsifies the riskiest assumption: that 1 m probes inside an 8 m room hold the gate.
  - The mover-aware sampler leaks across the mover's walls, because probe depth moments are mover-absent and movers sample with backface rejection off. The rule for probes inside a posed brush decides how dark the mover's own faces go (open question).
- **Dark-closet fixture.** A small hand map, `content/dev/maps/mover-closet.map` (name delegated): a bright room with static lights, a closet with none, joined by a doorway a two-waypoint `kinematic_mover` door seals at waypoint 0. No bright-room light has direct line of sight into the closet through the doorway, because direct light on world surfaces is brief 2's. The door is authored at a typical door thickness, thinner than the probe spacing. The reference build is generated in-test with the door swapped for a static brush at its closed pose, as `tests/buried_lights_compile.rs` generates maps. A README in the `mover-setpiece` style names the station.
- **Indirect half after the direct first slice.** Once the direct slice holds, bake S^b for the closet door and the raised room only, force t through the dev override, and run the reference-bake row before format work. This is where the posed-brushes-as-patches question is decided.
- **Leak diagnosis, in that slice, before format work.** Render the reference closet: the door replaced by a static brush of the door's thickness at its closed pose. The sampler weights each probe by one omnidirectional depth mean and variance (`sh_sample.wgsl` `sh_corner_depth_visibility`), so probes straddling a wall thinner than the probe spacing blend light across it, static or posed.
  - Reference reads dark (owner judges): the leak was the bake's, and S^b_0 must match the reference. Continue.
  - Reference glows: the residual leak is the sampler's. It already exists for every thin static wall, and this brief cannot cure it alone. **Stop and report** with captures of both. The owner decides whether the roadmap's direction-aware probe visibility leg joins the sprint. Never relax the closet row to pass.
  - Record which case held, with captures, in the plan of record. The same check runs at the raised room's walls.
- **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md` before the no-pop row. The channel scales exactly the arm that reads low. Measure whether the static (id-41) arm shares the gap. This brief owns the fix wherever the cause lies (the id-41 or id-45 bake, or the sampler), and the no-pop row stays absolute (owner, 2026-11-03). If the fix grows past this brief's scope, stop and report rather than relaxing the row.
- **Bake.**
  - `sh_bake/probe.rs` `bake_probe_direct_rgb` → `lightmap_bake::soft_visibility_early_out`. Evaluate one sample set under both traces, static alone and static plus posed brushes, and decide escalation jointly. Otherwise an early-out can price static blocking into one side only.
  - The prefilter is the foundation's (`mover_light_gate::light_candidates`, `MoverPoses`, `Terminus`). The memo shape is `pipeline/mover_light_stage.rs` `probe_gate`.
  - The bake must finish before `storage_level_ceilings`, whose `DeltaSectionsRef` gains the section.
- **Indirect bake.** Extend `surface_radiance/stage.rs` beside `bake_sh_base`; its sibling modules keep the stage file small. The gather and layer machinery is `surface_radiance/gather.rs`, `layer.rs`, `bounce.rs`.
- **Format and load.** Mirror `level-format/src/direct_sh_delta_volumes.rs` for direct entries and the id-27 (`DeltaShVolumes`) tile format for bounce states. Validation goes beside `validate_direct_sh_delta` in `level-loader/src/prl_lighting.rs`. The id goes in `section_registry.rs`.
- **Game.** Derive t in `crates/physics/src/kinematic_mover.rs` beside `mover_is_docked_closed`. Collect it where `runtime_movers.rs` builds `KinematicMoverInstance`, and call it at the `rebuild_blocked_portals` sites (`frame_loop/mod.rs`, `app/world_less_frame.rs`). The setter sits beside `Renderer::set_kinematic_mover_draws`. `main.rs` is past 12,000 lines; add nothing to it.
- **Compose.**
  - `direct_sh_compose.wgsl`: its L0 loop skips entries at w = 0, and channel entries must not be skipped.
  - `animated_direct_sh_compose.wgsl` `animated_light_scale`.
  - `sh_compose.wgsl`: the base read from `sh_base_atlas` takes the mover-bounce override at affected probes before id-27 deltas add.
  - The streamed planner takes a mover-state trigger: `compose_plan.rs` `ComposePlannerFrame` and `PassMembership`, `compose_staleness.rs` `PassTriggers`.
  - Both the streamed `sh_streaming/direct_compose.rs` and the legacy `direct_sh_compose.rs` need it.
  - Large files (`direct_sh_bake.rs`, `sh_density.rs`, `compose_plan.rs`, `frame_loop/mod.rs`) get sibling modules.
- **Rival: promotion priority instead of a bake.** Rank lights a mover gates higher for promotion; promoted slots already draw movers as casters, so receivers get exact shadows at any t with no section. Rejected: the slot budget cannot cover every gated light (the hallway gates lights on 11 movers), receivers outside promotion range keep leaking, and it does nothing for bounce.
- **Rival shape.** Store a blocked-light SH tile per terminus. It needs no T, but it costs about 200× the bytes, and two movers on one light subtract twice instead of composing.

## Open questions

- ~~Promotion pop ownership~~ — resolved (owner, 2026-11-03): this brief owns the fix; the row stays absolute. See Path.
- ~~Scatter billboards~~ — resolved (owner, 2026-11-03): non-goal; roadmap follow-up.
- `light_ramp_start` / `light_ramp_end` names. — owner, at review; non-blocking.
- What q does a valid probe inside a posed brush take: dark, tested without the brush containing it, or full? — **delegated**: the first slice decides it against the own-face row and reports it in the plan of record.
- Section name and exact field widths. — **delegated**, within the Wire format below.
- The indirect half's bake-time and peak-RSS gate on the full hallway. — owner, at review; the build measures and reports first.
- Whether posed mover brushes contribute patches with their albedo in the bounce bake. — **delegated**: decided against the reference-bake row.
- If the leak diagnosis finds the static reference glows, does direction-aware probe visibility join this sprint, ahead of the closet row? — owner, when the diagnosis reports; blocks only the closet row.

## Wire format

New section, next free `SectionId` (57 at read time). Mirrors id 41's (`DirectShDeltaVolumes`) header and per-affinity-cell CSR.

| Item | Pin |
|---|---|
| Endianness, version | Little-endian. First byte is a `u8` section-internal epoch, exact-match |
| Header | `affinity_factor` (`u8`, 4), `affinity_dims` (`u32` × 3); must equal id 35's grid |
| Cell index | `u64` valid-probe mask per affinity cell; `u32` × (cells + 1) CSR offsets, last = entry count. No cell-level array: every cell is L0 |
| Entry record, in order | `u32` mover id (id 43 `mover_id`); `u8` light namespace (0 = id-18 AlphaLights index, 1 = id-45 roster index); `u8` flags (bit 0 = carries its own tile, others 0); `u16` reserved 0; `u32` light index |
| Entry order | By cell, then mover id, then namespace, then light index; no duplicates |
| Visibility payload | Per entry, per valid probe in x-fastest order: `u8` q₀ then `u8` q₁, where q = byte / 255 |
| Own tiles | For flagged entries only, in entry order: one compact RGB f16 tile per valid probe, id 41's stride and tile dimension |
| Empty | The section is omitted when both halves have no entries, never written empty; either half may be empty alone |
| Bounce index | A second CSR over the same affinity cells: per cell, entries of `u32` mover id, `u32` reserved 0, ordered by mover id, no duplicates; each entry carries its own `u64` affected-probe mask |
| Bounce payload | Per bounce entry, per affected probe in x-fastest order: the S^b_0 tile then the S^b_1 tile, id 27's compact RGB f16 octahedral tile stride and dimension |
| Sentinels | None. An unresolved id, an index out of range, a reserved bit set, or trailing bytes is a load error |

## Boundary inventory

| Name | Rust | PRL | WGSL | Net wire | FGD |
|---|---|---|---|---|---|
| Mover light state | (mover id, t) per present mover, physics → game → renderer | n/a | per-mover t uniform/storage, renderer-owned | none (derived from `WireKinematicMoverState`) | n/a |
| Light ramp | parsed s, e on the mover component; t′ applied game-side | n/a (keys stay on the entity) | n/a | none (content) | `kinematic_mover` `light_ramp_start(float)` "0", `light_ramp_end(float)` "1" |
| Mover id | id 43 `mover_id`, `KinematicMoverInstance.mover_id` | entry `u32` | n/a | `WireKinematicMoverState.mover_id` | n/a |
| Light reference | AlphaLights index → id-40 selection index at load; id-45 roster index | entry namespace + `u32` | selection weight / animated scale index | n/a | n/a |
