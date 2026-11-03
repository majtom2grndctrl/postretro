# mover-light-flow--probe-channel

Brief · resumable · Lighting and GI · reads: `context/lib/rendering_pipeline.md` §4 (Baked direct for dynamic receivers, Promoted static lights, Promoted animated lights, Sampled-row compose) · `context/lib/build_pipeline.md` §Compiler pipeline step 12, §PRL section IDs, §Build Cache · `context/lib/networking.md` §Presentation events vs. replicated state · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/index.md` §Build order, §Door classification · `context/research/gi-rtx-lite/ip-check.md` §Piece 5, §Piece 7, §Bake position against '451 · read at 33ac590

## Problem

Brief 1 of 2 in mover light flow, for the set-pieces the product makes first-class: monster closets, scripted reveals, a wall that slides, a room that rises (`context/lib/index.md` §1). Every probe bake runs with every mover absent (id 43): the direct-SH bakes movers and entities light themselves from (ids 35, 41, 45), and the SH base (id 34) every surface takes its static bounce from. The compose has no input from mover state. So, on `mover-setpiece`, the risen room's interior is lit by the arena's baked lights as if its walls weren't there, and an entity behind the closed sliding wall stays lit. A dark closet sealed by a door glows with the bright room's bounce near the doorway, as if the door were open. And promotion pops today on a closed closet door (`drafts/animated-promotion-direct-sh-pop.md`: 145 at w = 0, 186 at w = 1). When this is done, a gated light's direct term reaches mover and entity receivers only where it would with the mover in place; static bounce near a mover matches a bake with the mover in place; both follow the mover's position along its path, and back on reversal; and a gated light hands off to promotion without a pop.

## Decisions

- **Driven by mover state, never by light state.** Neither half recomposes because a light changed. Any compose gate is framed as residency or visibility. The channel stores per-light direct visibility and per-mover absolute static bounce, never a swapped ambient or AO layer (`ip-check.md` §Piece 7). The bounce half sums static lights only, so it never covers a light that carries a curve. On the 2026-11-03 claim chart both halves lack '451 claim 1's light-source change and its changed-light coefficient (`ip-check.md` §Bake position against '451). Engineering reading for counsel.
- **Commitments this reverses (owner, 2026-11-03).** The bounce half overturns "doors change direct light only" (`gi-rtx-lite/index.md` §Hybrid stack; the foundation's bounce non-goal) and settles the research's open "is door bounce worth its bytes" by taking static door bounce now. Summing per-mover bounce differences is the accepted approximation for doors in series (`gi-rtx-lite/index.md` §Diagnosis of today's GI), so redesign gate item 1 stays not required. The rest of the hybrid stack stands.
- **Direct half: one scalar per state.** Per (mover, terminus, light, valid probe) the section stores q_k, the fraction of the light's baked soft visibility there that survives with the mover posed at terminus k, from the bake's own samples and seed. Warrant: `bake_probe_direct_rgb` stores one light at one probe as one lobe × radiance × one visibility, so the terminus state is exactly S_k = q_k × T, T being the light's per-probe tile.
  - T comes from the light's id-41 or id-45 entry when that entry is stored per probe. Under gated probes id 41's storage level is capped like ids 34/35's (below), so T is never rebuilt from corners. Where no entry survives, the section carries T.
  - Compose puts lerp(S₀, S₁, t′) in place of the light's mover-absent term. With q ≡ 1 the atlas equals today's.
  - Several movers gating one light at one probe multiply their lerped visibilities: never brighter than any one leaves it, never negative.
- **Bounce half: absolute static bounce per state.** Per (mover, terminus k, affected valid probe) the section stores S^b_k: id 34's own content (every static light's bounce one plus `_bounce_fill` × later bounces) re-baked with that mover's brushes posed at terminus k and every other mover absent. Animated lights' bounce (id 27) is not in it.
  - A probe is affected when its value at either terminus differs from id 34's by more than the per-probe BC6H round-trip error of id 34's encoded value (exact match under `--uncompressed-irradiance`). The domain is derived, never sampled.
  - Compose puts lerp(S^b_0, S^b_1, t′) in place of id 34 at affected probes; several movers at one probe add their lerped differences from id 34. Compose clamps the sum so evaluated irradiance is non-negative in every direction. Id-27 deltas add on top as today.
  - Every reader of the composed indirect atlas inherits it: world surfaces, movers, entities, fog and billboard ambient.
  - Memory: everything the bounce bake holds resident is charged to the SH memory gate before any ray; overage is the named error, never coarsening (`development_guide.md` §1.4).
- **Bounce bake cost is gated before format work.** The bounce slice measures cold and warm mover-only cost on the full hallway; the owner sets the gate then. If over it, the fallback is to re-bake bounce one per pose and keep id 34's later bounces.
- **Leak diagnosis stops the build.** Before any code, a static-brush reference closet is rendered with today's compiler. If it glows, the residual leak is the sampler's (`sh_sample.wgsl` `sh_corner_depth_visibility`, one omnidirectional depth mean and variance per probe), this brief cannot cure it, and the build stops for the owner. The closet row is never relaxed to pass.
- **Promotion pop (owner, 2026-11-03).** This brief root-causes and fixes the pop wherever it lies (the id-41 or id-45 bake, or the sampler), and the no-pop row stays absolute. A gated term lives inside the baked arm: a receiver gets (1 − w) × its gated term + w × the runtime term, whose slots already draw movers as casters (`RigidOccluderDepthPass::record_kinematic_movers`). No receiver sums a light twice. If the fix grows past this brief, stop and report.
- **Lights in scope.** Static lightmap lights (id 35), directional included, and animated-baked lights (id 45). SDF-shadow lights, static or animated, are not gated: SDF shadows are soft-deprecated (`plans/done/mover-light-flow--foundation/`).
- **Blend scalar.** t is the mover's arc-length fraction along its path, 0 at the first waypoint and 1 at the last, derived on each peer from the final post-tick phase the blocked-portal rebuild reads. Phase already replicates (`WireKinematicMoverState`), so there is no wire change. Physics derives t; game hands the renderer one (mover id, t′) per present mover each frame, naming no SH concept; the renderer owns every GPU write. A mover with no supplied state is ungated.
- **Author ramp.** Two `kinematic_mover` keys reshape t into t′ = clamp((t − s) / (e − s), 0, 1): `light_ramp_start` (s, default 0) and `light_ramp_end` (e, default 1). Parse rejects anything outside 0 ≤ s < e ≤ 1, naming the mover. Mover entities never reach runtime as map entities (`parse.rs` routes `kinematic_mover` to the mover list), so the section carries a per-mover ramp record for every mover with entries; id 43 is unchanged. The ramp is keyed to position, never a timer. A runtime script method is deferred until a map needs one.
- **PRL section.** One new optional section at the next free id, whole-resident outside id 50, as ids 47/48 are. Absent, frames render today's output; malformed or unresolved, it is a load error. Its cells, direct and bounce, cap base storage levels as grid-matched delta entries (`sh_density::storage_level_ceilings`), and cap id 41's levels under gated probes. A map with no movers, or whose movers change no direct term and no probe beyond the bar above, packs byte-identical.
- **Cache: warm equals cold.** The direct half keeps a per-light memo shaped like `mover_light_probes`. The bounce half keeps a per-mover memo keyed on id 34's whole volume key plus the mover's posed brushes at both termini (and their materials, if they become patches), logging hits and misses per mover under `[MoverLight]`. Anything that re-bakes id 34 re-bakes every mover's bounce states; a mover-only edit re-bakes only that mover's.
- **Per-frame cost.** A travelling mover recomposes only rows carrying its entries; pass membership is tracked per mover, so a held mover's rows recompose nothing. This binds the streamed planner; the legacy compose recomposes its whole atlas on any t change, with that cost measured. The budget is ≤ 0.2 ms per travelling mover per frame on the 5300M, stricter than research step 5's per-door-event gate because a rising room travels for seconds. The t pass allocates nothing per frame.
- **Capture poses movers.** Capture scenes gain a forced mover t, the same override the first slices use, so promotion and A/B stills at mid-travel are reproducible.
- **Non-goals.**
  - World-surface masks (lightmap texels, shadowmask, id-25 weights): brief 2, which consumes this brief's mover-state input.
  - Scatter billboards (valid scatter, ids 47/48) read no direct SH, so one behind the closed wall stays lit. The next roadmap leg owns it. A billboard without valid scatter samples direct SH and inherits the channel.
  - Animated lights' bounce (id 27) behind a mover: per-light bounce for a curve-driven light is the risky '451 shape; it waits on counsel's answer on id 27.
  - Per-light bounce states for static lights: rejected, far larger for no visible gain, since static lights never change.
  - Light → cell reach and slab shadows: pool and slab shadows own dynamic-tier lights (`gi-rtx-lite/index.md` §Door classification), and these set-pieces need neither.
  - Spinners, and intermediate waypoints as states: the foundation analyses termini only.
  - A script-side binding to mover progress.

## Acceptance

### Automated
Bake and format:
- [ ] Each stored q_k equals the bake's soft visibility for that probe and light, with the posed brushes added to its occluders, divided by its baked visibility, under the same seed (unit test against the direct-SH probe bake). A light whose samples never meet the posed mover stores no entry for that mover.
- [ ] `mover-setpiece`: `setpiece_wall` closed stores q < 1 at back-room probes for both arena point lights and the spot, and at arena probes for both back-room lights. It stores no entry for the shaft light (992, 320, 288). `setpiece_room` raised stores q < 1 at its interior probes for the shaft light.
- [ ] Dark-closet fixture (new, below): with the door closed, the composed base at closet probes valid in both bakes (S^b_0, or id 34 where a probe stores no entry) equals id 34 of a reference build where the door is a static brush at its closed pose, within each probe's BC6H round-trip error. With the door open, S^b_1 equals today's id 34 within the same bar. The bright room's probes beyond the doorway's reach store no entry.
- [ ] `mover-setpiece`: `setpiece_room` raised stores indirect entries at its interior probes, each no brighter than today's id 34 there. A mover whose poses change no probe beyond the affected-probe bar stores no indirect entry.
- [ ] On the dark-closet fixture and `mover-setpiece`, a test-only re-bake of every valid probe at each terminus differs from id 34 beyond the affected-probe bar only at probes the derived domain admits.
- [ ] `mover-setpiece`: every id-34 and id-35 cell under a probe with a direct or bounce entry stores no coarser than a grid-matched delta entry, and cells elsewhere keep the parent build's levels.
- [ ] A directional light a mover blocks stores q < 1 under the posed brushes, and a spinner stores no entry.
- [ ] Adding an animated light to the dark-closet fixture's bright room leaves every bounce-state byte unchanged; its id-27 deltas compose unchanged at every t′.
- [ ] Section bytes are identical across `-j 1` and `-j 4` and across two cold builds.
- [ ] The SH gate's prediction covers the measured peak of the mover bounce bake on every fixture, counting the resident patch cache, per-pose shading, the accumulated bounce states and the memo writes. A map whose prediction exceeds the budget fails with the SH gate's named error before any bounce ray, at the same probe spacing.
- [ ] A cold build drops the patch cache exactly once, after the mover bounce bake and before atlas preparation, and the SH gate's prediction covers the measured peak of every stage it outlives (P21).
- [ ] Moving one mover's last waypoint: on a warm build, the bakes behind ids 34, 35, 41 and 45 trace nothing, only lights reaching the old or new pose re-bake direct entries, only that mover's bounce states re-bake (the per-mover `[MoverLight]` log shows one miss), and the PRL equals the cold PRL of the edited map. Editing one static light's colour, retexturing one static surface, or duplicating one static light each re-bake every mover's bounce states and no direct entry of another light; each PRL equals cold.
- [ ] A warm mover-only edit whose id-34 volume hits still re-bakes that mover's bounce states equal to cold, within the SH gate's prediction (P20).
- [ ] Over one cache, a mover edit, then a static-light edit, then reverting both each build warm equal to cold, and the revert hits every direct, bounce and id-34 memo (P19).
- [ ] A map with no movers, and a map whose movers change no direct term and no probe beyond the affected-probe bar, emit no section and pack byte-identical to the parent commit's PRL. This is an opt-in ignored test that fails on a missing baseline, after the foundation's parity test.
- [ ] A malformed section, one naming a mover absent from id 43, a light outside its namespace, an affinity grid unlike id 35's, or a ramp record that is non-finite or outside 0 ≤ s < e ≤ 1, each fail the load with a message naming the section.
- [ ] A grep gate keeps every SH, probe and section type out of the game-side mover-state collector and the renderer setter's signature. The closet fixture's bounce-only section and a direct-only section each load and compose.

Blend scalar:
- [ ] t reads 0 docked at the first waypoint and 1 at the last. It reads the arc-length fraction mid-travel, and that fraction at the middle waypoint of a three-waypoint path whose two segments differ in length. A wait or a blocked stop holds t.
- [ ] With default ramp keys, t′ = t at every sample above. With s = 0.6, e = 0.9: t′ is 0 at t = 0.6 and below, 1 at t = 0.9 and above, and 0.5 at t = 0.75, both rising and after a reversal. Parse accepts s = 0, e = 1 and s = 0.6, e = 0.9; it rejects s = e, s > e, s < 0, e > 1, s = nan, a non-numeric value, and e = 0 with s left at its default, each with an error naming the mover.
- [ ] A reversal mid-travel, in either direction, continues from the current t with no jump.
- [ ] Two movers advancing on one tick each report their own t for that frame.
- [ ] A mover that reaches its last waypoint on tick N reads t = 1 in frame N's state (P1).
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
- [ ] A gated animated-baked light composes its curve × its gated term at every t. An SDF-shadow light, static or animated, has no entry and composes as today.
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
- [ ] Leak diagnosis, before any code: today's compiler, the closet map, the door replaced by a static brush of its thickness. Owner judges with captures whether the reference closet reads dark; the result and captures go in the plan of record before the build continues.
- [ ] Bounce-slice cost, before format work: cold and warm mover-only builds of the full hallway, SH Bake and peak RSS against the parent, reported for the owner's gate.
- [ ] Dark-closet fixture, door closed (owner judges the outcome; today's build and the static-wall reference render shown beside it): the closet reads dark. The closet floor and walls near the doorway show no bright-room glow, and the door's closet face reads dark. A fog volume inside the closet reads dark too. Opening the door ramps the glow in with its travel; closing ramps it out. This bar is not relaxed to "matches the reference": if the reference itself glows, the diagnosis step below stops the build.
- [ ] `mover-setpiece` station 2, raised room against today's build (owner A/B). The interior receives gated direct light and bounce only through the doorway, ramps while rising, and returns while lowering.
- [ ] A skinned-mesh entity on the enemy spot darkens while the wall is closed and relights as it opens (owner A/B against today).
- [ ] A mover's own faces, lit by a light it does not block from them, are no darker than today: the wall's south face under the arena lights, and the room's west face under (640, 320, 192).
- [ ] On a gated light, promotion w = 0 and w = 1 agree within `LIT_ENDPOINT_DRIFT` on the receiver's lit texels, at the mover's closed terminus and at mid-travel, in capture with forced mover t and forced promotion weight.
- [ ] 5300M, `mover-setpiece`, `POSTRETRO_GPU_TIMING=1`, streaming `async` and `off`: the `direct_sh_compose` plus `animated_direct_sh_compose` passes with both movers travelling, minus both docked, are ≤ 0.2 ms per mover.
- [ ] 5300M, `mover-setpiece`, `POSTRETRO_GPU_TIMING=1`: the `sh_compose` pass with both movers travelling, minus both docked, is ≤ 0.2 ms per mover.
- [ ] `research.md` records section bytes, direct and indirect halves apart, against id 35, ids 41 + 45 and id 34, for the foundation's five maps (measure and report).
- [ ] Full-hallway cold build against the parent, idle machine (class named), one named `-j`, `--no-cache`: SH Bake growth and peak RSS growth from the indirect half (RSS tool named), reported with the SH gate's logged prediction and its admission. The owner sets the gate at review (open question).

## Path

- **Leak diagnosis first, before any code.** Author the closet fixture: `content/dev/maps/mover-closet.map` (name delegated), a bright room with static lights, a closet with none and a fog volume, joined by a doorway a two-waypoint `kinematic_mover` door seals at waypoint 0. The door is a typical door's thickness, thinner than the probe spacing. No bright-room light has direct line of sight into the closet, since direct light on world surfaces is brief 2's. Build it with today's compiler once more with the door replaced by a static brush of its thickness at its closed pose, and capture the closet.
  - Reference reads dark: the leak is the bake's, and S^b_0 must match the reference. Continue.
  - Reference glows: the leak is the sampler's and exists for every thin static wall. Stop and report with captures; the owner decides whether the roadmap's direction-aware probe visibility leg joins the sprint.
  - The in-test reference build is generated as `tests/buried_lights_compile.rs` generates maps. A README in the `mover-setpiece` style names the station.
- **Direct first slice.** Bake q for `setpiece_room` raised only and force t = 1 through the dev override. A/B station 2 and the room's own faces; this falsifies the assumption that 1 m probes inside an 8 m room hold the gate. Movers sample with backface rejection off and mover-absent depth moments, so the rule for probes inside a posed brush decides how dark the mover's own faces go (open question).
- **Bounce slice.** Bake S^b for the closet door and the raised room only, force t, and run the reference-bake row. Posed brushes join the occluders for patch direct, later bounces and the probe gather (`surface_radiance/stage.rs` `bake_sh_base`, `gather.rs`, `layer.rs`, `bounce.rs`). On an id-34 volume hit no patch cache exists, and on a miss the per-light layers drop before the later bounces (`stage.rs`), so the bounce bake builds what it needs itself. Later bounces run over every static face, so each pose costs about a whole-region later-bounce pass plus a gather; measure it here against the owner's gate. Decide here whether posed brushes become patches.
- **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md` before the no-pop row; measure whether the static (id-41) arm shares the gap.
- **Bake.**
  - Direct: `sh_bake/probe.rs` `bake_probe_direct_rgb` → `lightmap_bake::soft_visibility_early_out`. Evaluate one sample set under both traces, static alone and static plus posed brushes, and decide escalation jointly, or an early-out prices static blocking into one side only. Prefilter and memo shape are the foundation's (`mover_light_gate::light_candidates`, `MoverPoses`, `Terminus`; `pipeline/mover_light_stage.rs` `probe_gate`).
  - Mover poses resolve before the bounce bake and before `storage_level_ceilings`, whose `DeltaSectionsRef` gains the section; today they resolve after it (P21).
  - The own-tile flag is decided against ids 41 and 45 as emitted, after the exact-zero drop and coarsening (P22).
  - The bounce bake's resident set is priced by the SH gate across every stage it outlives (P21).
