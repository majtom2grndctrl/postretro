# mover-light-flow--probe-channel — research notes

Read at fd8cf76, 2026-11-03. Every handoff fact below was re-read in source this session; none was contradicted.

## Where this sits

- Research build order step 6 (door factor in the direct-SH compose), extended from doors to every analysed (mover, terminus), as step 12's rest-pose channel asks. Step 5 (world-surface masks) is brief 2. It follows this brief and reuses its mover-state input.
- The foundation (`plans/done/mover-light-flow--foundation/`) sized this work. Its probe gate counts one centre segment per probe. This brief stores the bake's own soft visibility instead, as the foundation's research names for the mask bake ("must use the bake's own samples per state").

## Grounding

| Claim | Symbol |
|---|---|
| Mover fragments sample the composed direct atlas at their world position, with backface rejection off | `kinematic_brush.wgsl` `sample_sh_direct` → `sh_sample.wgsl` `sample_sh_direct_corners_depth_aware(…, false, …)` |
| Skinned meshes and legacy billboards sample the same atlas; scatter billboards do not | `skinned_mesh.wgsl` `sample_sh_direct`; `billboard.wgsl` `sample_sh_direct` (legacy path only); ids 47/48 (`build_pipeline.md` §PRL section IDs) |
| Pass A: base id 35 − Σ w × id-41 tile, clamped once after the sum; the L0 loop skips entries at w = 0 | `direct_sh_compose.wgsl` `compose_main`, `selection_weight` |
| Pass B: + Σ curve × compose weight × id-45 tile | `animated_direct_sh_compose.wgsl` `animated_light_scale`, `animated_compose_weight` |
| id 35 sums every static lightmap light, directional included; id 41 holds only selected lights | `direct_sh_bake.rs` `static_direct_lights`; `entity_shadow_select.rs` `select_entity_shadow_lights` |
| One light at one probe is a single lobe × radiance × one scalar soft visibility, in ids 35, 41 and 45 | `sh_bake/probe.rs` `bake_probe_direct_rgb` (called from `direct_sh_bake.rs` and `animated_direct_sh_bake.rs`); `accumulate_sh_rgb(acc, l, radiance * v, 1.0)` |
| Soft visibility early-outs on a 4-sample probe set | `lightmap_bake::soft_visibility_early_out`, `SoftProbes` |
| Base storage is never coarser than a grid-matched delta entry | `sh_density::storage_level_ceilings`, `DeltaSectionsRef` |
| Default probe spacing is 1 m | `sh_bake::DEFAULT_PROBE_SPACING` |
| Promoted slots draw movers as casters | `RigidOccluderDepthPass::record_kinematic_movers`; `lighting::entity_occluder_eligible` |
| The path coordinate is private to physics; the docked predicate is public | `kinematic_mover.rs` `path_coordinate`, `position_for_phase`, `mover_is_docked_closed` |
| Blocked portals are rebuilt from the final post-tick phase and read game-side by visibility; the renderer gets only mover instances | `main.rs` `rebuild_blocked_portals` (called from `frame_loop/mod.rs`, `app/world_less_frame.rs`); `render_preparation.rs` `VisibleRenderPreparation::for_level`; `Renderer::set_kinematic_mover_draws` |
| Mover phase replicates; host/client docked agreement is pinned | `WireKinematicMoverState`; `client_predicted_mover_phase_matches_host_door_occlusion_phase` |
| Streamed compose plans per-pass triggers and per-row membership | `sh_streaming/compose_plan.rs` `ComposePlannerFrame`, `PassMembership`; `compose_staleness.rs` `PassTriggers` |
| `LIT_ENDPOINT_DRIFT` is 12, summed RGB | `crates/postretro/tests/capture_frame.rs` |
| Free section id | `section_registry.rs` ends at `GhostLinks = 56`; no draft or ready plan claims 57 |

## Representation

For light l at probe p, the bake stores T = lobe(l̂) × radiance × v, with v the soft visibility against static geometry. Posing a mover adds occluders to the same samples, so v_k ≤ v and the posed term is (v_k / v) × T. The terminus state is S_k = q_k × T with q_k = v_k / v. That is exact under the bake's own model: the tile packing is linear, and only id 35's BC6H rounding (which promotion already lives with) separates base from T.

