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
| Free section id | `section_registry.rs` holds 58 (`LightmapPoolBound`) and 59 (`ProbeDepthTiles`); 57 stays free and `build_pipeline.md` names it `MoverProbeChannel` (re-read at 38ef0dfc2) |

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
| P24 | First supply of a held mover | A level installs with a mover docked at its seeded terminus; its t′ never changes after the first frame the game hands it over | That first frame writes every live tile the mover changes with lerp(D₀, D₁, t′) at its seeded t′ and points each probe's reference at it, in the submission before that frame's forward pass; no frame samples a live tile that was never written; before that first hand-over the probe keeps its id-59 tile or averaged moments |
| P25 | Depth and compose in one frame | A mover's t′ changes on tick N, including a step from 1 to 0 in one tick | Frame N's forward pass samples live tiles at frame N's t′, the same frame direct and bounce compose at it; no frame pairs S^b_0 with D₁ or S^b_1 with D₀ |
| P26 | Two movers, one live tile | Movers A and B both change a probe's depth; both advance on one tick, or A holds at 0.3 while B travels, with their states handed over in either order | The tile is written once per frame from both movers' current t′ under the delegated rule; its bytes do not depend on hand-over order; while A holds, each rewrite B triggers uses A's held t′ |
| P27 | Relevel, restart, hand-over before install | Level B installs after A, reusing mover id 0 at the same t′ as A's last frame; a restart re-seeds a mover; or a t′ is handed over while no level is installed | B's first frame writes every live tile from B's own D₀/D₁ at B's t′, whether or not it equals A's; a hand-over with no level writes nothing and is not consumed as B's state |
| P28 | Live-tile write on a frame that records nothing | A skipped present, a held world-less frame, or a failed encode sees a mover's t′ change; later frames' t′ is unchanged | The next recording frame's live tiles hold the current t′; a change is never consumed unwritten, as P12 for compose |
| P29 | Live tiles without id 59 | A PRL whose id 59 is absent or has zero tiles carries a depth block | The tile texture, grid words and whole-load moments are sized from the live tiles alone; the closed door's probes reference their live tiles; stripping id 57 as well restores the 1×1 dummy |
| P30 | t′ change and unload on one frame | A mover's t′ changes on the frame its level unloads or a relevel installs | No live-tile write or reference texel lands in either level's textures; no upload is pending at the install or unload boundary |
| P31 | Id-59 trim after mover depth | Id 59 is ranked inside the base gather at the full cap and cached with id 34; the mover tile count N is known only after the posed re-gather, cold or from the bounce memo | After the mover bake, id 59 keeps exactly the tiles a selection at cap − N would keep, by its own rank; a mover-only edit that changes N traces no id-34 ray and re-trims; the depth-tile log line and the build summary report the trimmed count and the shrunk cap |

## Indirect half (added 2026-11-03)

Owner chose option A after confirming the leak: every surface's indirect comes from the SH probe grid (`rendering_pipeline.md` §4), and neither the SH bake nor the surface radiance cache reads sealed portals or posed movers (no `sealed_portal`, `blocked_portal` or door handling under `level-compiler/src/sh_bake*` or `surface_radiance/`). A dark room behind a closed door therefore reads bright-room bounce near the doorway. The id-34 bake runs in `surface_radiance/stage.rs` `bake_sh_base`, which drops the patch cache at gather end; the runtime base read is `sh_compose.wgsl` (`sh_base_atlas`, then id-27 deltas).