- **Format and load.** Mirror `level-format/src/direct_sh_delta_volumes.rs` for direct entries and the id-27 (`DeltaShVolumes`) tile format for bounce states. Validation goes beside `validate_direct_sh_delta` in `level-loader/src/prl_lighting.rs`; the id in `section_registry.rs`. Ramp keys parse in `parse_kinematic_mover`.
- **Game.** Derive t in `crates/physics/src/kinematic_mover.rs` beside `mover_is_docked_closed`; a terminus with no segment left reads its waypoint's t (P9). Collect it where `runtime_movers.rs` builds `KinematicMoverInstance`, including culled movers (P15), and hand it over at the `rebuild_blocked_portals` sites (`frame_loop/mod.rs`, `app/world_less_frame.rs`) and in capture (`capture/prepared.rs`). The setter sits beside `Renderer::set_kinematic_mover_draws`. Add nothing to `main.rs`.
- **Compose.**
  - `direct_sh_compose.wgsl`: its L0 loop skips entries at w = 0; channel entries must not be skipped.
  - `animated_direct_sh_compose.wgsl` `animated_light_scale`.
  - `sh_compose.wgsl`: the base read from `sh_base_atlas` takes the bounce override at affected probes, then the clamp, before id-27 deltas add.
  - The streamed planner takes a mover-state trigger with per-mover membership: `compose_plan.rs` `ComposePlannerFrame` and `PassMembership`, `compose_staleness.rs` `PassTriggers`. The legacy `direct_sh_compose.rs` recomposes whole.
  - Large files (`direct_sh_bake.rs`, `sh_density.rs`, `compose_plan.rs`, `frame_loop/mod.rs`) get sibling modules.