| Option | Per (gated light, probe) | Needs T at runtime | Two movers on one light | Verdict |
|---|---|---|---|---|
| Scalar state q₀, q₁ | 2 B | Yes: id 41/45, else carried | Product of visibilities | Chosen |
| Absolute blocked tile per terminus | ≈ 432 B (2 × 216 B, 6×6 RGB f16) | No | Two subtractions, light removed twice | Rival |

Carried T costs ≈ 216 B per (light, probe) only where id 41/45 has no entry for that light at that cell. Non-selected static lights always qualify. Selected ones qualify where the drop policy left no id-41 entry.

## Compose algebra

Notation: U_l is the light's mover-absent term in the base (T for a static light); Q_l(t) = Π_m lerp(q₀ᵐ, q₁ᵐ, tₘ).

- Static, not promoted: atlas = base − U_l + Q_l × U_l.
- Static, promoted at w: (1 − w)(Q_l U_l) + w × runtime, so atlas = base − w U_l − (1 − w)(1 − Q_l) U_l.
- Animated at curve c and compose weight (1 − w): + c (1 − w) Q_l T_l.
- q ≡ 1 gives today's atlas term for term. That is the droppable-section proof and the no-residue rule.

## Byte estimate (est.)

From the foundation's cold probe totals (centre-segment counts, per terminus). Soft visibility gates penumbra probes the centre test misses, so these run low.

| Map | Gated probe-terminus counts | q bytes (2 B per union probe) | Carried T, all lights (216 B × union) |
|---|---|---|---|
| mover-setpiece | 6,443 (1,000 shared) | ≈ 12 KB | ≈ 1.3 MB |
| stress-warren-hallway-inspection | 56,672 (11,040 shared) | ≈ 0.1 MB | ≈ 11 MB |

The union counts a shared probe once: total − shared ÷ 2. Both columns count gated probes only. An entry's payload covers every valid probe in its 4×4×4 cell, so an isolated gated probe can cost its cell's 64 probes, plus a 12 B entry record. The carried-T column assumes no gated light has an id-41/45 entry. The manual row records real bytes.

## Risks

- **Leak across a mover's walls.** Probe depth moments are mover-absent, and movers sample with backface rejection off. So a fragment on the room's inner wall blends interior (gated) and exterior (ungated) probes within one spacing. Representation cannot fix this. The first slice measures it.
- **Probes inside a posed brush.** The foundation counts them as dark. Stored dark, they pull the mover's own lit faces down through trilinear weights. The own-face row guards this; the rule is delegated.
- **Early-out asymmetry.** The 4-sample early-out can return v = 1 while the posed trace escalates and prices static blocking that v skipped. Evaluate both traces on one sample set.
- **Promotion pop.** It is unrooted (`drafts/animated-promotion-direct-sh-pop.md`, animated tier, 145 vs 186). The channel scales the baked arm, so a low arm stays low after gating.

## Pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Arrival | A mover reaches its last waypoint on tick N | Frame N's state reads t = 1 (portals block only at waypoint 0; see P11) |
| P2 | Reversal mid-travel | A command reverses the mover at t = 0.4 | t continues from 0.4 in the new direction; no frame reads a terminus value |
| P3 | Two movers, one tick | Both advance on tick N | Each reports its own t in frame N; a probe both gate composes their product |
| P4 | Level load | A `start_on_spawn` mover before its first tick | t = 0 from its seeded phase; the first composed frame is gated |
| P5 | Promotion during travel | w rises while t changes | Each frame composes (1 − w) × lerp at that frame's w and t |
| P6 | Residency during travel | A row installs or enters the view gate mid-travel | It composes with the current t before any consumer samples it (`rendering_pipeline.md` §4 Sampled-row invariant) |
| P7 | Section stripped | Same PRL, section removed | Composes as no mover state supplied; equal to q ≡ 1 |
| P8 | Mask change | The baked-direct mask bit toggles mid-travel | The existing full repair recomposes channel rows at current t |
| P9 | Terminus with no segment | An auto-close (target waypoint 0, heading back) or a completed `once` run leaves the mover at a terminus whose heading points off the path | t reads that terminus, 0 or 1, from its waypoint index; the mover is never handed over ungated |
| P10 | Frames between ticks | Several frames present between ticks N and N + 1, or one frame runs two ticks | t is the last post-tick phase's and holds across interpolated frames; the gate leads the drawn pose by at most one tick; a frame that ran no tick recomposes no channel row |
| P11 | Closed arrival and portal seal | A mover reaches waypoint 0 on tick N | Frame N reads t = 0; its sealed portals block from tick N + 1, the docked predicate's settle tick; no frame has its portals blocked while t > 0 |
| P12 | t changes on a frame that records no compose | A held world-less frame, a planned frame with no encoder, or a failed encode sees t change; later frames' t is unchanged | The next recording frame composes the covered rows at current t; the change is never consumed unrecorded, as the light-term mask's T8 ordering |
| P13 | Relevel or restart | Level B installs after level A, both with mover id 0 at different t; or a restart re-seeds a mover from t = 1 | B's first composed frame, held settling frames included, uses B's seeded t; no row composes with A's last t or the pre-restart t |
| P14 | Phase set by a snapshot | A client joins while the host's mover is at t = 0.5, or reconciliation corrects a predicted phase, with no local tick | The frame the snapshot applies hands over its t and recomposes the mover's rows; the seeded t = 0 never reaches a presented play frame after it |
| P15 | Mover culled from beauty | A present mover is outside the frame's drawn cells, so it has a shadow instance only | It still hands over its t; probes it gates compose gated |
| P16 | Shared row, one mover held | Mover A holds at t = 0.3 while mover B travels; one row carries both movers' entries, others carry only A's | Each recompose of the shared row uses A's held 0.3 and B's current t; rows carrying only A's entries recompose nothing |
| P17 | Slot re-tenanted mid-travel | A row is evicted and its slot reused by another row on one frame while a mover travels | The new tenant composes its own channel entries at current t, or today's term if it has none; no tenant inherits the old row's gated term |
| P18 | Pass routing | One mover with only bounce entries, one gating only an id-45 light, and one gating only static lights travel in turn | Bounce-only recomposes the indirect pass alone; id-45-only recomposes Pass B alone; static-only recomposes Pass A and then Pass B at those rows |
| P19 | Edit order over one cache | Cold build; warm mover edit; warm static-light edit; warm revert of both, all on one cache | Each warm PRL equals its cold; the revert hits every direct, bounce and id-34 memo |
| P20 | id-34 hit, bounce miss | Warm mover-only edit: id 34's volume hits, so no patch cache is built; that mover's bounce memo misses | The bounce bake shades its poses from the gate's patch set and cached layers; the PRL equals cold; the SH gate prices this path's peak |
| P21 | Patch cache across stages | The mover bounce bake runs after the id-34 gather and must finish before the base storage-level classifier, which today runs before mover poses resolve | Mover poses resolve before the bounce bake; the patch cache drops exactly once, after it and before atlas preparation; the SH gate prices every bake stage it outlives |
| P22 | Own tile against final id 41 | A gated selected light's id-41 entry at a cell is removed by the exact-zero drop or reshaped by coarsening | The own-tile flag is decided against ids 41 and 45 as emitted, after the drop and coarsening; where no entry survives, the entry carries its tile |
| P23 | Degenerate path | A mover's total path length is at or below the minimum segment length, or one interior segment is | t stays finite: 0 for a zero-length path; a degenerate segment adds no length; no NaN reaches compose |

## Indirect half (added 2026-11-03)

Owner chose option A after confirming the leak: every surface's indirect comes from the SH probe grid (`rendering_pipeline.md` §4), and neither the SH bake nor the surface radiance cache reads sealed portals or posed movers (no `sealed_portal`, `blocked_portal` or door handling under `level-compiler/src/sh_bake*` or `surface_radiance/`). A dark room behind a closed door therefore reads bright-room bounce near the doorway. The id-34 bake runs in `surface_radiance/stage.rs` `bake_sh_base`, which drops the patch cache at gather end; the runtime base read is `sh_compose.wgsl` (`sh_base_atlas`, then id-27 deltas).

Options weighed: per-mover summed static bounce states (chosen); per-light bounce states (about 200× the bytes, and the risky '451 shape for curve-driven lights); defer to the first-slice A/B (leaves the closet case wrong). IP reading: `ip-check.md` §Bake position against '451 and the 2026-11-03 boundary report, item 11b (bounce blended by occluder state, static lights: out, leaning).
