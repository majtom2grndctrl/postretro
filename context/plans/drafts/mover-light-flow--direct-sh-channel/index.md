# mover-light-flow--direct-sh-channel

Brief · resumable · Lighting and GI · reads: `context/lib/rendering_pipeline.md` §4 (Baked direct for dynamic receivers, Promoted static lights, Promoted animated lights, Sampled-row compose) · `context/lib/build_pipeline.md` §Compiler pipeline step 12, §PRL section IDs, §Build Cache · `context/lib/networking.md` §Presentation events vs. replicated state · `context/research/gi-rtx-lite/index.md` §Build order steps 5–6 · `context/research/gi-rtx-lite/ip-check.md` §Piece 5, §Piece 7, §For door and mover light flow · read at fd8cf76

## Problem

Owner-observed defect on `mover-setpiece`, brief 1 of 2 in mover light flow. Movers and entities light themselves from the composed direct-SH atlas (ids 35, 41, 45). Every one of those bakes runs with every mover absent (id 43), and the compose has no input from mover state. So the risen room's interior is lit by the arena's baked lights as if its walls weren't there. An entity behind the closed sliding wall stays lit too. When this is done, each gated baked or animated-baked light reaches mover and entity receivers only where it would with the mover in place. It ramps with the mover's position along its path, and back on reversal. Promotion still hands that light off without a pop.

## Decisions