Options weighed: per-mover summed static bounce states (chosen); per-light bounce states (about 200× the bytes, and the risky '451 shape for curve-driven lights); defer to the first-slice A/B (leaves the closet case wrong). IP reading: `ip-check.md` §Bake position against '451 and the 2026-11-03 boundary report, item 11b (bounce blended by occluder state, static lights: out, leaning).

## Closed-pose depth (re-read at 38ef0dfc2)

Why the bounce half alone leaves the closet glowing: closet probes get closed-pose bounce, but bright-side probes stay correctly bright, and their depth (id-34 moments, id-59 tiles) was baked with the door absent. A closet fragment near the door blends them at full Chebyshev weight, as the static door did before tiles.

| Claim | Symbol |
|---|---|
| The tile texture is sized at load from id 59's count and keeps only its view | `renderer` `ShDepthTiles::new`; `ProbeDepthTileTextureLayout::for_tile_count`; render-cpu `depth_tile_texture_plan` |
| The reference table's tile count is fixed; references past it are refused | render-cpu `ProbeDepthTileReferences::set_reference` → `TileOutOfRange` |
| Id 59 cannot carry spare tiles: tile i belongs to strictly ascending `probe_indices[i]` | level-format `ProbeDepthTilesSection::from_bytes` |
| Reference is 15 bits under a sign flag; cap 1 << 15 | `DEPTH_TILE_REFERENCE_FLAG`, `MAX_PROBE_DEPTH_TILES` |
| Any gather's per-ray distances bin to a tile; selection and cap are separate | `sh_bake/depth_tiles.rs` `TileRayTable::interior`, `bordered_tile`, `DepthTileSelector::consider` |
| Tile bytes reach the SH ledger from the texture extent | `ShDepthTiles::new` → `depth_tile_allocation`; `probe_depth_tile_texture_bytes` |
| Whole-load id-34 moments are kept only when the reference table has a layout; the rewrite expects them; the tile texture is sampled + copy-dst only, and `Rg16Float` storage writes need an adapter feature the device does not request | `ShDepthTiles::new` (`whole_load_moments`); `set_depth_tile_reference`; `sampled_upload_usage`, `depth_tile_allocation`; `request_renderer_device` |

**Blend, not switch.** Moments are expectations, so lerp(D₀, D₁, t′) is the moment pair of the mixture distribution "depth from D₀ with probability 1 − t′, else from D₁". It is a valid moment set at every t′ (variance stays ≥ 0 by Jensen), the Chebyshev bound reads it unchanged, and t′ = 0 or 1 reproduces a terminus exactly. A reference switch would hold D₀ until the door leaves its dock, then jump to D₁, while the direct and bounce terms ramp from the same t′. IP: `ip-372-direction-aware-visibility.md` design lever 7 names switching or blending moment sets as the safe shape; counsel question 9 asks about switching.

**Closet water was a separate defect**, fixed by PR #662 before this brief: a mover near a selected static light promoted it, and fluids added promoted static lights back unshadowed through walls. Fluids and legacy billboards now sample a promotion-free direct atlas; movers and skinned meshes sample the handoff atlas (`rendering_pipeline.md` §4, Baked direct for dynamic receivers).

## Acceptance instruments (AC audit, 2026-11-08)

Notes for the executor from the achievability audit; none changes a row.

- q is stored as a byte, so "equals" in Bake 1 holds within 1/510; the denominator is the joint-sample static count (Bake row 1).
- No knob lowers the tile cap today: production passes the full constant into `bake_sh_base`. Rows under "a lowered test cap" need a test seam. The 1 << 15 load case needs a synthetic PRL.
- `bake_counters` cannot tell id-34 gather rays from the mover re-gather's; prove "traces no id-34 ray" with id 34's `[cache] … hit` line.
- The full re-bake domain check runs about four whole id-34 bakes on `mover-setpiece`: `#[ignore]`.
- "Measured peak" is the compiler's `live_bytes` ledger, which counts only what routes through it; name the fixtures.
- The duplicated static light in the cache row is appended to the map; an in-place duplicate shifts later lights' seeds.
- The no-change parity map must be authored with movers no probe ray reaches.
- Exact compose mixes hold only with values exact in f16 (q bytes 0 and 255); the two-mover sum and the clamp need their own fixtures.
- The tile texture lacks `COPY_SRC` (`sh_allocation.rs`); readback rows add it.
- Capture writes one image per run; frame-to-frame rows (P24, P25) and the tick-driven ramp row need an offscreen renderer test or a capture extension. Capture poses movers only at rest today; forced mover t is this brief's.
- "Sampler shader byte-identical to the parent's" needs a pinned hash or a preflight diff gate.
- Rows that mix a game-side half with a GPU compose half (blend-scalar ordering rows, the animated-light row) split by crate.
- `mover-setpiece`'s chaser patrols off the enemy spot: capture and the manual A/B hold a receiver there.
- Live-tile uploads share the per-frame upload copy batch; their cost is travelling minus docked.
- `mover-setpiece` was relit 2026-11-07 (49e03a39c); the foundation's `mover_light_gate_setpiece.rs` holds the current light sets.

## Build notes (first plan, read at 44881f797)

From the first build attempt, 2026-11-03. Source facts and proposals, not findings: re-verify each against current source.

**Direct bake.**
- Denominator: the baked trace can agree all-clear at 4 samples while the posed trace splits and escalates. Bake row 1's joint-sample count keeps static blocking out of q, so two movers on one light never count a hidden static occluder twice. A fully sealed mover stores q = 0. `soft_visibility_with` takes one trace, so a two-trace function is new.
- Direct memo key: `mover_light_probes`' key folds `geometry_content_hash`, which serializes texture names, so a retexture moves it. The q key folds an occlusion-only geometry hash (positions, indices, solidity, as the SDF key does) plus the light's seed axis.
- `probe_gate` is a centre-segment prefilter and misses penumbra probes; the q bake uses its own sample-level reach test.
- Posed occlusion: `segment_clear(static) && !segment_touches_brush(posed)`. Proposal.

**Bounce bake and cache.**
- `PatchCache` is built only on an id-34 miss and the gate's `PatchSet` always; neither survives `bake_sh_base`'s return, so its handoff changes. `stage_events::PATCH_CACHE_DROPPED` proves the drop.
- For the posed gather, closest hit = min(static hit, analytic entry distance into the posed convex brushes), with no BVH per pose. Patches for posed faces need triangles from `extract_kinematic_mover_geometry`. Proposal.
- Placement: after the q bake per the Path. Directly after the id-34 gather also meets P21 and frees the patch cache sooner, but may overlap the volume store; measure the peak before moving it.
- The SH gate is two gates in `admit_sh_working_sets`. The bounce bake is priced in the base gate, which runs before the material stage, so `MoverPoses::resolve` hoists ahead of it (it depends only on map data). The error text "before patch shading" generalizes.
- `DeltaSectionsRef` gaining the section reaches `delta_pin_counts` and `apply_forced_scale_constraints`, but `has_delta_entry` lists its views by hand. The id-41 cap goes in at `classify_direct_levels` and must survive `apply_runtime_safe_envelope`.
- Id 34's dense tile carries alpha; bounce tiles drop it. The BC6H encoder needs dimensions in multiples of 4, so a 6×6 tile pads to 8×8; decode with `decode_bc6h_block`.
- Direct seeds use position in `static_lights`, so the cache row's duplicated light is appended.
- Section plumbing follows `ghost_links`: `PostBakeDeltaSections`, `pack/section_plan.rs`, `finalized_publication.rs`, loader `staged.rs`, `entities.rs`, `prl.rs`.

**Runtime.**
- Compose binding walls: 8 storage buffers per stage by default; Pass A uses 6, Pass B and `sh_compose` 8 (re-read at 9382e3c23; Passes A and B now also write two storage-texture outputs). Proposal: one renderer-owned channel storage buffer per pass, holding entries, q bytes, tiles and a per-mover t block; freeing a slot in Pass B and `sh_compose` means merging two read-only `u32` arrays with matching lifetimes. `GridDims` was read as exactly 64 KiB; unverified since. The engine limit is not raised.
- The GPU layout need not match the wire layout: the loader re-sorts entries light-major per cell, so the multi-mover product is one linear walk.
- Per-mover membership is new: a per-mover row bitset plus a t-changed input to `ComposePlannerFrame`, not an extension of `PassMembership`. A shared row reads every mover's current t, so no per-mover epochs (P16).
- `use_promotion_subtraction` needs the dynamic mask bit; channel terms gate on `BAKED_DIRECT_STATIC` alone.
- The `sh_compose` clamp applies at channel-touched probes only, so an absent section equals today's. Legacy `should_dispatch` needs a t-changed term committed with the dispatch, as `last_composed_mask` is (P12).
- Hand-over at the `rebuild_blocked_portals` sites covers held world-less frames, which never call `set_kinematic_mover_draws`. `collect_with_mirror` skips movers with no `Transform` or bounds, so t reads the registry. The renderer's t state resets at level install (P13).
- `path_coordinate` is in waypoint-index units and ignores the minimum segment length; a new arc-length function matches `position_for_phase`'s degenerate rule (P23).
- `LIT_ENDPOINT_DRIFT` is function-local in `capture_frame.rs`; hoist it. The streamed analogue of commit-after-encode is in `compose_plan.rs`.

**Slice method.** Format work waits on the bounce gate, so the slices bake a pose straight into the existing sections. A dev-only compiler switch poses one named mover at one terminus: ids 35/41/45 trace with it as an occluder, or id 34 is replaced by S^b. Capture draws the mover at that pose. That PRL is the composed t = 1 atlas, so it tests the premise with no format or compose code. The switch is deleted before review.

**Risks.**
- Later-bounce drift (highest): later bounces carry a mover's effect across every connected surface, and a flat or dark tile's round-trip error can be near zero, so the affected domain could spread far past the mover and inflate bytes. The bounce slice measures it; if the bar admits far probes, stop and report, since a perceptual floor would change a Decision.
- Gate prediction before the domain is known: an all-valid-probes bound could refuse maps that fit (the hallway has 11 movers). The bounce slice reports the bound against the measured peak.
- The reference build changes faces, patches and cells, not only occlusion. Closet probes should agree since both are near dark; if they miss the BC6H bar, stop and report without relaxing it.

## Leak diagnosis history (2026-11-04)

The first build stopped here. With today's compiler at 7af769fba, the static-door reference closet glowed in a narrow band at the door, so direction-aware probe visibility (id 59) went first; its tiled reference is now the bar (brief, Reference closet). Captures are in `captures/`; `verdict-sheet.png` holds the six panels that matter. Values are post-tonemap sRGB, 0–255.

| Build | Station 3 frame mean | Station 2 frame mean | Floor 0.04 m past the door plane | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Today, mover door | 6.60 | 6.83 | 23 | 19 | 17 |
| Reference, door 8 units back (the fixture) | 0.50 | 0.15 | 13 | 2 | 0 |
| Reference, door flush with the closet wall | 0.26 | 0.14 | 17 | 10 | 1 |
| Reference, no plane straddles the door | 0.00 | 0.00 | not profiled | not profiled | not profiled |

- Most of the mover door's glow is the bake's: it fills the room, where the reference's stays within about 0.5 m of the door.
- The rest depended on the lattice phase: about 59% of placements of a 16-unit door are exposed. The fixture sits at y 760, the worst phase measured, so the closet rows cannot pass by luck.
- Door recognition seals the doorway, and static-direct-only captures read black in the closet in every build, so no bright-room light reaches it directly.
- The door's closet face read 1.7 at slab centre in today's build against the reference's 0, from mover-absent data.

## Bounce slice measurements (2026-11-08)

Task 2 stopped at the drift check: the affected-probe bar admits probes far from the mover. Built on the main checkout at 51b4ea966 plus the uncommitted slice (`mover_probe_channel/{posed_scene,bounce,gather,domain,bake}.rs`, temporary `dev_bounce.rs`). Unswitched PRLs of `mover-closet` and `mover-setpiece` are byte-identical to HEAD's, cold.

**Method.** Each pose re-bakes id 34 over the gate's patches: bounce one re-shaded only for (patch, light) pairs whose shadow segments can meet the pose, posed sides as patches cut by the mover's other brushes, four later bounces over both patch sets, then every valid probe's 256-ray gather. Closest hit is the nearer of the static BVH's and the analytic entry into the posed convex brushes; a posed face flush with a static one leaves the static hit. A probe whose rays meet nothing posed and whose hits read static radiance gathers id 34's bytes exactly.

**Drift.** Every pose changes every valid probe's bounce by more than zero. The bar admits:

| Pose | Valid probes | Past the bar | Distance from pose bounds, m (median / p95 / max) | Rays meet the pose |
|---|---|---|---|---|
| `closet_door` closed | 1,689 | 144 (all closet probes; no bright-room probe) | 3.5 / 5.8 / 6.1 | 1,040 |
| `closet_door` open | 1,689 | 0 (S^b₁ equals id 34 bit for bit) | — | 0 |
| `setpiece_wall` closed | 11,942 | 11,420 | 12.4 / 24.9 / 30.1 | 11,071 |
| `setpiece_wall` open | 11,942 | 597 | 6.8 / 20.3 / 28.9 | 2,009 |
| `setpiece_room` lowered | 11,942 | 3,090 | 6.1 / 13.2 / 32.4 | 6,062 |
| `setpiece_room` raised | 11,942 | 3,154 | 3.1 / 13.7 / 34.3 | 8,916 |

The far probes are real, not numerical drift. For the raised room, at the 228 admitted probes 12 m or more away, |S^b − id 34| is about 0.002–0.003 against bars of 0.001–0.002 (median ratio 1.3), on dim tiles (median tile max 0.029): a 7–20% change. At every one, S^b sits 10–20× nearer the static-room reference's id 34 than today's id 34 does. A perceptual floor would cut them, which changes the bounce Decision's domain: owner.

**Reference checks** (scratch dumps, not yet tests).
- B3 closet closed: 144 closet probes valid in both, 0 past the reference tile's own bar (both read black; today's id 34 differs from the reference by up to 0.32). Open: S^b₁ equals today's id 34 exactly.
- B4: of the reference's 40 tiles, 28 sit on probes whose rays meet the closed door; all 28 match the closed-pose tile within f16 rounding (max 0.0005). The other 12 never meet the door and equal today's id-59 tiles.
- B7 raised room: 232 interior probes valid in both; 14 miss the reference tile's bar, all but one at floor height, margins up to 0.0034 (|id 34 − reference| there is 0.013–0.026). Patch seed noise explains part: re-seeding the posed patches moves the miss count between 9 and 14 and the worst probe by 0.0016. One floor probe stays about 0.003 over a 0.001 bar at every seed: research's "reference changes faces, patches and cells".

**Bytes and tiles, partial M7** (closet closed only; setpiece all four poses). Bounce block 216 B × 2 per affected probe plus 16 B per cell entry; depth 2 × 400 B per changed probe.

| Map | id 34 | id 59 | Bounce block | Depth block | Live tiles | Closed-pose tiles | id 59 + live + closed-pose |
|---|---|---|---|---|---|---|---|
| `mover-closet` | 69 KB | 9 KB (23 tiles) | 62 KB | 840 KB | 1,040 | 2,080 | 9.6% of 1 << 15 |
| `mover-setpiece` | 531 KB | 228 KB (565 tiles) | 6.5 MB | 16.2 MB | 11,566 | 39,990 | 159% of 1 << 15 |

The depth domain the brief names ("every probe whose gather rays the posed mover changes") is most probes that see the mover: on `mover-setpiece` its closed-pose tiles alone pass the 15-bit tile reference. No id-59 tile would be trimmed for live tiles alone (37%).

Not run, because of the stop: captures, M1 on the hallway, M7 for the other three maps, the direct-half byte column.