- **Rivals.**
  - Promotion priority instead of a bake: rank gated lights higher for promotion, whose slots already draw movers as casters. Rejected: the slot budget cannot cover every gated light, receivers outside promotion range keep leaking, and it does nothing for bounce.
  - A blocked-light SH tile per terminus: needs no T, but is far larger, and two movers on one light subtract twice instead of composing.

## Open questions

- `light_ramp_start` / `light_ramp_end` names. — owner, at review; non-blocking.
- The bounce bake's cost gate on the full hallway. — owner, when the bounce slice reports; blocks format work.
- If the leak diagnosis finds the static reference glows, does direction-aware probe visibility join this sprint? — owner, when the diagnosis reports; blocks the build.
- What q does a valid probe inside a posed brush take: dark, tested without the brush containing it, or full? — **delegated**: the direct first slice decides it against the own-face row.
- Whether posed mover brushes become patches with their albedo in the bounce bake. — **delegated**: the bounce slice decides it against the reference-bake row.
- The clamp rule that keeps summed bounce non-negative in every direction. — **delegated**, against the two-movers row.
- Section name and exact field widths. — **delegated**, within the Wire format below.

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
| Ramp record | Per mover with entries in either half, ordered by mover id: `u32` mover id, `f32` s, `f32` e; brief 2 extends the rule to its own entries |
| Sentinels | None. An unresolved id, an index out of range, a reserved bit set, or trailing bytes is a load error |

## Boundary inventory

| Name | Rust | PRL | WGSL | Net wire | FGD |
|---|---|---|---|---|---|
| Mover light state | (mover id, t) per present mover, physics → game → renderer | n/a | per-mover t uniform/storage, renderer-owned | none (derived from `WireKinematicMoverState`) | n/a |
| Light ramp | parsed in `parse_kinematic_mover`; s, e loaded from the section's ramp record; t′ applied game-side | section ramp record | n/a | none (content) | `kinematic_mover` `light_ramp_start(float)` "0", `light_ramp_end(float)` "1" |
| Mover id | id 43 `mover_id`, `KinematicMoverInstance.mover_id` | entry `u32` | n/a | `WireKinematicMoverState.mover_id` | n/a |
| Light reference | AlphaLights index → id-40 selection index at load; id-45 roster index | entry namespace + `u32` | selection weight / animated scale index | n/a | n/a |