- **Direct light only, driven by mover state.** The channel changes only the direct-SH compose. Indirect (ids 34, 27) is untouched, which keeps '451 claim 1's indirect element (c) absent. A recompute fires only on a mover-state change, never on a light-source change. Any compose gate is framed as residency or visibility (`ip-check.md` §Piece 5 design-arounds, §For door and mover light flow). The channel stores per-light visibility and never swaps in an ambient or AO layer (§Piece 7). The claim chart is an engineering reading for counsel.
- **Representation: one scalar per state.** Per (mover, terminus, light, valid probe) the section stores q_k: the fraction of the light's baked soft visibility there that survives with the mover posed at terminus k, from the bake's own samples and seed. Warrant: `bake_probe_direct_rgb` stores one light at one probe as a single lobe × radiance × one scalar visibility, so the terminus state is exactly S_k = q_k × T (T being that light's per-probe tile), and a tile per state would only repeat T. Rival and byte estimates: `research.md`.
- **Runtime blend.** Compose builds each gated light's term as lerp(S₀, S₁, t′), a lerp of two stored terminus states of a light that does not change, and puts it in place of the light's mover-absent term. Algebraically this equals base + (lerp(q₀, q₁, t′) − 1) × T. The IP footing is not the formula's shape but that the term is direct light and the scalar is occluder state, not a light-source change (claim chart of US12293451B2 claim 1, 2026-11-03, for counsel). T comes from the light's id-41 or id-45 entry at that cell when one exists, else the section carries it. With q ≡ 1 there is no residue: the atlas equals today's.
- **Several movers on one light.** Each mover is still baked alone (`plans/done/mover-light-flow--foundation/`). Where several movers gate one light at one probe, compose multiplies their lerped visibilities. The result is never brighter than any single mover leaves it, and never negative.
- **Promotion.** The gated term lives inside the baked arm: a mover or mesh receives (1 − w) × lerp(S₀, S₁, t) + w × the runtime term, animated lights scaled by their curve. The runtime arm is unchanged, because promoted and dynamic slots already render movers as casters (`RigidOccluderDepthPass::record_kinematic_movers`, gated by `entity_occluder_eligible`). No receiver sums a light twice (`rendering_pipeline.md` §4).
- **Lights and movers in scope.** Gated lights are id 35's static lightmap lights, directional included, and id 45's animated-baked lights; SDF-shadow lights are absent from id 35, so nothing gates them. The unit is (mover, terminus), termini only, over the movers the foundation analyses (no spinners).
- **Blend scalar t.** t is the mover's arc-length fraction along its path: 0 at the first waypoint, 1 at the last. Intermediate waypoints are crossed linearly. Each peer derives t from the final post-tick phase the blocked-portal rebuild reads; phase already replicates (`WireKinematicMoverState`), so there is no wire change.
- **Author ramp (owner, 2026-11-03).** Two `kinematic_mover` keys reshape t into the blend input t′ = clamp((t − s) / (e − s), 0, 1): `light_ramp_start` (s, default 0) and `light_ramp_end` (e, default 1). Defaults give t′ = t. Example: a rising room whose doorway crests the floor at 60% of its rise sets s = 0.6. The keys are content, so every peer reads the same values; the game applies the remap before handing state to the renderer, so brief 2 inherits it. Parse rejects anything outside 0 ≤ s < e ≤ 1 with a compile error naming the mover. The ramp is keyed to position, never to a timer (owner chose position over a timed fade, 2026-11-03). A script method to change the ramp at runtime is deferred until a map needs one. Key names are a proposal; the owner confirms them at review.
- **Layers.** Physics derives t, since the path coordinate is private there (`path_coordinate`, `position_for_phase`). Game logic hands the renderer one (mover id, t) per present mover each frame, apart from the cull-split draw instances and naming no SH concept, because brief 2 reuses it unchanged; a mover with no supplied state is ungated. The renderer maps ids to section entries and owns every GPU write.
- **PRL section.** A new optional section takes the next free id. Wire format and boundary names are below.
  - Absent, the frame renders today's output. Malformed, or naming a mover or light that does not resolve, it is a load error.
  - It is whole-resident, outside id 50, as ids 47/48 are. Streaming it later is additive.
  - Its cells count as grid-matched delta entries for base storage levels (`sh_density::storage_level_ceilings`). So ids 34/35 pack finer under gated probes. A map with no movers, or whose movers gate no baked light, packs byte-identical.
- **Cache.** The channel uses a per-light memo shaped like `mover_light_probes`. A mover-only edit re-bakes only the lights that reach its old or new pose. The mover-absent bakes behind ids 34, 35, 41 and 45 never re-trace on a mover edit; only the storage levels under changed cells repack.
- **Per-frame cost.** Compose recomposes channel rows only when a mover's t changes, and then only the rows its entries cover. The budget is ≤ 0.2 ms per travelling mover on the 5300M (research step 5's gate). The game's t pass allocates nothing per frame.
- **Non-goals.**
  - World-surface masks (lightmap texels, shadowmask, id-25 weights) belong to brief 2. It consumes this brief's mover-state input.
  - Indirect light by mover state is counsel-gated ('451).
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
- [ ] Section bytes are identical across `-j 1` and `-j 4` and across two cold builds.
- [ ] Moving one mover's last waypoint: on a warm build, the bakes behind ids 34, 35, 41 and 45 trace nothing, only lights reaching the old or new pose re-bake channel entries, and the PRL equals the cold PRL of the edited map.
- [ ] A map with no movers, and a map whose movers gate no baked light, emit no section and pack byte-identical to the parent commit's PRL. This is an opt-in ignored test that fails on a missing baseline, after the foundation's parity test.
- [ ] A malformed section, one naming a mover absent from id 43, a light outside its namespace, or an affinity grid unlike id 35's, each fail the load with a message naming the section.

Blend scalar:
- [ ] t reads 0 docked at the first waypoint and 1 at the last. It reads the arc-length fraction mid-travel, and that fraction at the middle waypoint of a three-waypoint path. A wait or a blocked stop holds t.
- [ ] With default ramp keys, t′ = t at every sample above. With s = 0.6, e = 0.9: t′ is 0 at t = 0.6 and below, 1 at t = 0.9 and above, and 0.5 at t = 0.75, both rising and after a reversal. Parse accepts s = 0, e = 1 and s = 0.6, e = 0.9; it rejects s = e, s > e, s < 0 and e > 1, each with an error naming the mover.
- [ ] A reversal mid-travel, in either direction, continues from the current t with no jump.
- [ ] Two movers advancing on one tick each report their own t for that frame.
- [ ] A mover that reaches its last waypoint on tick N reads t = 1 in frame N's state, the same frame its blocked-portal state changes.
- [ ] Host and client derive equal t from one predicted phase at a terminus, mid-travel and after a reversal, as `client_predicted_mover_phase_matches_host_door_occlusion_phase` does for the docked predicate.

Compose (headless GPU, `POSTRETRO_REQUIRE_GPU`):
- [ ] At a probe gated only at terminus 0, the gated light's composed term is S₀ at t = 0, ungated at t = 1, and their exact linear mix at t = 0.5. A second light that the mover does not gate at that probe is unchanged at every t.
- [ ] Stripping the section from a PRL composes the same atlas as supplying no mover state. Both equal the atlas from supplying each mover's state with every stored q read as 1. The composed indirect atlas is identical in all three.
- [ ] Two movers gating one light at one probe compose their product. Neither alone is brighter than the pair, and no texel goes negative.
- [ ] A gated animated-baked light composes its curve × its gated term at every t. An SDF-shadow static light has no entry and composes as today.
- [ ] A promoted gated static light contributes (1 − w) × its gated term at every t, and nothing at w = 1. A promoted gated animated light also carries its curve.
- [ ] A frame where no mover's t changed and no other trigger fired recomposes no channel row. A changing t recomposes only rows its entries cover. A row that enters residency or the view gate while a mover travels composes with that frame's t before any consumer samples it.
- [ ] Streaming `off` and `sync-proof` compose identical channel output.

### Manual
- [ ] `mover-setpiece` station 2, raised room against today's build (owner A/B). The interior receives gated light only through the doorway, ramps while rising, and returns while lowering.
- [ ] A skinned-mesh entity on the enemy spot darkens while the wall is closed and relights as it opens (owner A/B against today).
- [ ] A mover's own faces, lit by a light it does not block from them, are no darker than today: the wall's south face under the arena lights, and the room's west face under (640, 320, 192).
- [ ] On a gated light, promotion w = 0 and w = 1 agree within `LIT_ENDPOINT_DRIFT` on the receiver's lit texels, at the mover's closed terminus and at mid-travel.
- [ ] 5300M, `mover-setpiece`, GPU timing: direct compose with both movers travelling, minus both docked, is ≤ 0.2 ms per mover.
- [ ] `research.md` records section bytes against id 35 and ids 41 + 45 for the foundation's five maps (measure and report).

## Path

- **First slice, before format, streaming or cache work.**
  - Bake q for `setpiece_room` raised only, and force t = 1 through a dev override.
  - A/B station 2 and the room's own faces. This falsifies the riskiest assumption: that 1 m probes inside an 8 m room hold the gate.
  - The mover-aware sampler leaks across the mover's walls, because probe depth moments are mover-absent and movers sample with backface rejection off. The rule for probes inside a posed brush decides how dark the mover's own faces go (open question).
- **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md` before the no-pop row. The channel scales exactly the arm that reads low. Measure whether the static (id-41) arm shares the gap. This brief owns the fix wherever the cause lies (the id-41 or id-45 bake, or the sampler), and the no-pop row stays absolute (owner, 2026-11-03). If the fix grows past this brief's scope, stop and report rather than relaxing the row.
- **Bake.**
  - `sh_bake/probe.rs` `bake_probe_direct_rgb` → `lightmap_bake::soft_visibility_early_out`. Evaluate one sample set under both traces, static alone and static plus posed brushes, and decide escalation jointly. Otherwise an early-out can price static blocking into one side only.
  - The prefilter is the foundation's (`mover_light_gate::light_candidates`, `MoverPoses`, `Terminus`). The memo shape is `pipeline/mover_light_stage.rs` `probe_gate`.
  - The bake must finish before `storage_level_ceilings`, whose `DeltaSectionsRef` gains the section.
- **Format and load.** Mirror `level-format/src/direct_sh_delta_volumes.rs`. Validation goes beside `validate_direct_sh_delta` in `level-loader/src/prl_lighting.rs`. The id goes in `section_registry.rs`.
- **Game.** Derive t in `crates/physics/src/kinematic_mover.rs` beside `mover_is_docked_closed`. Collect it where `runtime_movers.rs` builds `KinematicMoverInstance`, and call it at the `rebuild_blocked_portals` sites (`frame_loop/mod.rs`, `app/world_less_frame.rs`). The setter sits beside `Renderer::set_kinematic_mover_draws`. `main.rs` is past 12,000 lines; add nothing to it.
- **Compose.**
  - `direct_sh_compose.wgsl`: its L0 loop skips entries at w = 0, and channel entries must not be skipped.
  - `animated_direct_sh_compose.wgsl` `animated_light_scale`.
  - The streamed planner takes a mover-state trigger: `compose_plan.rs` `ComposePlannerFrame` and `PassMembership`, `compose_staleness.rs` `PassTriggers`.
  - Both the streamed `sh_streaming/direct_compose.rs` and the legacy `direct_sh_compose.rs` need it.
  - Large files (`direct_sh_bake.rs`, `sh_density.rs`, `compose_plan.rs`, `frame_loop/mod.rs`) get sibling modules.
- **Rival shape.** Store a blocked-light SH tile per terminus. It needs no T, but it costs about 200× the bytes, and two movers on one light subtract twice instead of composing.

## Open questions

- ~~Promotion pop ownership~~ — resolved (owner, 2026-11-03): this brief owns the fix; the row stays absolute. See Path.
- ~~Scatter billboards~~ — resolved (owner, 2026-11-03): non-goal; roadmap follow-up.
- `light_ramp_start` / `light_ramp_end` names. — owner, at review; non-blocking.
- What q does a valid probe inside a posed brush take: dark, tested without the brush containing it, or full? — **delegated**: the first slice decides it against the own-face row and reports it in the plan of record.
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
| Empty | The section is omitted when it has no entries, never written empty |
| Sentinels | None. An unresolved id, an index out of range, a reserved bit set, or trailing bytes is a load error |

## Boundary inventory

| Name | Rust | PRL | WGSL | Net wire | FGD |
|---|---|---|---|---|---|
| Mover light state | (mover id, t) per present mover, physics → game → renderer | n/a | per-mover t uniform/storage, renderer-owned | none (derived from `WireKinematicMoverState`) | n/a |
| Light ramp | parsed s, e on the mover component; t′ applied game-side | n/a (keys stay on the entity) | n/a | none (content) | `kinematic_mover` `light_ramp_start(float)` "0", `light_ramp_end(float)` "1" |
| Mover id | id 43 `mover_id`, `KinematicMoverInstance.mover_id` | entry `u32` | n/a | `WireKinematicMoverState.mover_id` | n/a |
| Light reference | AlphaLights index → id-40 selection index at load; id-45 roster index | entry namespace + `u32` | selection weight / animated scale index | n/a | n/a |
