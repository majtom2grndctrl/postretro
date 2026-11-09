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

### B7 investigation (2026-11-08, second worker)

Built on 9dbf014d6 plus temporary switches in `mover_probe_channel/dev_trace.rs`: a patch-seed salt (`POSTRETRO_DEV_PATCH_SEED_SALT`, XORed into every patch seed, static and posed), per-ray and per-patch dumps of named probes and a box of patches, and a τ experiment (`POSTRETRO_DEV_POSED_TAU`). The reference and the posed bake (skip rule) were each baked at five patch seeds, and the missing probes were traced ray by ray and patch by patch.

| Source | Verdict | Evidence |
|---|---|---|
| (a) Patch layout, face splitting | **Cause** of one systematic miss, (965, 379.5, 4.9) | The reference's floor under the room is three faces, cut along the jamb planes y 272 and y 368; 30 of its 246 floor cells hold two half-patches. The posed floor top is one face. In cell (−19, 48) a light edge runs between y 364 and y 371. The reference's half-patch at y 371 reads 0.030–0.037 summed RGB (bounce one), and its half at y 361 reads 0.0067. The posed whole patch, centroid y 364.2, reads 0.0067. Cells whose centroids agree read alike to four digits. A hit on the y > 368 face blends only that face's patches. Seed-averaged bias 0.0034 against a 0.0011 bar; seed spread 0.0009. |
| (b) Mover-side patches against reference faces | Clean | Both builds hold the same 246 floor cells, none in only one. Posed sides cut by the mover's other brushes cover the reference's faces. |
| (c) Static faces under the raised footprint | Clean | The raised footprint is the shaft hole. The shaft below the slab is sealed in both builds, and no interior probe ray reaches it: every ray distance matches but the τ ray below. |
| (d) Probe validity and positions | Clean | Same grid. 232 interior probes are valid in both, and per-ray hit distances are identical. |
| (e) Monte Carlo patch-seed noise | **Explains most misses** | The reference against itself re-seeded misses its own bar at 2–9 of 232 probes (mean 6.5 over 20 pairings). The posed bake against itself re-seeded misses at 7.0 on average. Posed against reference misses at 7–16 across 25 seed pairings. The first worker's worst miss, (886.3, 300.8, 4.9), margin 0.0034, has a seed spread of 0.0054 in both bakes. |
| (f) Tracer tolerance τ (not on the list) | **Cause** of the systematic column at (886.3, 222.0), z 4.9, 44.2 and 83.6 | One gather ray per probe passes 0.4 mm outside the doorway jamb's corner edge (x 848, y 272). The static tracer counts a hit within τ = 0.5 mm of a triangle (`build_pipeline.md` §Ray traversal), and the posed analytic entry is exact. The reference reads the dark jamb at 1.66 m; the posed bake escapes to the arena at 26.7 m. Growing the posed brushes by τ in the probe gather removes all three probes from the seed-averaged miss list. |

Seed-averaged over five seeds each, 5 probes stay past the bar: the τ column (3), the split-cell probe, and one probe whose bias, 0.00021 against a 0.00017 bar, is inside its own 0.00034 seed spread. With τ growth, 2 remain: the split-cell probe and that noise probe. At every seed the composed base is nearer the reference than today's id 34 at 232 of 232 probes, with |composed − ref| / |id 34 − ref| at median 0.006 and max 0.28. In display terms (the bounce floor's measure, below), composed against reference reads median 0.06–0.09 levels and max 1.8–2.8. The reference against itself re-seeded reaches 2.5, and today's id 34 against the reference reads median 9.8 and max 30.

**Is the posed bake wrong?** Not in representation. Two discretization conventions differ:
- **τ.** The posed brushes are thinner than static ones by τ at silhouette edges in the gather. The posed shadow test uses the 0.1 mm contact band, not τ. This is a real inconsistency, but measure-zero: one ray per affected probe. It is not fixed by default. Growing planes by τ breaks the tracer's "never across the ray" rule at grazing rays, so the switch stays an experiment. Proposed for task 7: trace posed sides through the static tracer's own triangle hit test, with τ, over the pose-bounds-prefiltered posed triangles. The analytic entry stays only for rays from inside a posed brush, the inside-brush rule.
- **Patch partition.** Face extraction splits the static floor along BSP planes, and the posed sides are unsplit. Neither is more right. Matching it would mean reproducing face extraction's splits on posed sides.

A B7 at the BC6H bar fails for an exact bake, since the reference fails it against itself re-seeded. **Proposed restatement (owner decides):** "`mover-setpiece`: with `setpiece_room` raised, at every interior probe valid in both, the composed base is nearer the static-room reference's id 34 than today's id 34 is, and within 3 display levels of it (the bounce floor's measure; the reference re-baked at another patch seed moves up to 2.5). At those probes, each light the reference's walls block…" The direct clause is unchanged.

### Captures (2026-11-08)

Captures run with `--features capture`, `POSTRETRO_SH_STREAMING=sync-proof`, full-resident compose, offscreen, on the unfloored domain. Means are frame means of post-tonemap sRGB luma, 0–255. Files are `captures/t2-{closet,room}-<build>-<view>.png`. A temporary renderer switch, `POSTRETRO_DEV_MOVER_BACKFACE`, passes `true` for backface rejection in both of `kinematic_brush.wgsl`'s SH calls, direct and indirect.

**Closet, door closed** (960×540, every light term). The "outside door" column is the mean outside the door's projected box, the K1 measure. The floor profile is taken at station 3, 3×3 px, on the floor 0.04, 0.26 and 0.48 m past the door's closet face. The door-face column is the mean inside the door's box from (320, 600, 56), yaw 270, fov 60. Posed skip and posed dark are byte-identical PRLs: no lattice plane falls inside this door. Backface-on renders identically to skip, since only mover fragments change and the door face reads 0 in both.

| Build | Station 2 | outside door | Station 3 | outside door | Floor 0.04 / 0.26 / 0.48 m | Door closet face |
|---|---|---|---|---|---|---|
| Today | 4.16 | 4.30 | 3.64 | 3.87 | 30.0 / 24.5 / 20.7 | 1.75 |
| Posed (skip = dark) | 0.123 | 0.129 | 0.082 | 0.088 | 1.5 / 0.4 / 0.0 | 0.00 |
| Tiled static-door reference | 0.009 | 0.006 | 0.033 | 0.017 | 2.0 / 0.6 / 0.0 | 0.00 |
| Posed, reflections off | 0.009 | 0.006 | 0.032 | 0.016 | 1.5 / 0.4 / 0.0 | — |
| Reference, reflections off | 0.009 | 0.006 | 0.033 | 0.017 | 2.0 / 0.6 / 0.0 | — |

- **The probe channel holds the closet.** With fluid reflections off, the posed closet equals the tiled reference to 0.001 at both stations, and its floor profile is no brighter.
- **The remaining glow is the pool's reflection, not the probe data.** At the default reflection tier the posed closet reads 0.12 at station 2 against the reference's 0.009, all of it in the pool, which mirrors the lit doorway (`t2-closet-sheet.png`, ×20 rows). At the probe-only tier (`low`) station 2 reads 0.845 against 0.009. The pool's reflection probe (id 54) is captured at compile time with movers absent, so it sees the bright room through the open doorway. K1 ("no brighter than the reference plus 0.02") fails at station 2 (0.129 against 0.026) and station 3 (0.088 against 0.037) for this reason alone. The brief's "Closet water and fog" Decision names promotion as the water's only other path; this is a third, outside the probe channel. It needs an owner ruling: pose movers in reflection captures, or scope K1 and M2 to reflections off.

**Raised room** (`setpiece_room` posed raised, 1280×720, `light_term_mask` 351: every term but dynamic direct, so the room's carried lights are excluded). Views: interior, (860, 320, 72) yaw 0 pitch −8, as in Task 1. Upper walls, the same eye at yaw 20 pitch +28, toward the north-east upper walls and ceiling. West face, Task 1's. Station 2, README. "Ind." is `light_term_mask` 6 (indirect only); "dir." is 24 (baked direct only). The static-room reference is the room staticized at its raised pose (`staticize.py`), whose room faces are lightmapped world surfaces. Its own-face columns are therefore not like for like; its upper-wall columns are the bar for the leak question.

| Build | Interior | Upper | Upper ind. | Upper dir. | Interior ind. | Interior dir. | West face | Station 2 |
|---|---|---|---|---|---|---|---|---|
| Today | 30.62 | 19.90 | 7.44 | 14.50 | 5.69 | 28.25 | 38.66 | 35.41 |
| Skip | 7.22 | 5.55 | 4.48 | 1.135 | 1.66 | 5.85 | 38.88 | 34.67 |
| Dark | 7.02 | 4.06 | 3.96 | 0.093 | 1.60 | 5.70 | 38.20 | 34.53 |
| Skip, backface on | 7.19 | 4.88 | 4.51 | 0.355 | 1.66 | 5.82 | 38.95 | 34.77 |
| Dark, backface on | 7.06 | 4.60 | 4.49 | 0.095 | 1.64 | 5.71 | 38.95 | 34.76 |
| Static-room reference | 8.05 | 4.61 | 4.54 | 0.063 | 1.61 | 6.77 | 46.64 | 34.40 |

- **Upper interior walls (the owner's question).** Skip leaks exterior direct light onto them: 1.135 against the reference's 0.063, because a probe inside a wall serves both faces. Closed-pose depth does not fix that. Backface rejection cuts it to 0.355. Dark removes it (0.093), but dark alone darkens their bounce 13% (3.96 against 4.54). Dark with backface on matches the reference in both terms (direct 0.095 against 0.063, indirect 4.49 against 4.54, full 4.60 against 4.61).
- **Own faces (M5).** With depth in place, dark no longer costs the west face: 38.20 against today's 38.66, where Task 1 measured 25.7 without depth. With backface on, both rules read 38.95. So the Stop 1 objection to dark is gone.
- **Read: dark plus backface rejection on mover fragments** is the best of the four. Backface off for movers is a renderer contract (`research.md` §Grounding), so turning it on is a renderer change for the owner. Without it, dark costs 13% of upper-wall bounce, and skip leaks direct light.

`captures/t2-closet-sheet.png` has columns today | posed | reference. Its rows are stations 2 and 3 and the door face, then ×20 brightened stations 2 and 3 at the default reflection tier, then ×20 stations 2 and 3 with reflections off. `captures/t2-room-sheet.png` has columns today | skip | dark | skip + backface | dark + backface | static-room reference. Its rows are interior, upper walls, west face and station 2. Montage has no fonts, so neither sheet is labelled.

### Floor sweep (2026-11-08)

Both floors are slice parameters: `POSTRETRO_DEV_BOUNCE_FLOOR` (display levels) and `POSTRETRO_DEV_DEPTH_FLOOR` (score). They are local and deterministic, unset means no floor, and `POSTRETRO_DEV_PROBE_TABLE` writes every posed probe's measures for the sweep (`dev_floor.rs`).

- **Bounce measure** (`domain.rs` `display_levels`): the largest change, over a tile's texels and RGB channels, in 8-bit sRGB levels of a mid-grey (albedo 0.5) receiver lit by that texel alone, through the renderer's default indirect scale of 0.33 (`DEFAULT_INDIRECT_SCALE`) and the capture's soft-knee tonemap, which is the identity below 0.98, into an sRGB target. No exposure stage exists. The measure counts indirect only, so direct light never hides a change. A probe is affected when its change passes both its BC6H bar and the floor.
- **Depth score** (`sh_bake/depth_tiles.rs` `visibility_change`, extracted as a free function, bytes unchanged): id 59's own score of the closed-pose tile against the probe's mover-absent moments, which are its id-59 tile where id 59 keeps one, else its averaged id-34 pair. Id 59's kept scores are min 0.105 and median 0.970 on `mover-setpiece`, min 0.165 and median 0.970 on `mover-closet`. The median is the saturated maximum, 1 − 0.03, so a floor at it keeps nothing.

Distribution of bar-passing probes' display change (p50 / p90 / max levels): closet door closed 17.1 / 42.4 / 65.4; wall closed 9.3 / 20.3 / 70.0; wall open 3.3 / 7.9 / 30.7; room lowered 4.6 / 11.2 / 37.5; room raised 4.1 / 10.1 / 30.0. Most closed-pose depth tiles score exactly 0: the rays the mover changes move no neighbour's visibility.

**Bounce floor, `mover-setpiece`.** Affected probes per pose, then distance from the pose bounds in m (median / p95 / max). Bytes are per mover over the union of both termini: 2 × 216 B per probe plus 16 B per affinity cell.

| Floor | Wall closed | Wall open | Wall bytes | Room lowered | Room raised | Room bytes | Bounce block |
|---|---|---|---|---|---|---|---|
| none | 11,420; 12.4 / 24.9 / 30.1 | 597; 6.8 / 20.3 / 28.9 | 4.94 MB | 3,090; 6.1 / 13.2 / 32.4 | 3,154; 3.1 / 13.7 / 34.3 | 1.57 MB | 6.51 MB |
| 0.5 | 11,284; 12.4 / 24.9 / 30.1 | 571; 7.2 / 20.4 / 28.9 | 4.88 MB | 3,088 | 3,150 | 1.57 MB | 6.45 MB |
| 1 | 11,061; 12.3 / 24.9 / 30.1 | 562; 7.4 / 20.4 / 28.9 | 4.78 MB | 3,013; 6.1 / 12.0 / 21.5 | 3,070; 3.0 / 12.9 / 34.3 | 1.53 MB | 6.32 MB |
| 2 | 10,376; 11.8 / 24.8 / 30.0 | 545; 7.2 / 20.8 / 28.9 | 4.49 MB | 2,661; 5.3 / 9.7 / 13.7 | 2,572; 2.5 / 8.6 / 16.0 | 1.31 MB | 5.80 MB |
| 4 | 9,419; 11.1 / 24.8 / 30.0 | 142; 2.5 / 5.3 / 8.1 | 4.07 MB | 1,846; 4.4 / 8.1 / 9.9 | 1,627; 1.7 / 4.6 / 8.2 | 0.89 MB | 4.97 MB |
| 8 | 6,605; 8.8 / 22.0 / 28.3 | 58; 0.9 / 3.1 / 4.5 | 2.86 MB | 579; 3.1 / 6.1 / 7.1 | 602; 0.6 / 3.1 / 3.7 | 0.34 MB | 3.20 MB |

On `mover-closet`, the door closed keeps all 144 closet probes affected through 2 levels; their smallest change is 2.30 levels. At 3 levels 140 remain, at 4 levels 134, and at 8 levels 118. B3's closet probes stay affected at every floor up to 2.3 levels and no higher.

**Depth floor.** A probe carries mover depth when its score exceeds the floor. Live tiles are the union over movers. The CPU holds the closed-pose tiles, and the GPU holds id 59 plus live tiles. Depth bytes are (2 × 400 + 8) B per mover-probe record. Upload is per frame for that mover travelling alone: its live tiles × 400 B.

| Floor | Setpiece: wall probes / room probes | Live | Closed-pose (CPU) | GPU share of 1 << 15 | Depth block | Upload/frame wall, room | Closet probes | Closet: reference door tiles kept |
|---|---|---|---|---|---|---|---|---|
| none | 11,079 / 8,916 | 11,566 | 39,990 | 37.0% | 16.2 MB | 4.4 MB, 3.6 MB | 1,040 | 28 / 28 |
| > 0 | 573 / 731 | 1,197 | 2,608 | 5.38% | 1.05 MB | 229 KB, 292 KB | 31 | 28 / 28 |
| 0.03 | 427 / 593 | 1,006 | 2,040 | 4.79% | 824 KB | 171 KB, 237 KB | 28 | 28 / 28 |
| 0.05 (below id 59's min) | 414 / 586 | 991 | 2,000 | 4.75% | 808 KB | 166 KB, 234 KB | 28 | 28 / 28 |
| 0.105 (id 59's min kept) | 364 / 571 | 935 | 1,870 | 4.58% | 755 KB | 146 KB, 228 KB | 24 | **24 / 28** |
| 0.3 | 359 / 550 | 909 | 1,818 | 4.50% | 734 KB | 144 KB, 220 KB | 18 | **18 / 28** |
| 0.97 (id 59's median) | 0 / 0 | 0 | 0 | 1.72% | 0 | 0 | 0 | **0 / 28** |

**Closet hold.** The 28 reference-tiled door probes score 0.065–0.199 (10 probes) or 0.97 (18) against the mover-absent moments. Their reference scores, against the static door's own moments, all passed 0.1. A floor at id 59's minimum, 0.105, drops 4 of them, and the reference closet no longer holds by the brief's rule. Every floor below 0.065 keeps all 28.

**Captures at the middle values** (bounce 1 level, depth 0.105) and at the recommended values (bounce 2 levels, depth 0.05), dark rule, backface on. On the room views both floors render identically to the unfloored dark + backface build: interior 7.06, upper 4.60, west face 38.95, station 2 34.76 / 34.76 against 34.76. On the closet they render identically to the unfloored posed closet: station 2 0.123 (0.009 with reflections off), station 3 0.082 (0.032), door face 0.00. The 4 door probes the 0.105 floor drops move no station mean, but they fail the reference rule. No variant differs in these views, so none were saved. What the bounce floor gives up lies past 13–16 m from the room, outside every station.

**Recommendation.**
- **Depth floor 0.05.** It is the largest round value that keeps every reference-tiled closet probe. It brings the setpiece's GPU tiles from 37% to 4.75% of the 15-bit range, its depth block from 16.2 MB to 0.81 MB, and a travelling room's live-tile upload from 3.6 MB to 234 KB a frame. Most of the cut comes from dropping zero-score tiles; 0.05 trims 17% more.
- **Bounce floor 2 levels.** It is the largest floor that keeps B3's closet probes, and it cuts the raised room's reach from 34 m to 16 m. It does not localize the sliding wall. Closed, the wall splits the map in two rooms, and its change is genuinely visible map-wide: median 9.3 levels, 10,376 probes at 2 levels, 9,419 at 4. A bounce block for the wall stays at 4.5 MB under any floor that keeps the closet. If that is over the resident byte bar, the lever is not this floor but streaming the bounce payload with id 50, as the brief's "PRL section" Decision already provides.

### M1: bounce-bake cost on the full hallway (2026-11-08)

The setup: `stress-warren-hallway-inspection.map`, with 11 movers and 1,748,534 patches. The machine is a 16-thread i9-9980HK MacBook Pro with 32 GB, an Intel Mac. Runs were at `-j 6` under `nice -n 10` and `caffeinate -i`, with peak RSS from `/usr/bin/time -l`. The owner was at the machine, and load averaged 3–9 during the runs. The baseline is the parent compiler `bin/prl-build-head`. The slice's bounce bake is not priced in the SH gate yet (task 7), so both compilers log the same prediction.

| Run | SH Bake | Total | Peak RSS | Footprint | SH gate |
|---|---|---|---|---|---|
| Parent, cold `--no-cache` | 213.6 s | 1,241 s (20.7 min) | 7.12 GB | 4.42 GB | est. peak 4.99 GB of 17.2 GB, admitted |
| Slice, cold `--no-cache`, measurement over all 11 movers (22 poses) | — | **killed at 45 min, still in the mover bake** | — | — | same line |
| Slice, cold `--no-cache`, 2 movers measured (`warren_door_0` = `#0`, `warren_lift_0` = `#9`, 4 poses) | 1,084.9 s | 2,150 s (35.8 min) | 7.06 GB | 4.44 GB | same line, admitted |
| Parent, warm, one-mover edit (`warren_door_0`'s last waypoint 32 units down, cache primed by the parent on the unedited copy) | 0.84 s (`sh_base_volume hit`) | 108 s | 6.20 GB | 4.58 GB | same line |
| Slice, warm, the same edit, re-baking only the edited mover (the per-mover memo's work) | 428.8 s (`sh_base_volume hit`) | 544 s | 5.84 GB | 4.38 GB | same line |

- **One pose costs about one whole id-34 bake.** Six poses measured: bounce one 0.3–0.6 s, later bounces 128–131 s, gather 84–86 s, about 216 s a pose against id 34's 214 s. A door and a lift cost the same: the later bounces run over every static patch, and the gather over every valid probe, whatever the mover's size.
- **Cold, all 11 movers (extrapolated, not run to the end).** 22 poses × 216 s ≈ 4,750 s on top of the parent's SH Bake: SH Bake ≈ 4,960 s, 23× the parent's. The build totals about 100 min against 20.7.
- **Warm one-mover edit.** 429 s of SH Bake against 0.84 s. On an id-34 hit, resolving the cached layers and summing them is negligible; the two poses are the whole cost. The build totals 544 s against 108 s, 5.0×.
- **Peak RSS does not grow.** 7.06 GB against 7.12 GB cold, and 5.84 GB against 6.20 GB warm, both within run-to-run noise. The process peak is set outside the SH stage, and the bounce bake holds one pose's patch radiance at a time (≈ 1.75 M patches × 2 sides × 12 B plus three later-bounce buffers, about 0.1 GB). The SH gate's prediction is unchanged, so it does not yet cover the bounce bake, as P21 requires.
- **For the owner's cost gate.** The brief's fallback, "re-bake bounce one per pose and keep id 34's later bounces", removes the 130 s later-bounce term per pose. That leaves the ~85 s gather: about 60% off, but still ~1,900 s cold and ~170 s per warm mover edit. A gather restricted to probes the pose can affect would bound the rest. That needs a conservative reach domain, the open drift question, which the floors above do not answer before the gather.

### M7: bytes and tiles at the middle floor values (2026-11-08)

These are measurement-mode runs over every mover at both termini, at bounce floor 1 level and depth floor 0.105 (id 59's kept minimum). They are cold `--no-cache` at `-j 6`, and section sizes come from the same PRL. The bounce block is 2 × 216 B per affected probe plus 16 B per affinity cell, over each mover's union of termini. The depth block is (2 × 400 + 8) B per mover-probe record. The CPU holds the closed-pose tiles, and the GPU holds id 59 plus live tiles.

| Map | Movers (poses) | id 34 | id 59 | Bounce block | Depth block | Live tiles | Closed-pose (CPU) | id 59 + live, share of 1 << 15 | SH Bake (measured, slice) |
|---|---|---|---|---|---|---|---|---|---|
| `mover-closet` | 1 (2) | 69 KB | 9 KB (23) | 62 KB | 19 KB | 24 | 48 | 0.14% | — |
| `mover-setpiece` | 2 (4) | 531 KB | 228 KB (565) | 6.32 MB | 755 KB | 935 | 1,870 | 4.58% | — |
| `kinematic-platform` | 5 analysed (10) | 4.47 MB | 11 KB (28) | **15.3 MB** | 111 KB | 124 | 274 | 0.46% | 127 s |
| `campaign-test` | 3 (6) | 4.75 MB | 363 KB (898) | 14 KB | 166 KB | 206 | 412 | 3.37% | 66 s |
| `stress-warren-showcase` | 8 (16) | 18.5 MB | 1.78 MB (4,410) | 6.99 MB | 1.82 MB | 2,237 | 4,508 | 20.3% | 1,452 s (total 2,089 s, peak RSS 5.42 GB) |
| `stress-warren-hallway-inspection` | 11 (22) | 44.0 MB¹ | 5.4 MB (13,451) | **not run: over 45 min** (≈ 100 min extrapolated, M1). 2 of 11 movers: 8.5 MB | 2 of 11: 1.30 MB | 2 of 11: 1,615 | 2 of 11: 3,230 | 2 of 11: 46.0% | 4 poses: 1,085 s |

¹ From the on-disk `stress-warren-hallway-inspection.prl` of 2026-11-06. It predates id 59, so the id-34 size is indicative only. Id 59's count is this session's cold bake.

- **The bounce floor does not localize on open maps.** On `kinematic-platform`, `dev_platform` at 1 level still admits 8,492 of 138,886 valid probes, at a median 23 m and up to 95 m away. Its bar-passing changes sit at a median 2.0 levels, because the map is dim and sRGB is steepest near black. At 2 levels the reach is p95 50–80 m and the block 8.3 MB. At 4 levels it is p95 8–14 m (two movers' tails reach 45–50 m) and the block 1.9 MB. At 8 levels it is p95 ≤ 5 m and the block 0.44 MB. The closet's B3 caps the floor at 2.3 levels (above), so no single display floor both holds the closet and localizes these platforms.
- **The depth floor localizes everywhere.** Live tiles stay under 2,300 a map wherever every mover was measured. The hallway is the exception. Its own id 59 already holds 41% of the range, and `warren_lift_0` alone adds 1,323 live tiles at 0.105. Eleven movers at that rate stay within the range (rough extrapolation: under 10,000 live tiles in all), with no id-59 trim.
- **Direct-half byte column (q bytes and carried T): not derived.** Task 1's joint visibility runs only under the pose switch, for one mover and terminus, inside the id-35/41/45 bakes. A per-(mover, terminus, light, probe) count needs the q bake task 6 builds. It is not cheap from what exists.
- **Bright-room probes on `mover-closet` storing a bounce entry: none** at any floor. All 144 affected probes are closet probes, 3.5 m median and 6.1 m max from the door's bounds.

### M7 direct-half column (task 6, 2026-11-08)

These are cold `--no-cache` builds at `-j 6` under nice, with the production q bake and the dark inside-brush rule. Bake-only lights are not gated yet (task 6b), so the counts below leave them out.

| Map | Gated lights with entries | Entries (own tile) | Direct block | ÷ id 35 | ÷ (id 41 + id 45) | id 57 total | Build |
|---|---|---|---|---|---|---|---|
| `mover-closet` | 0 of 3 | 0 | no section | — | — | — | 2 s |
| `mover-setpiece` | 10 of 10 | 519 (393) | 4,086,210 B | 9.97× | 1.29× | 4,092,597 B | 14 s |
| `kinematic-platform` | 6 of 17 | 11 (0) | 1,188 B | 0.04% | 0.025% | 52,035 B | 32 s |
| `campaign-test` | 2 of 16 | 50 (0) | 4,496 B | 0.30% | 0.057% | 110,339 B | 117 s |
| `stress-warren-showcase` | 14 of 49 | 223 (0) | 20,398 B | 0.17% | 0.026% | 235,561 B | 540 s, RSS 5.98 GB |
| `stress-warren-hallway-inspection` | 6 of 344 | 20 (0) | 1,618 B | 0.006% | 0.002% | 577,717 B | 887 s, RSS 7.02 GB |

- Setpiece's direct block is 4,029,480 B of own tiles, 50,502 B of q and 6,228 B of records.
- On big maps, id 57's size is mostly the dense cell index.
- The direct bake took 0.7–312 ms wall time on every map.
- Setpiece's base bricks go from 351/42/3 to 380/13/3 (L0/L1/L2) under the channel caps.

After task 6b (bake-only lights gated through namespace 2), same method:

| Map | Gated lights with entries (bake-only) | Entries (own tile) | Direct block | ÷ id 35 | ÷ (id 41 + id 45) | id 57 total |
|---|---|---|---|---|---|---|
| `mover-setpiece` | 10 of 10 (0 of 0) | 519 (393) | 4,086,210 B | 9.97× | 1.29× | 4,092,597 B |
| `kinematic-platform` | 9 of 21 (3 of 4) | 30 (19) | 200,232 B | 6.6% | 4.1% | 251,079 B |
| `campaign-test` | 2 of 16 (0 of 0) | 50 (0) | 4,496 B | 0.30% | 0.057% | 110,339 B |
| `stress-warren-showcase` | 20 of 85 (6 of 36) | 295 (72) | 600,052 B | 5.1% | 0.76% | 815,215 B |
| `stress-warren-hallway-inspection` | 49 of 763 (43 of 419) | 1,745 (1,725) | 19,589,126 B | 73.9% | 25.8% | 20,165,285 B |

On the hallway, 19.39 MB of the direct block is own tiles. Task 6c revises the format.

After task 6c (epoch 2: sparse cells, shared per-(cell, light) tiles, probe masks), same method. Columns are index / records / q / tiles → direct block, then id 57's total.

| Map | Before (6b) | After (6c) | id 57 after |
|---|---|---|---|
| `mover-setpiece` | 4,756 / 6,228 / 50,502 / 4,029,480 → 4,090,966 | 4,848 / 9,964 / 28,842 / 2,310,984 → 2,354,638 | 2,355,401 |
| `kinematic-platform` | 38,092 / 360 / 2,880 / 196,992 → 238,324 | 404 / 600 / 258 / 18,360 → 19,622 | 19,749 |
| `campaign-test` | 79,348 / 600 / 3,896 / 0 → 83,844 | 948 / 1,000 / 1,352 / 0 → 3,300 | 3,531 |
| `stress-warren-showcase` | 161,284 / 3,540 / 23,032 / 573,480 → 761,336 | 3,864 / 5,708 / 8,616 / 198,720 → 216,908 | 217,707 |
| `stress-warren-hallway-inspection` | 432,004 / 20,940 / 180,890 / 19,387,296 → 20,021,130 | 17,456 / 34,900 / 102,534 / 11,056,176 → 11,211,066 | 11,213,837 |

- On setpiece, 519 entries became 493 records over 179 cells, with 13,762 masked probes.
- On the hallway, each of the 1,725 records has a single mover, so the masks do all the work: 51,267 probes are masked.
- Ids 34, 35, 41, 45, 49 and 50 are byte-identical before and after on every map.

### Task 7a: bounce and depth bake, measured (2026-11-09)

Cold builds, pre-7a compiler (9b5d4d241) → 7a. The pre-7a timings for campaign-test and showcase ran beside other work, so read them as rough.

| map | cold total | id 57 bytes | bounce block (payload) | depth block | live tiles / CPU-held tiles |
|---|---|---|---|---|---|
| mover-closet | 3.5 → 2.5 s | 0 → 85,095 | 62,332 (62,208) | 22,628 | 28 / 56 |
| setpiece | 16.1 → 20.9 s | 2,355,401 → 8,963,433 | 5,798,768 (5,790,960) | 808,004 | 991 / 2,000 |
| kinematic-platform | 45.5 → 124.8 s | 19,749 → 8,450,017 | 8,299,788 (8,259,840) | 111,508 | 125 / 276 |
| campaign-test | 45.7 → 109.7 s | 3,531 → 196,999 | 4,572 (4,320) | 189,076 | 229 / 468 |
| showcase | 809 → 1,656 s | 217,707 → 8,305,807 | 6,048,728 (6,037,200) | 2,033,740 | 2,483 / 5,034 |

- Warm builds after moving one waypoint up 8 units hit `sh_base_volume` and re-bake one mover's bounce: setpiece 4.4 s (1 of 2 movers), showcase 168 s (1 of 8).
- Each showcase mover's bounce costs about 120 s whatever its affected count (SH Bake 66 s → 1,032 s). Posed later bounces gather over all 829k static patches, 4 bounces × 2 poses.
- SH gate: the closet predicts 13.0 MB cold against 3.1 MB measured (4.2×); other modes measure 1.4–2.4 MB. Showcase is admitted at 9.87 GB predicted (pre-7a: 935 MB), driven by the direct-entry bound ×2 plus the states.
- Floors: setpiece stores 95% of valid probes (mover 0: 11,313 of 11,942).
- Sections changed against 9b5d4d241: closet 34, 35, 49, 50, 54, 57; setpiece, platform and showcase 34, 35, 49, 50, 57; campaign-test 57 only. Ids 34, 35, 49 and 50 move because bounce cells cap base storage at L0. Id 54 moves only on the closet, whose reflection probes shade from id 34. Id 59 is unchanged everywhere, since no map's live tiles reach its cap.
- The hallway was not measured in 7a.

### Task 7b

Measured 2026-11-09. Each map was cold-built with the pre-7b compiler (7bb4be94c) and then the 7b compiler. Both ran `--no-cache -j 12`, except the four small 7b builds, which filled an empty cache so the memo line would print. Other builds and test suites ran alongside most of these, so read the timings as rough.

**Step 0, hallway pilot (pre-7b compiler).**
- The default 16 GiB budget refused the hallway at a predicted 39.51 GB. Of that, the direct-entry bound was about 27.25 GB, on top of the 12.26 GB mover-bounce phase.
- Under `--working-set-max-size 64GiB` it built in 6,283 s, with 8.95 GB max RSS and a 4.98 GB peak footprint, about 4.4× under the prediction.
- Id 57 was 34,668,329 B: a direct block of 11,220,426 B (payload 11,158,710 B) and a bounce block of 19,099,904 B (payload 19,074,960 B). Both payloads exceed the 8 MB bar.
- `cost.rs` now prices each (light, mover, cell) that the bake's own prefilter admits: `light_movers`, then `may_meet` at each valid probe within the light's reach. Each one is costed at that cell's valid-probe count.
- The 7b compiler admits the hallway at the default budget, predicting 13.71 GB against 7.17 GB max RSS and a 4.47 GB footprint. That is 1.9× RSS and 3.1× footprint; both are whole-process figures, while the gate prices only the SH-base phase.
- Showcase drops from 9.87 GB to 5.10 GB.
- The bound still covers what the bake holds on all six maps (`fixture_cost_tests`). The stress maps run on demand.

| map | bound | held |
|---|---|---|
| closet | 97,034 B | 0 B |
| setpiece | 14.1 MB | 5.6 MB |
| platform | 1.41 MB | 0.40 MB |
| campaign-test | 12.8 MB | 0.80 MB |
| showcase | 176 MB | 3.6 MB |
| hallway | 530 MB | 21.8 MB |

**Placement, pre-7b → 7b.**

| map | id 57 bytes | streams | id 50 bytes added | cold |
|---|---|---|---|---|
| mover-closet | 85,095 → 85,096 | neither | 0 | 14 → 3 s |
| setpiece | 8,963,433 → 8,963,434 | neither (direct 2,339,826, bounce 5,790,960) | 0 | 40 → 28 s |
| kinematic-platform | 8,450,017 → 8,450,236 | neither (bounce 8,259,840) | 0 | 188 → 172 s |
| campaign-test | 196,999 → 197,000 | neither | 0 | 183 → 102 s |
| showcase | 8,305,807 → 8,305,816 | neither (bounce 6,037,200) | 0 | 3,158 → 2,450 s |
| hallway | 34,668,329 → 4,434,660 | both (direct 11,158,928, bounce 19,074,960) | 44,233,712 | 6,283 → 5,245 s |

- Under both bars, only id 57 changes, and ids 49 and 50 stay byte-identical to pre-7b.
  - Every map gains 1 B of header for the direct placement byte (epoch 3).
  - The τ fix below adds the rest. Setpiece changes 19 q bytes and campaign-test 14 (one area sample each, 8 to 48 levels).
  - Platform gains one masked probe: +2 B of visibility and +216 B of own tile.
  - Showcase gains four masked probes: +8 B of visibility.
  - Closet's id-57 body is identical, since it has no direct payload.
- The hallway streams both blocks. Id 57 keeps its cell index, records, masks, mover ids and the depth block.
  - Id 50 grows from 362,223,360 B to 406,457,072 B. That is 30.2 MB of payload plus 14 MB of overhead: the row tables, plus every cell carried by each cluster that covers it, as sparse rows already are.
  - Id 49 grows from 6,296,320 B to 6,413,080 B, from the id-57 resource rows and affinity ranges.

**Memo, task 6d's dense payload → valid probes only** (`direct_sh_delta_light` + `animated_direct_sh_delta_light`, per cache generation):

| map | dense | valid probes only |
|---|---|---|
| campaign-test | 46.4 MB (26.7 + 19.7) | 14.9 MB (9.3 + 5.6) |
| setpiece | 8.29 MB | 4.37 MB |
| platform | 9.23 MB | 7.28 MB |
| closet | 3.73 MB | 1.09 MB |

**B7 direct clause.**
- The first run missed by one pair. Of 1,025 blocked (probe, light) pairs across 224 interior probes and 7 lights, one was 8.4 q steps off: probe 3433, light 2, q₁ 120 against the reference's 111.6.
- Exactly one of 32 area samples passed 0.1–0.5 mm outside the raised room's doorway jamb. The static tracer's τ = 0.5 mm blocks it; the direct half's 0.1 mm contact band did not.
- Fix: the direct half's posed shadow segments now meet a face within τ (`SHADOW_SEGMENT_BAND_M`), and `mover_probe_direct` is now v2. Worst case is now 0.50 q steps, which is quantization.
- The bounce half's patch-direct segments keep the gate's band.

### Task 7c

Measured 2026-11-09. Each map was cold-built (`--no-cache -j 12`) with the 7b compiler (HEAD 7d646f223) and then the 7c compiler, on the main checkout with tests running alongside, so timings are rough.

**What the static reference uses.** The static-brush reference's direct bake (ids 35, 41, 45 and the probe bake behind them) traces each shadow segment with `ray_traversal::segment_clear`. That is the watertight triangle test with τ = 0.5 mm, from a start lifted `RAY_EPSILON` along the segment to its end. The SH base gather and the patch-direct shading use the same hit test, so the static side has one predicate for gather rays and shadow segments.

**Change.**
- `ray_traversal::ShadowSegment` is the static tracer's own segment test: lift, far bound and `TraceRay::hit` with τ. `segment_clear` runs it over the tree.
- `mover_probe_channel::PosedSolid` owns every posed trace. It holds the pose's half-spaces and its sides cut where the mover's other brushes cover them, and it has two tests:
  - `nearest_hit` for gather rays, moved from 7a's `PosedScene::trace`.
  - `segment_blocked` for shadow segments: `ShadowSegment` over the posed triangles. A lifted start inside a posed brush is blocked, which is the dark inside-brush rule.
- The direct half's `PosedOccluder` and the bounce half's patch-direct `PosedScene::segment_blocked` both call `segment_blocked`. `PosedMovers` builds each pose's solid once per half.
- 7b's plane growth (`SHADOW_SEGMENT_BAND_M`, `segment_touches_brush_within`) is gone. The mover light gate keeps its own 0.1 mm contact rule; it is not a channel trace.
- Memo stages: `mover_probe_direct` v3, keyed on the solid's triangles; `mover_probe_bounce` v2.
- A unit test pins the predicate against a unit box built as static triangles: a segment and a ray 0.3 mm outside an edge block, 0.7 mm outside are clear, an oblique segment past a corner edge blocks, and a segment parallel to a face 0.3 mm off it is clear inside the face's footprint and blocked across the side planes. Plane growth blocks the in-footprint parallel segment that the static tracer passes.

**Proof rows, 7b → 7c.** B3: worst |composed − reference| 0 against bar 0, 144 probes, before and after. B4: 28 of 40 reference tiles carried, before and after. B7 base: 224 interior probes, median 0.0747 and max 2.78 display levels (bar 3), max share of today's distance 0.284, unchanged to the printed digits. B7 direct: 1,025 blocked pairs over 7 lights, worst 0.500011 q steps, unchanged. B8 passes: closet 191, setpiece mover 0 11,313 and mover 1 3,154 probes past the rule, all stored. B16 (`mover_probe_channel_cache`, 10 tests) and B17 (against `t6/prl-build-parent`) pass.

**Bytes.** Only id 57 changes, on every map.

| map | id 57 | direct | bounce |
|---|---|---|---|
| mover-closet | identical | — | identical |
| mover-setpiece | 8,963,434, same size | 8 q bytes, each +8 (one more of 32 samples clear), all at terminus 1 | 980 of 13,405 stored probes move, ≤ 6.9e-5 absolute, ≤ 0.09% of the tile's peak |
| kinematic-platform | 8,450,236 → 8,450,466 | one masked probe dropped (q₀ 247 → 255, −2 B q, −216 B own tile) | 19,120 → 19,121 affected probes (3 enter, 2 leave at the floor), 5,386 shared probes move, ≤ 1.1% of the tile's peak; payload 8,260,272 B, still under the 8 MiB bar |
| campaign-test | 197,000, same size | 4 q bytes, each +8 | identical |

- Every direct change raises q by one sample: plane growth blocked samples the static tracer passes (corners, where a grown plane reaches past τ, and near-parallel samples). Platform's dropped probe undoes 7b's one added masked probe by size (2 B of q, 216 B of tile); its identity was not checked.
- The bounce changes are the patch-direct segments moving from the 0.1 mm band to τ. No listed cell is added or dropped, so ids 34, 35, 49 and 50 stay byte-identical.

### Task 8

Measured 2026-11-09 on the main checkout, AMD Radeon Pro 5300M under Metal.

**Storage.** One level table at binding 25 serves both direct compose passes.
- Pass A goes from 6 to 7 storage buffers. Pass B stays at 8 of 8 because the table also holds id 45's animation descriptor map, which was Pass B's own read-only `u32` array with the same lifetime. That is the "merge matched read-only arrays" route.
- The table holds a header, the listed cells, per-cell record ranges (static records first, then animated-baked ones), 8-word records, mover entries, the t′ block, the descriptor map, one T per masked probe, and the q bytes.
- T is copied per masked probe from the record's own tile, its id-41 entry or its id-45 entry. So Pass B can gate static lights in the promotion-free atlas (Case 2) without an extra intermediate texture or id-41 bindings.
- The ledger kind is `mover_channel_table`. On the setpiece the table is 3,023,680 B: 13,762 masked probes × 216 B of T, plus 28,842 B of q.
- A streamed direct payload gets a full-sized table at load. Each cell is written once, by the first drained cluster that carries it. The table is not priced per residency, so streaming does not shrink it. On the hallway it would be about the size of the 11.2 MB direct payload plus the borrowed T copies. Pricing it against the pool is still open.

**Missing entries.** If id 41 or id 45 is absent or disabled at load, a record that borrows from it composes ungated, and the load logs a warning. If the family is present but the record's cell is coarser than L0 or holds no entry for its light, the load is refused. Bake-only records without their own tile are refused too.

**Capture index-space bug (fixed).** Capture keeps static lights only, so it renumbers the selection into compact static-light indices. Id 57's namespace-0 indices stayed global AlphaLights indices. On the setpiece, 79 of 126 unflagged records named a global index that is not a value in capture's renumbered selection, so the channel refused the load ("light 11 holds no selection slot"). Capture now renumbers id 57's namespace-0 indices with the same table (`capture_mover_probe_channel`, unit-tested). The game path keeps both lists global, so it never hit this.

**Captures: smooth against hard backface.** Method:
- `--features capture`, `POSTRETRO_SH_STREAMING=sync-proof`, full-resident compose, 1280×720.
- `setpiece_room` parked raised. A temporary hook in `park_dev_pose_mover` hands the renderer the parked waypoint's t (1), so the channel composes gated.
- `POSTRETRO_DEV_MOVER_BACKFACE` now takes `none` (no backface weight, the pre-Stop-2 contract) or any other value (the hard form). Unset ships the smooth form.
- Means are of 8-bit Rec.709 luma with alpha dropped, 0–255.
- The map is the production 7c bake, so absolute values do not compare with §Captures, which used the slice bakes and alpha-included means. Compare columns, not tables.
- Views: interior and upper walls as in §Captures (engine (−8.128, 1.829, −21.844)). West face is a new pose, (−8.128, 2.286, −15.748), yaw 0, fov 60, framing the doorway wall, because Task 1's pose is not recorded. Station 2 is the README's.

| View | Smooth (ships) | Hard | None |
|---|---|---|---|
| Interior | 11.79 | 11.73 | 11.79 |
| Upper walls | 7.69 | 7.22 | 8.06 |
| Upper, direct only (mask 24) | 0.681 | 0.554 | 0.785 |
| Upper, indirect only (mask 6) | 7.00 | 6.64 | 7.28 |
| West face | 28.22 | 29.34 | 21.13 |
| Station 2 | 34.44 | 34.73 | 32.41 |

- **The smooth form leaks more than the hard form**, but less than no backface weight. On the upper walls, direct light reads +0.127 luma over hard (+23%) and bounce +0.35 (+5%), so the full frame reads +0.47 (+6.5%). It closes about 45% of the gap from none to hard, in both terms. The ×20 row of `captures/t8-backface-sheet.png` shows the extra direct light as soft patches on the ceiling and upper walls.
- **Own faces.** The west face reads 28.2 smooth against 29.3 hard and 21.1 with none. Station 2 reads within 0.3 of hard.
- Sheet: `captures/t8-backface-sheet.png`. Columns are smooth | hard | none. Rows are interior, upper walls, upper direct ×20, upper indirect, west face and station 2.

**Per-frame cost: not measured.**
- `POSTRETRO_GPU_TIMING=1` reports that this adapter lacks `TIMESTAMP_QUERY` and/or `TIMESTAMP_QUERY_INSIDE_ENCODERS`.
- A windowed setpiece run stalled at "Window ready" for 45 s and was stopped.
- What the structure bounds:
  - CPU: one pass over the movers' states per frame, and an upload of 4 B × movers only when some t′ changed. A unit test pins it as allocation-free.
  - Legacy path: every frame in which any gated mover's t′ moves re-dispatches all of Pass A and Pass B, so a travelling mover keeps the whole direct compose running.
  - Streamed path: only the travelling mover's rows recompose.
  - Each table probe costs one `q` read per mover and one T read per record. The binary search over listed cells runs once per workgroup.
- The 5300M budget (M6) stays open.

### Task 9

Built 2026-11-09 on the main checkout, AMD Radeon Pro 5300M under Metal, on top of task 8 (4852b8e16), uncommitted.

**Indirect compose.** `sh_compose.wgsl` applies the bounce half between the id-34 base sample and the id-27 deltas, at stored L0 probes with `INDIRECT_STATIC` on.
- The first mover with a state and a tile at the probe replaces the base tile with lerp(S^b₀, S^b₁, t′).
- Each later mover adds its lerp minus id 34, with id 34 re-sampled from the base atlas.
- If any mover touched the probe, the summed tile clamps per texel at zero. Id-27 deltas then add as before, and every reader inherits the result.
- **Clamp rule adopted as the owner leaned:** per-texel max(0) on the summed base tile, before the deltas. The sampler's octahedral taps and probe weights are convex, so non-negative texels evaluate non-negative irradiance in every direction. No fixture needed anything else.
- A mover with no state (t′ negative) is skipped, so an unaffected probe or a stateless frame composes id 34 exactly. With no section, or no bounce block, the table lists no cells and every frame is byte-identical to today's.
- Only mover state and residency reach the block, never a light's state. The states sum static lights only. The affected mask is storage sparsity, and an unmasked probe composes id 34.

**Binding wall.** The SH compose's group 1 was at 8 of 8 storage buffers. The bounce table takes binding 25, and id 27's light → descriptor map (the old binding 25, the same read-only `u32` lifetime) folds into it at word 8. This is the same merge task 8 used for id 45 in Pass B. There is no new binding, bind group or texture format.
- `ShAllocationKind::IndirectComposeDescriptorIndices` is gone. `MoverBounceTable` (`mover_bounce_table`) records the table and cites ids 57 and 27.
- The unused descriptor plumbing in `compose_storage_payloads` is gone too.

**Direct T.** The copy per masked probe stays. Pass B is still at 8 of 8 and has no id-41 bindings, so it cannot reference id 41/45 directly.

**Residency (`mover_channel/pool.rs`, `sh_streaming/mover_cells.rs`).** A block placed Streamed gets a pooled table on a streamed level. A resident block keeps a whole table, ledgered as before.
- **Reference counting.** Each cluster carrying a listed cell's id-57 row holds one `(57, cell)` storage reference under the install journal, as a sparse row does.
  - The first reference places the cell's payload with first-fit (`FirstFitRanges`, shared with the SH pools). The payload is checked on arrival: run length, finiteness, and each record's mask tight against its q bytes.
  - The payload is written, then the cell's base word is published in the same upload batch.
  - The last release frees the range and writes `NO_BASE` in the eviction's batch, before any install can reuse it. A session reset clears both pools.
- **Not resident.** While a cell's base reads `NO_BASE`, the shaders treat it as unlisted: the cell composes as with no section. That is residency, never a light- or mover-driven switch.
- **Pricing.** Each pool starts at the dense SH pool's share of the map times the whole payload, plus 25% headroom, capped at the whole payload. A pool that runs out grows by 1.5×, capped at whole, with a warning. The live words are copied over, and every streamed compose pass rebinds (`rebind_mover_channel`).
  - The pools' allocated and live bytes join the snapshot's active capacity and logical occupancy. Through `streaming_allocation_summary`, they reach the ledger.
- **Membership and trigger.** A newly placed cell dirties its rows in both passes. Row membership and the t′ trigger reuse task 8's machinery. The streamed planner stales indirect rows from the movers' bounce rows (`bounce_by_mover`). The legacy pass fires on `bounce_t_changed_since_composed`, which only movers with bounce entries move, and commits after the encode (P12).
- **Re-parsing.** A cluster re-parses its id-41/45 rows only when a direct cell can take them, meaning a whole cell still waiting for a borrowed term or a pooled direct cell it places first.

**C.1, gated records without ids 41/45.** `gate_direct_compose` gives the streamed plan the base-only direct carrier an animated-only level already uses, whenever the channel gates direct terms and id 35 is present.
- `direct_compose_required`, the dense direct atlases, their pool pricing (`plan_initial_pool`, `DenseTextures::new`) and the carrier preflight now follow the plan's carrier rather than `sources.direct_delta || animated_direct_delta`.
- Not exercised on a real map: no fixture holds id-57 direct records without id 41 or 45. Covered by `gated_direct_records_plan_the_direct_passes_without_ids_41_and_45`.
- Open: an animated-baked record with its own tile on a level without id 45 still has no Pass B.

**C.2.** A record without its own tile, whose family is absent from the PRL's section table, refuses the load with `MoverProbeChannelBorrowedFamilyAbsent`, naming DirectShDeltaVolumes (id 41) or AnimatedDirectShDeltaVolumes (id 45). A family the load disables still composes ungated, with the renderer's existing warning.

**Proof.**

| Row | Test | Result |
|---|---|---|
| C3 | `mover_channel::bounce_compose_tests::c3_bounce_replaces_id_34_with_the_lerp_of_its_states_and_deltas_still_add` (t′ 0, 0.5 and 1; no state, or no section, equals today's bytes; nothing outside the probe moves) | pass |
| C3, two movers | `two_movers_at_one_probe_add_their_lerped_differences_from_id_34` | pass |
| C3, clamp | `the_summed_base_clamps_per_texel_at_zero_before_deltas_add` (negative state; negative sum of two movers; positive sums pass through). Fails with the max(0) removed, checked | pass |
| B11 (GPU) | Id 27 composes unchanged on top, in the C3 test with `INDIRECT_ANIMATED` off and on. The table takes no light state. | pass |
| C7, C14 (indirect) | `mover_channel::tests::planner::only_a_travelling_movers_bounce_rows_recompose_indirect`; legacy: `state.rs` `a_mover_recomposes_only_the_passes_its_entries_feed` | pass |
| C11 (indirect) | `a_bounce_change_outside_the_gate_or_unrecorded_composes_later` (streamed); legacy commit-after-encode as task 8 | pass |
| C10 (indirect) | **Deferred to task 11.** `off` composes the legacy atlas and `sync-proof` the slot-pooled one, so the comparison needs the captures' readback path. Streamed ≡ resident table words is proven below. | deferred |
| C7 GPU half (lagging rows compose before sampling) | **Deferred to task 11**, with the captures | deferred |
| Residency (GPU, synthetic) | `a_streamed_bounce_cell_composes_the_resident_bytes_only_while_resident`: not resident composes id 34; on arrival, equal to the resident build's atlas; on eviction, live 0 and id 34 again; on re-arrival, equal again | pass |
| Residency (real map) | `uploads::renderer_tests::streamed_mover_channel::a_streamed_id_57_payload_places_the_resident_builds_words_per_resident_cell` (ignored; env `MOVER_CHANNEL_RESIDENT_PRL`, `MOVER_CHANNEL_STREAMED_PRL`) on `mover-setpiece` built default and with `--mover-payload-resident-bar 0` by the same compiler | pass |
| C.1 | `sh_streaming::tests::gated_direct_records_plan_the_direct_passes_without_ids_41_and_45` | pass |
| C.2 | `prl_loader::mover_probe_channel_tests::a_record_borrowing_from_an_absent_family_is_refused_naming_the_family` | pass |

**Real-map residency (setpiece).**
- The bar-0 PRL is 55,362,439 B against 51,850,511 B resident: cells repeat in every cluster that covers them.
- All clusters resident: all 179 direct cells and 303 bounce cells place the resident build's words exactly. The pools hold 8,792,520 B live and allocated (direct 3,001,560 B, bounce 5,790,960 B). The dense pool started whole, so the pools did too.
- Logical occupancy went 0 → 13,481,880 B.
- After evicting every cluster, live is 0, every base reads not resident, and occupancy drops by at least the payload. On re-arrival, the words are identical.

**Hallway bytes, estimated, not built.** From 7b's and task 6's figures:
- Direct pool: 51,267 masked probes × 216 B of T is 11,073,672 B. Add about 0.10 MB of q, padded per cell, for ≈ 11.18 MB.
- Bounce: 19,074,960 B.
- **Resident (whole tables):** ≈ 30.3 MB, plus fixed parts of about 0.2 MB (1,725 records × 32 B, mover entries, cells, bounce entries).
- **Streamed:** the same ≈ 0.2 MB fixed, plus two pools.
  - They start at f × 30.3 MB × 1.25, where f is the dense pool's share of the map, capped at 30.3 MB.
  - Live bytes are only the resident cells' share.
  - Growth past the start is logged.
- On the setpiece, pooled direct is 1.28× its file payload (borrowed T copies), and bounce is 1.00×.

**GPU steps.** No windowed run; nothing stalled.

**Gates.**
- `cargo test` with `POSTRETRO_REQUIRE_GPU=1`: renderer 1,311 pass (11 ignored), level-loader 387, app (`postretro`) 2,513 + 3 + 2.
- Clippy `--all-targets -D warnings` is clean with and without `--features postretro/capture` on renderer, level-loader, level-format and postretro.
- `cargo fmt` applied.

**Smaller changes.**
- The total atlas carries `COPY_SRC` under `cfg(test)` as well as dev-tools, for the compose GPU tests.
- `clear_pools` is classified drain-internal in the upload drift guard.
- The planner harness now plans the indirect pass (`PlannedRows`).

### Task 10

Built 2026-11-09 on the main checkout (AMD Radeon Pro 5300M, Metal), on top of task 9 (24b4ba91c), uncommitted.

**Texture and references.** The tile texture holds id 59's tiles, then one live tile per probe carrying mover depth. `ProbeDepthTileReferences::from_section` and `depth_tile_texture_plan` take the live count, and the layout is sized from the sum. `ShDepthTiles` keeps its texture, its layout and id 59's count, which is the first live tile's index. Live slots start zero, and no probe references one until it is written.
- One count serves all three sides: `level_format::mover_probe_channel::depth_tile_texture_count(id59, id57)`. The loader's range check uses it, the renderer sizes from it, and the compiler summary prints the same total.
- Format, binding, texture format and sampler are unchanged. The depth path of `sh_sample.wgsl` hashes to main's (`the_sampler_depth_path_is_byte_identical_to_the_parents`).
- A level whose SH is unusable binds no live tiles and drops the CPU-held tiles (`discard_depth`).

**CPU side (`mover_channel/depth.rs`).**
- `MoverDepthTiles` holds the closed-pose tiles D₀ and D₁, decoded once to f32 (exact from f16).
- It also holds each probe's records in mover-slot order, each mover slot's probes, and each probe's base reference: its id-59 tile, or none.
- Live tiles are numbered in order of first appearance in the wire's (mover, probe) order. A mover's own probes therefore sit together, and a probe shared with a lower mover sits with that mover's.
- On setpiece, one travelling mover takes 8 copies (mover 0) or 19 (mover 1), against 73 and 83 with plain probe order.

**Trigger.** `MoverTBlock` gains its own observer, `take_depth_changes`, beside task 8's streamed-planner and legacy-pass observers, over the same t′ values. A slot whose t′ bits moved since the last write marks its probes dirty. Each dirty probe is then handled one of two ways:
- If no mover changing it has state, its reference returns to its id-59 tile or averaged moments, bit for bit.
- Otherwise it is re-blended from every stated mover's current t′, so a shared tile reads a held mover's t′.

Runs of consecutive dirty tiles on one tile row become one `UploadQueue::write_texture` copy at `tile_origin`. A frame allocates nothing.

**Two movers at one probe (delegated, adopted as the plan leaned).** Each texel takes the pair whose blended E[d] is smaller, i.e. the nearer occluder. Ties go to the lower mover slot.
- The pair is always one mover's lerp(D₀, D₁, t′), so the tile stays a valid moment set.
- The choice depends on t′ alone, never on hand-over order.
- No flag, zero weight or exclusion is involved. The 0.03 floor and the bilinear tap are untouched.

**When the write happens (`mover_depth_tiles.rs`).** `Renderer::drain_sh_residency` now runs the live-tile step after the streamed drain (a no-op on a whole load). Every frame entry calls it once before recording: windowed, skipped and capture frames alike. It does the following:
- Takes this frame's handed-over states and plans the frame.
- Stages the tile runs on the frame's upload batch, then moves references through the existing rewrite seam, now split into `DepthTexels` so the mover channel can borrow beside it.
- On a whole load, the reference texel is staged on the same batch. On a streamed level, the queued texels are flushed right away on their own batch (`flush_depth_tile_texels_now`); the batch submission carries the frame batch first. Without that flush, a reference moved after the drain would wait one frame, which breaks P24/P25.

Some properties follow from the structure:
- The upload batch is always submitted: by the frame, or by `flush_skipped_frame`. So the trigger commits at staging time. A frame that never reaches the drain leaves the change pending (P28).
- Compose re-reads the same state table later in the frame, so the live tiles and the direct and bounce compose share one t′ (P25).
- Install and release clear the state table and rebuild the t block. A hand-over before install, or on an unload frame, writes nothing, and no upload is pending at either boundary (P27, P30).

**Eviction and growth.** Live tiles never enter the pool. The load's reference table owns each live reference, and every streamed texel writer packs G from that table. Promote, evict, reinstall and dense-pool growth therefore all keep it. A probe whose cluster arrives later promotes straight onto its live tile.

**Ledger.**
- The `depth_tiles` row prices the layout for id 59 plus the live tiles, sourced to ids 59 and 57.
- `ShResidencyReport` gains `host_rows`, which are outside `total_bytes`. Its `mover_closed_pose_depth_tiles` row holds the CPU-held D₀/D₁ (f32 pairs, 8 B per texel), sourced to 57. Capture's JSON writes `host_rows` only when non-empty.
- The compiler's Probe depth tiles summary line now prices the texture for id 59 plus the live tiles, ending "… for N tile(s) with L live".

**Cost, setpiece** (`mover-setpiece` built by task 9, resident; dev profile, opt-level 2 with debug assertions; `a_travelling_movers_live_tile_plan_cost_on_a_real_map`, ignored, env `MOVER_DEPTH_TIMING_PRL`).

| mover | live tiles it writes per frame | copies per frame | bytes per frame | CPU blend and run assembly per frame |
|---|---|---|---|---|
| 0 (wall) | 414 | 8 | 165,600 | 0.18 ms |
| 1 (room) | 586 | 19 | 234,400 | 0.25 ms |

- Setpiece has 991 live tiles over 2 movers and holds 1.6 MB of closed-pose tiles on the CPU (f32).
- The first cut ran at 1.0 and 1.3 ms per frame. The f32 decode at load, a single-mover fast path, an integer-rounded f32→f16 conversion and row-segment copies brought it down to the figures above.
- The GPU side is the staged upload copies above. This Mac's Metal adapter has no timestamp queries, so their cost is unmeasured; M6's Metal System Trace covers it.
- Staging copies the run bytes once more into the 256-aligned staging buffer.

**Proof.**

| Row | Test | Result |
|---|---|---|
| D1 | `mover_depth_tiles::tests::a_live_tile_holds_the_blend_and_only_g_moves_to_it` (GPU, whole load: D₁ at the first hand-over, then 0.5, 0, 0.5, 1, 0.25 within one f16 ulp, D₀ and D₁ exact; untouched probes and id-59 tiles bit for bit); `mover_channel::depth::tests::a_live_tile_blends_its_movers_closed_pose_tiles_exact_at_both_termini` (CPU, rising and after reversal) | pass |
| D2 | Texel clauses: `tiles_are_written_only_on_change_and_no_state_keeps_todays_texels` (stripped id 57 installs id 59's texture alone and today's texels; with id 57 and no state, every texel equals the stripped install's bit for bit); `a_probe_keeps_its_id_59_reference_without_state_and_returns_to_it` (CPU). Frame clauses ("renders byte-identical", "within 2 levels of the same PRL with no depth block"): **deferred to task 11**, because they need an offscreen frame of a compiled map, which is the capture harness | texels pass; frames deferred |
| D3 | `tiles_are_written_only_on_change_and_no_state_keeps_todays_texels` (GPU: repeated t′ writes 0 tiles; the traveller writes its 3; probe 20, held mover only, is untouched; the shared tile reads the held t′); `only_a_travelling_movers_tiles_are_written_and_shared_ones_read_the_held_t` (CPU); `state::tests::depth_changes_report_each_t_change_once_on_their_own_trigger` | pass |
| D4 (P24) | Whole load: `a_live_tile_holds_the_blend_and_only_g_moves_to_it` (no state: every texel id 59's; first hand-over at terminus 1: references and D₁ on that frame). Streamed: `…::mover_depth_tile_tests::a_streamed_first_hand_over_points_resident_texels_at_written_tiles_that_frame` (resident texels on the live tile with no further drain; a non-resident probe's tile is written, and its texel waits for promotion). "Within 2 levels of the frame after": **deferred to task 11** (frame pair) | pass; frame clause deferred |
| D5 (P25) | Streamed 1 → 0 step in `a_streamed_first_hand_over…` (tiles hold D₀ and texels point at them); whole-load 1 → 0 in `a_change_no_frame_wrote_lands_on_the_next_recording_frame`. Compose and tiles share one t′ by structure (the drain boundary precedes compose, from one state table). Frame clauses: **deferred to task 11** | pass; frame clauses deferred |
| D6 (P26) | `two_movers_at_one_probe_write_it_once_with_the_nearer_blend_per_texel` (CPU: one copy, order-independent bytes, nearer pair); shared probe 9 in the D3 GPU test (held mover's t′) | pass |
| D7 (P27) | `a_relevel_writes_its_own_tiles_and_an_unload_frame_writes_nothing` (a pre-install hand-over is dropped; a relevel reusing mover 4 at A's last t′ writes B's own D₁; B's probe 3 stays on its id-59 tile); task 4's `mover_light_states_store_only_while_a_level_is_installed…`. The app-level restart re-seed is the C9-style app test in task 11 | pass |
| D8 (P28) | `a_change_no_frame_wrote_lands_on_the_next_recording_frame` (whole load: a change on a frame with no drain is written on the next frame; a skipped frame's staged write lands with its flush). The streamed path uses the same trigger and boundary | pass |
| D9 (P30) | `a_relevel_writes_its_own_tiles_and_an_unload_frame_writes_nothing` (t′ handed over, then unload: `assert_empty` after unload and after install, and no write in either level) | pass |
| D10 (P29) | `live_tiles_without_id_59_size_the_texture_alone` (whole load, id 59 absent and zero-tile: texture for 4 live tiles, probes reference live tiles holding D₀; id 57 stripped too: 1×1 dummy, 0 tile bytes); `a_streamed_level_without_id_59_points_at_live_tiles_alone` | pass |
| D11 | `a_live_tile_holds_the_blend_and_only_g_moves_to_it` (at 0, 0.5 and 1: R = id-34 E[d], B/A keep the valid word, G = the live reference); `the_sampler_depth_path_is_byte_identical_to_the_parents` (FNV-1a pin of the depth constants and functions, equal to main's) | pass |
| D12 | `a_frame_writing_two_travelling_movers_tiles_allocates_nothing` (alloc probe over the whole live-tile step, staging included); `a_frame_keeps_every_depth_buffer_at_its_capacity` (CPU plan, alloc probe) | pass |
| D13 | `live_references_survive_eviction_reinstall_and_pool_growth` (streamed: evict keeps G under the invalid word; reinstall; dense-pool growth; a later cluster promotes onto its live tile; tile bytes unchanged). The whole load has no residency writer. The next load has no live tile: the D7 test and the existing `a_rewritten_reference_stays_with_the_load_it_was_made_on` | pass |
| D14 | `tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level` (row = layout bytes for 2 + 4 tiles, sources 59 and 57; host row outside the total; gone after unload). Summary equality: `mover_probe_channel_depth_cap::live_tiles_past_the_range…` (summary bytes = `probe_depth_tile_texture_bytes(loaded count)`) and `depth_tiles_tests::the_summary_reports_the_tile_texture_bytes_the_runtime_allocates` | pass |
| B5 (loader) | `prl_loader::mover_probe_channel::tests::a_prl_whose_id_59_and_live_tiles_fill_the_range_exactly_loads_and_one_more_is_refused` (synthetic 32×32×17 PRL: 16,384 + 16,384 loads, and `depth_tile_texture_count` = 1 << 15; one more is `TileRange`); compiler count = loader count in `live_tiles_past_the_range_fail_by_name_and_a_range_exactly_full_packs_and_loads`; renderer side in render-cpu `id_59_and_live_tiles_exactly_at_the_range_lay_out_and_the_last_is_referenceable` | pass |
| B13 (depth) | The D14 test | pass |

**Carried from task 9, not built: Pass B for an own-tile animated record on a level without id 45.** Running Pass B there would compose wrong output:
- Pass B gates an animated-baked light by subtracting (1 − Q)·T·curve from the term id 45 added.
- Id 35 holds static lights only. With no id 45, nothing ever added that light's term.
- So the pass would subtract a term no pass added, darkening every other light at those probes; the store clamps the result at zero.
- When does id 45 go missing while a namespace-1 record keeps a non-zero T? The compiler omits an all-empty id 45. Id 45 bakes unit radiance wherever the light reaches, so an animated record with a T means id 45 has entries. That leaves the loader's over-floor disable, and any compiler withholding past its aggregate cap.
- In that case the light has no direct-SH term at all. Composing nothing matches the existing rule that "a family the load disables composes ungated".
- Making the record compose its gated term would mean adding Q·T·curve where no id-45 entry exists. That brings in light the disabled family otherwise lacks, at masked probes only.
- Pass A's case, task 9's `gate_direct_compose` fix, differs: the static light is in id 35, so subtracting is right there.

→ Executor or owner decision. Proposed: keep today's behaviour and pin it with a test, or have the loader refuse it.

**Gates.**
- `cargo test` with `POSTRETRO_REQUIRE_GPU=1`: renderer 1,331 passed (12 ignored), postretro 2,513 + 3 + 2, level-format 744, level-loader 388, render-cpu 794.
- prl-build bin `depth_tiles` 14; integration tests `mover_probe_channel_depth_cap` 2 and `probe_depth_tiles_compile` 1.
- Clippy `--all-targets -D warnings` is clean with and without `--features postretro/capture` on renderer, level-loader, level-format, render-cpu, postretro and level-compiler. `cargo fmt` applied.
- No windowed run; nothing stalled.

**For task 12 (docs).**
- `rendering_pipeline.md` §4: live tiles are built; the depth-tile texture holds id 59's tiles and then the live tiles.
- The live-tile step at the drain boundary, and the streamed immediate texel flush.
- The two-mover rule.
- The `host_rows` ledger.
- The summary line's live count.
- `drain_sh_residency`'s new second duty.

### Task 11

Built 2026-11-09 on the main checkout (AMD Radeon Pro 5300M, Metal), on top of task 10 (b5e2f8350), uncommitted. Maps compiled with the release `prl-build` at the working tree (closet, static-door closet, set-piece); the capture tests compile their own fixtures through `compile_dev_map`.

**Hand-over.** `KinematicMoverRenderCollector::hand_over_light_states` collects the post-tick (mover id, t′) list and hands it to the renderer in one call. All three frame paths use it: the play frame (`frame_loop`), the held Settling frame (`held_level_visibility`), and now the plain world-less frame (Loading, Frontend), which used to skip it. With no level it hands over an empty list. The renderer setter, the collector and the state type are unchanged, so the B19 grep gate still reads them.

**Capture forced mover t.** Scene field `force_movers: [{ "name", "t" }]` (t in [0, 1], before the ramp; names unique, non-empty). `capture/forced_movers.rs`:
- Sets each named mover's phase through a new physics inverse, `set_mover_phase_at_path_fraction` (`kinematic_mover/forced_phase.rs`, the inverse of `mover_path_fraction`). The phase rests: no wait, no velocity, not active, heading toward the last waypoint. So t = 0 reads docked closed. The transform takes the phase's position.
- Drawn geometry, shadow casters, occluder bounds and SH sample regions then follow from the existing collectors.
- The forced movers' portal seal (the live frame's `mover_is_docked_closed` rule) feeds capture's visibility walk. The registry now spawns before visibility.
- The forced movers' light states are collected as the game collects them (arc-length t, then the ramp from id 57) and handed over before the first preload frame.
- Unforced movers keep today's behaviour: rest pose, no state, no seal.
- `dev_pose_mover` and `park_dev_pose_mover` are deleted.

**Proof.**

| Row | Test | Result |
|---|---|---|
| C9, game half | `runtime_movers::light_state_closet_tests::c9_a_tick_driven_closet_door_hands_over_its_ramped_post_tick_t_every_frame` (ignored; `MOVER_CLOSET_PRL`, `MOVER_CLOSET_RAMP_PRL`). The door is driven by ticks from load, reversed at t ≥ 0.5, and closed. Each tick presents two frames. Each frame's t′ equals the ramp of the door's drawn fraction (from its transform). The ramp variant carries `light_ramp_start` 0.6 and `light_ramp_end` 0.9 on `closet_door`, and the PRL's ramp record is checked. | pass: 173 ticks, peak t′ 0.5038, largest step 0.0059. Ramp variant (reverses at 0.75): 259 ticks, every frame at t ≤ 0.6 hands t′ = 0, peak t′ 0.519 |
| C9, renderer half | `uploads::renderer_tests::closet_mover_channel::c9_the_closet_composes_and_writes_the_mix_at_every_handed_t` (ignored; `MOVER_CLOSET_PRL`, `POSTRETRO_SH_STREAMING=off`). Real closet, whole load, t′ sequence 0, 0.25, 0.5, 0.5, 0.375, 0.25, 0.125, 0, 0. The total indirect atlas and the tile texture are read back each frame. | pass: 15,487 atlas halves and 1,498 live-tile halves move with t′, each within one f16 step of the mix of the t′ = 0 and t′ = 1 frames; every other half holds bit for bit. The closet has no direct block, so "the gated light's composed term" has no subject here |
| C10 (+ C7 GPU half) | `capture_mover_probe_channel::c10_streaming_off_and_sync_proof_compose_the_same_channel_output` | **open at frame level**, see below |
| D2, frame clauses | `…::d2_no_section_is_byte_identical_and_no_state_is_within_two_levels_of_no_depth` | pass: id 57 stripped, the door's state handed over against none, max channel delta 0 (closet interior). With id 57 and no state against the depth block stripped: 0 at stations 2, 3 and interior (bar 2). Control: the same state with id 57 moves that view by 28 |
| D4 (P24), frame clause | `capture::mover_frame_tests::d4_d5_…` (ignored; `MOVER_CLOSET_PRL`; run under `off` and `sync-proof`). The door's state is first handed over on a captured frame, then the next frame is captured. | pass: delta 0 on both paths (bar 2); the state itself moves 58 (off) and 60 (sync-proof) |
| D5 (P25), frame clause | same test: t′ 1 for two frames, then 0; the step frame against the next | pass: delta 0 on both paths; t′ 1 against 0 moves 60 and 59 |
| D7 (P27), app restart | `app::world_less_frame::tests::a_held_frame_after_a_restart_hands_over_the_reseeded_mover_state`. A restart respawns the door at its seeded phase; the first held frame hands over t′ 0, never the pre-restart 0.5. The renderer half is task 10's relevel row. | pass |
| World-less hand-over | `…::a_hand_over_with_no_level_leaves_no_state` | pass |
| K1 | `…::k1_closet_closed_reads_as_dark_as_the_static_door_and_open_reads_as_stripped` | **closed passes, open misses**, see below |
| K2 | `…::k2_setpiece_wall_closed_darkens_the_held_receiver_like_the_static_wall` | pass |
| K3 | `…::k3_promotion_pops_no_more_at_a_forced_terminus_than_the_ungated_arms` | pass |

Units and stations:
- Means are Rec. 709 luma of the post-tonemap sRGB capture (0–255) at 960×540.
- K3 is in summed RGB (0–765) over lit texels.
- The closet stations are the README's. Door face: (320, 600, 56), yaw 270, fov 60. Closet interior: (320, 540, 72), yaw 90, pitch −20.
- Closet rows run with reflections off.

**K1, door forced closed** (bar: the reference + 0.02):

| | Station 2 outside door | Station 3 outside door | Pool (st2 / st3) | Door closet face |
|---|---|---|---|---|
| Forced closed | 0.0056 | 0.0160 | 0.000 / 0.000 | 0.000 |
| Tiled static-door reference | 0.0059 | 0.0171 | 0.000 / 0.000 | 0.000 |

Frame means: closed 0.0086 and 0.0314, reference 0.0089 and 0.0327.

**K1, door forced open, misses** (bar: within 0.02 of id 57 stripped): station 2 reads 13.796 against 13.889 (0.093), and station 3 reads 29.590 against 29.722 (0.132).
- Diagnosis at station 2. Stripping only the depth block reads 13.800, so depth accounts for 0.004 and bounce for 0.089.
- At t′ = 1 the 144 affected probes compose S^b₁, which is stored as unencoded f16. Today's frame composes id 34, which is BC6H-encoded. B3 holds S^b₁ to id 34 only within each probe's BC6H round-trip error, and that error is what the open frame shows: 656 px move by more than 2 levels, by at most 4.
- No compose-side fix keeps the Bounce Decision. A bake-side fix would store id 34's decoded texels as S^b_k wherever S^b_k is within the round-trip bar. That changes what the Bounce Decision says is stored ("id 34's own content re-baked"), so it goes to the owner and is not built.

**K2** (receiver: a `prop_mesh` on the enemy spot, set-piece station 3 at pitch −8; 1,088 mesh px). The mesh pixels are those that differ from the receiver-free map with static indirect alone, so the floor is not judged.
- Wall closed 27.556 against the static-wall reference's 29.041 (bar: reference + 0.02).
- Wall open 31.174, so closed reads darker.
- Open against id 57 stripped: 31.174 against 31.158 (0.016, bar 0.02).
- The first cut's mask also caught the plinth and floor. Their lightmapped direct and specular are world surfaces (brief 2): specular read 0.564 closed and open against the reference's 0.035. The mesh itself has no specular.

**K3** (static promotion pinned at w = 0 and 1; neither map has an animated-baked light). The set-piece's point lights are too dim for the entity-shadow selection, so the set-piece case uses the arena spot (368, 560, 288) and a kinematic slab in the back room, in its beam.

| Case | Pop with channel | Ungated (forced open, id 57 stripped) |
|---|---|---|
| Set-piece, wall t = 0 | 5.03 | 5.50 (+ 12) |
| Set-piece, wall t = 1 | 5.50 | 5.50 (+ 12) |
| Closet, door t = 0 / 1 | 0.00 / 0.00 | 0.00 (+ 12) |

- At t = 1 the casters follow the forced pose. Casters left at the closed rest pose would shadow the runtime arm (w = 1) while the baked arm reads open.
- The closet gates no direct light, so its case is a control: the slab gets no light from the pinned centre light at any w.

**C10, open at frame level.** `off` and `sync-proof` frames already differ with id 57 stripped.
- With static indirect alone (mask 2): up to 9 levels on about a third of the closet's pixels.
- With baked direct alone (mask 8): up to 15 on the set-piece. The closet's baked direct agrees byte for byte.
- So frames cannot show byte identity. The test asserts what they can show:
  - The channel moves both modes by more than 2 levels: 12–34 per case.
  - With id 57, the modes differ by no more than they do stripped, plus 2: closet indirect 5 against 9; set-piece indirect 3 against 4 and 1 against 6; set-piece direct 12 against 11 and 16 against 15.
- The compose inputs are proven equal per cell by task 9's `streamed_mover_channel`.
- A renderer-level per-probe comparison of the composed tiles across the two paths would close the row. → Executor.

**Captures** (`captures/t11-*.png`): K1 at stations 2 and 3, closed and reference, open and open-stripped, and the door face; K2 closed, reference, open and open-stripped; K3 set-piece channel at t = 0 and 1 × w = 0 and 1, and stripped at w = 0 and 1.

**GPU steps.** No windowed run; nothing stalled.

### K1 open investigation

Investigation only, 2026-11-09. Nothing landed. The compiler ran in a throwaway worktree at 2d5a90d16 with env-gated experiment code. Captures used task 11's capture binary and K1's scene, stations, masks and measures. The default mode is `sync-proof`, which decodes id 34 from id 50's cells. All builds used `--no-cache`.

**Numbers** (luma 0–255; open = frame mean against id 57 stripped, bar 0.02; closed = outside the door box against the static-door reference + 0.02):

| Build | St2 open gap | St3 open gap | Open px > 2 levels (st2 / st3), max | St2 closed (ref) | St3 closed (ref) | B3 | B7 base |
|---|---|---|---|---|---|---|---|
| Today (BC6H) | 0.0934 | 0.1319 | 656 / 2,235, max 4 | 0.0056 (0.0059) | 0.0160 (0.0171) | pass | pass, max 2.78 levels, ratio 0.284 |
| `--uncompressed-irradiance`, reference built the same way | 0.0043 | 0.0172 | 22 / 76, max 4 | 0.0059 (0.0060) | 0.0166 (0.0167) | — | — |
| Bounce snap | 0.0043 | 0.0175 | 26 / 81, max 3 | 0.0056 (0.0059) | 0.0160 (0.0171) | pass, same figures | pass, same figures |
| Bounce snap + depth snap | 0.0000 | 0.0000 | 0 / 0, max 0 | 0.0056 (0.0059) | 0.0160 (0.0171) | — | — |
| Today, `POSTRETRO_SH_STREAMING=off` | 0.2130 | 0.2594 | 23,515 / 28,297, max 9 / 7 | — | — | — | — |
| Bounce snap, `off` | 0.1239 | 0.1449 | 16,719 / 16,121, max 9 / 6 | — | — | — | — |

- Closet interior pose (the door out of view): the open gap is 0.0497 today and 0.0000 with the snap or uncompressed.
- Contributions. With the depth block stripped, the uncompressed open frame equals stripped exactly: 0 px move. So in today's gap, bounce gives 0.089 (st2) and 0.115 (st3), and depth gives 0.0043 and 0.0173.
- Nothing else contributes: the seal, the promoted arm (the closet gates no direct light) and the drawn door are the same in both frames.

**Probe counts** (dev probe table, measure mode):
- All 144 BC6H-affected probes are affected only at the closed terminus. None is affected at the open terminus.
- The open door rests inside the lintel (z 112–224). No probe ray meets it there.
  - 40 valid probes change at all at the open pose: by at most 1.2e-4, or 0.02 display levels. All 40 are within the round-trip bar, and 34 of them are among the 144.
- Under `--uncompressed-irradiance` the bar is 0, so the floor alone admits 158 probes, again all closed-only.
- Today a probe's unaffected open terminus stores id 34's f16 tile before encoding. The frame shows exactly that tile's round-trip error, not a bounce change.

**Snap** (an unaffected terminus stores id 34 as BC6H decodes it):
- Implementation: the tile padded to 8×8 by repeating edges, encoded alone and decoded. This is the domain's own round trip, and it is exactly what id 50's isolated block-aligned tiles decode to.
- Bytes on the closet:
  - All 144 S^b₀ tiles are byte-identical.
  - All 144 S^b₁ tiles change, by at most 0.041.
  - Only id 57 differs. Bounce payload size and affected count are unchanged.
- The snap is exact only on the id-50 path (`async`, `sync-proof`). Id 34's whole-load atlas packs 6×6 tiles contiguously, so its 4×4 blocks straddle tiles and decode differently. Under `off` the gap only shrinks.
- The modes' stripped frames already differ: today 13.889 vs 14.007 at st2. This is C10's pre-existing mode difference.

**Depth residual.** 17 of the closet's 28 depth probes have no id-59 tile. At t′ = 1 their live tile holds D₁, the full static-gather tile, where the stripped frame reads averaged moments. The 11 id-59 probes' D₁ equals their id-59 tile exactly. Storing averaged moments as a flat D₁ at a terminus no ray meets removes the rest of the gap. That changes what D_k stores, so it is the owner's call.

### Task 11b

Built 2026-11-09 on the main checkout (AMD Radeon Pro 5300M, Metal), on top of task 11 (2d5a90d16), uncommitted. Maps: task 11's `mover-closet.prl` and `mover-setpiece.prl` (release `prl-build`, BC6H). The scratch builds are `mover-setpiece` with `--mover-payload-resident-bar 0` (both blocks stream, 55,362,439 B), and `mover-closet` and `mover-setpiece` with `--uncompressed-irradiance`.

**Instrument.** Both C10 and C7 read the composed atlases back probe by probe, in renderer tests on compiled maps (`uploads/renderer_tests/`).
- `render/composed_tiles.rs` reads every composed atlas whole: the indirect total, the promotion-free direct atlas and the handoff direct atlas. It cuts a probe's logical 6×6 tile out through that load's own indirection word.
  - On a whole load, the word is `probe_indirection_words`, at stride 6.
  - On a streamed load, it is the residency state's `compose_words`, at stride 8 in the dense pools (`sh_streaming/composed_tiles.rs`).
- `real_level.rs` installs a compiled PRL under a named SH mode. It installs or evicts every cluster on demand, and records frames with the gate open to every row or closed to all.
  - The closed gate passes no visible cell, and passes a fog reach that names only an absent cell. An empty fog reach would gate every row, and `record_scene_passes` takes the fog reach as its own argument.
- To load one PRL under both modes in one process, the loader gains `load_prl_with_sh_streaming(path, mode)`, which takes the SH mode as an argument instead of reading `POSTRETRO_SH_STREAMING`. The lightmap mode still comes from the environment. It is a public API, not test-only.

**Baseline finding (pre-existing, not fixed): the modes compose different bases.** In the no-state frame, every stored tile of both loads was compared:

| Map (BC6H) | Indirect halves differing | Worst | Direct halves differing (both atlases) | Worst |
|---|---|---|---|---|
| `mover-closet` | 85,902 of 156,816 | 0.228 | 48,071 of 156,816 | 1.12 |
| `mover-setpiece` (and bar 0) | 899,740 of 1,719,648 | 0.038 | 683,319 of 1,719,648 | 0.47 |

- **Cause.** The whole load decodes ids 34 and 35 from their compact atlases. Those pack 6×6 tiles contiguously, so each 4×4 BC6H block straddles tiles. Id 50 re-encodes every tile alone in a block-aligned 8×8 cell (`pack/cluster_sh_payloads/atlas.rs`). The same source therefore decodes to different texels in each mode.
- **Discriminating experiment: `--uncompressed-irradiance`.** On both maps, every stored tile is bit-identical across the modes.
  - Captured frames with the movers forced to 0.5 and id 57 in place: closet stations 2 and 3, full frame and indirect-only (mask 2); set-piece stations 2 and 3, full frame, mask 2 and baked direct (mask 8).
  - The f16 frames are byte-identical between `off` and `sync-proof`, 0 px differing.
  - The BC6H frames differ by up to 5 levels on the closet and up to 16 on the set-piece.
- **So the difference is in the composed tiles, not added at sampling.** The two sampler paths (stride 6 against stride 8) read the same composed values to the same pixels. The f16 PRLs sat in the scratchpad, so capture used placeholder materials for them. That affects both modes alike.
- Follow-up: make one encoding serve both modes, for example by having the whole-load atlas use id 50's block-aligned tiles. It is not this brief's.

**C10.** Every mover is handed no state, then t′ = 0, 0.5 and 1, with every row in the gate. On a streamed load every cluster is resident. Each probe the channel touches is compared: direct masks, bounce masks and depth records. The base each mode composes is taken out as follows:
- **Indirect.** At a probe that n movers carry bounce at, compose writes Σ lerp(S^b₀, S^b₁, t′) − (n − 1) × base, plus id 27's deltas, which the no-state frame also carries. So (with state) + (n − 1) × (no state) is mode-independent.
  - n = 1: it is the composed tile itself, held bit for bit.
  - n = 0: the indirect must not move.
  - n = 2: within 1 f16 step at the largest magnitude involved. The outputs differ because each composed half rounds once to f16, and the (n − 1)-weighted base is a second rounded half.
- **Direct atlases.** The gated term replaces the light's mover-absent term, so (with state − no state) is mode-independent up to the two stores' rounding: 1 step.
- **Clamps.** A half that either mode composes as zero (a clamped two-mover bounce sum, a gated direct term past its BC6H base, or an unlit texel) is held to the clamp's 1-Lipschitz bound: |a − b| ≤ w × |a₀ − b₀| + 1 step.
- **Uncompressed builds.** Where the baselines agree, every comparison above is held bit for bit.
- **Depth.** The depth-tile texture is compared whole, bit for bit.

Test: `uploads::renderer_tests::mover_channel_modes::c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe`. It is ignored and reads `MOVER_C10_PRLS`, a `:`-separated list of PRLs. **It passes** on all five PRLs.

| Map | Channel probes (direct / bounce, of which 2 movers / depth) | Indirect, n = 0 / n = 1 | Indirect, n = 2 (worst, steps) | Direct output (worst, steps) | Clamp excess | Depth tiles |
|---|---|---|---|---|---|---|
| closet | 157 (0 / 144, 0 / 28) | 0 / 0 differing | — | 0 differing | ≤ 0 | identical |
| set-piece | 11,401 (5,889 / 11,285, 2,120 / 991) | 0 / 0 differing (1,319,760 halves) | 0.94 (up to 52,978 halves, ≤ 5.7e-5 abs) | 0.99 (up to 41,270 halves, ≤ 4.9e-4 abs) | ≤ 0 | identical |
| set-piece, bar 0 | same as set-piece, figure for figure | | | | | identical |
| closet, f16 | 159 | 0 / 0 | — | 0 | ≤ 0 | identical |
| set-piece, f16 | 11,442 | 0 / 0 | 0 differing | 0 differing | ≤ 0 | identical |

- The channel moves 1.21 M indirect halves and 0.12–0.46 M direct halves per atlas at each t′ on the set-piece, and 15.5 k indirect halves on the closet. It moves 5,600 depth halves on the closet and 164–198 k on the set-piece.
- Streaming placement changes nothing: the bar-0 build's numbers equal the resident build's.
- The closet's direct atlases are compared too. It has no direct block, and they hold still.
- The moved-half counts differ between modes by a few hundred on BC6H builds, where a lerp lands on one mode's base and not the other's. They are equal on f16.

**C7, GPU half.** Test: `uploads::renderer_tests::mover_channel_lag::c7_a_lagging_channel_row_composes_the_frame_it_is_first_sampled`. It is ignored and reads `MOVER_C7_PRLS`. It runs under `sync-proof` with no forced full compose, and **it passes** on the closet and on the set-piece at bar 0.
- **Gate lag.** t′ steps 0 → 1 on a frame whose gate admits no row. Every channel tile still holds the t′ = 0 compose, so the rows lag. The next frame admits every row with t′ held at 1, so no t′ trigger fires on it. On that frame, every channel tile equals the t′ = 1 compose bit for bit.
  - Closet: 144 probes, 15,487 indirect halves moving.
  - Set-piece at bar 0: 11,353 probes; 1,188,250 indirect halves and 430,300 per direct atlas moving.
- **Residency lag.** Every cluster is evicted, so no channel probe is resident. t′ then steps 1 → 0, and the clusters return with t′ held at 0. On the install frame, every resident channel tile equals the t′ = 0 compose bit for bit: 144 and 11,353 probes, all on one install frame.
- The readback runs after the frame's submit. Compose precedes every consumer in the frame's encoder, and nothing writes the atlases after it, so the readback is what that frame sampled. Outside the gate, no consumer samples a row by contract (`rendering_pipeline.md` §4, Sampled-row compose).

**K3, animated half.** K3 moved to its own target, `tests/capture_mover_promotion_pop.rs`. `Run` moved to `mover_capture_support`.
- The fixture is generated in-test, as task 11's K3 generates its own. The set-piece's arena spot (368, 560, 288) gets `"_animated" "1"` and a tag, and task 11's slab is added. The test asserts that the compiled id 57 gates a namespace-1 (animated-baked) light; the set-piece has no other animated light.
- The light's animated promotion is pinned at w = 0 and w = 1, with `setpiece_wall` forced to each terminus. That is held against the same light forced open with id 57 stripped, plus `LIT_ENDPOINT_DRIFT`. The capture fails if the pinned light holds no slot.
- `k3_animated_promotion_pops_no_more_at_a_forced_terminus_than_the_ungated_arms`: **passes**.

| Case | Pop with channel (w = 0 / w = 1) | Ungated (forced open, id 57 stripped) |
|---|---|---|
| Animated, wall t = 0 | 0.000 (116.254 / 116.254) | 1.050 (160.408 / 159.359), + 12 |
| Animated, wall t = 1 | 1.047 (160.403 / 159.356) | 1.050, + 12 |

Over 15,288 lit px, in summed RGB.
- At t = 0 the wall blocks both arms: the gated baked term and the runtime arm behind the posed caster. So w moves nothing.
- The static K3 re-run after the move reads task 11's figures: set-piece 5.025 / 5.498 against 5.495; closet 0.000.

**Gates.**
- `cargo test` with `POSTRETRO_REQUIRE_GPU=1`: renderer 1,331 (15 ignored), level-loader 388, postretro 2,515 + 3 + 2. With `--features postretro/capture`: postretro 2,601 (19 ignored) + 3 + 2, renderer 1,331, and the capture targets compile, all ignored. The ignored C10, C7 and K3 rows were run as above.
- Clippy `--all-targets -D warnings` is clean on renderer, level-loader and postretro, with and without `--features postretro/capture`.
- `cargo fmt` applied.
- No windowed run; nothing stalled.

**Also.** `closet_mover_channel` now reads through `composed_tiles::read_halves`, replacing its own copy.

### Task 11c

Built 2026-11-09 on the main checkout (AMD Radeon Pro 5300M, Metal), on top of task 11b (ffc3a2dbc), uncommitted. Maps were built with the release `prl-build` into one scratch cache: before with ffc3a2dbc's compiler, after with 11c's.

**What an unchanged terminus is.**
- **Bounce.** The domain is unchanged: a probe is affected when its state at either terminus leaves both the round-trip bar and the 2-level floor. Each affected probe now stores `S^b_k` only at a terminus whose state leaves the round-trip bar (`PosedProbe::changed`). A state within the bar stores nothing; under `--uncompressed-irradiance` the bar is 0, so only an exact match stores nothing. The gather keeps a tile for every probe past the bar, not only for affected ones, so a terminus past the bar but under the floor is stored beside an affected one.
- **Depth.** An exact test, at the ray level: terminus k stores `D_k` only if some gather ray of the probe meets the mover posed at k. A terminus no ray meets bins the mover-absent gather's tile bit for bit, so it stores nothing, and the live tile blends toward the probe's base there.
  - The significance comparison was not used. It scores a tile against the probe's averaged moments, so at an untiled probe the static gather's own tile can clear 0.05 although nothing changed.
  - The record rule is unchanged (some met terminus scores ≥ 0.05). So every record stores at least one tile, and the record and live-tile sets do not move.
  - `static_depth_tile`, which filled the unmet terminus, is deleted.

**Epoch 4.** Only id 57 changes, and id 50 shrinks where the bounce payload streams.
- Header byte 0 is 4.
- Bounce entry, 24 B: `u32 mover_id, u32 reserved 0, u64 m₀, u64 m₁`. `m_k` holds the probes storing `S^b_k`. The affected mask is `m₀ | m₁`, a non-empty subset of the valid mask; either `m_k` may be empty. Payload per entry, per affected probe x-fastest: `S^b₀` if stored, then `S^b₁` if stored.
- Depth record, 12 B: `u32 mover_id, u32 probe_index, u32 stored termini` (bit 0 `D₀`, bit 1 `D₁`; non-zero; other bits reserved 0). Payload per record: `D₀` if stored, then `D₁` if stored.
- New refusals: `MoverProbeChannelDepthRecordStoresNothing`; a reserved termini bit is `ReservedBits` ("depth record stored-termini bits"); a bounce entry storing nothing is `ProbeMaskEmpty` on the union.
- Streamed rows keep their shape; a cell's bounce row holds its stored tiles only.
- Views: `MoverBounceEntry::{affected_probe_mask, stored_tile_count, probe_tiles}`, `BounceEntryView::probe_tiles`, `MoverDepthRecord::{order_key, stored_tile_count}`, and `MoverProbeChannelSection::depth_record_tiles`, which replaces `depth_tile_pair`.
- Memo `mover_probe_bounce` v3: each probe carries a stored-termini byte and only its stored tiles.

**Renderer.**
- Bounce table entries are 6 words: slot, `m₀` (2), `m₁` (2), first tile half. Whole and pooled tables hold stored tiles only.
- `mover_bounce.wgsl` `mb_entry_tiles` gives each terminus's tile or `MB_NONE`. At an unstored terminus, `sh_compose.wgsl` reads `sample_compact_base_atlas`, the texel the pass composes with no section in that mode.
- The lerp is exact at both termini, and a later mover adds `(state − base)`. So a mover resting at an unstored terminus adds exactly zero. The clamp and the id-27 deltas are unchanged.
- Depth: `MoverDepthTiles` holds the stored tiles. At `bind` it adds one base tile per probe that a record reads at an unstored terminus: the id-59 tile the probe's reference names, else its averaged id-34 moments in every texel.
  - `DepthBases` carries id 59 and the probes. On a streamed load the probes come from the base metadata (`ShVolumeSections::stream_base_probes`).
  - The host ledger row counts stored and base tiles.

**Measurements.** Before is ffc3a2dbc's compiler, after is 11c's, all resident at the default bar. "One / two" counts (probe, mover) bounce pairs and depth records by how many termini they store.
- Only id 57 changes on all four maps; ids 34, 35, 49, 50 and 59 are byte-identical.
- The affected-probe and depth-record sets are unchanged.

| map | id 57 bytes | bounce payload | bounce pairs one / two (S^b₀ only, S^b₁ only) | depth records one / two (D₀ only, D₁ only) | depth tile bytes | live tiles |
|---|---|---|---|---|---|---|
| mover-closet | 85,096 → 42,952 | 62,208 → 31,104 | 144: 144 / 0 (144, 0) | 28: 28 / 0 (28, 0) | 22,400 → 11,200 | 28 |
| mover-setpiece | 8,963,434 → 6,688,026 | 5,790,960 → 3,545,856 | 13,405: 10,394 / 3,011 (10,098, 296) | 1,000: 94 / 906 (83, 11) | 800,000 → 762,400 | 991 |
| kinematic-platform | 8,450,466 → 4,353,858 | 8,260,272 → 4,170,744 | 19,121: 18,933 / 188 (11,478, 7,455) | 138: 63 / 75 (34, 29) | 110,400 → 85,200 | 125 |
| campaign-test | 197,000 → 182,608 | 4,320 → 2,160 | 10: 10 / 0 (0, 10) | 234: 33 / 201 (30, 3) | 187,200 → 174,000 | 229 |

Before, every pair and record stored both termini. Each bounce entry grows 8 B and each depth record 4 B. Scratch builds:
- `mover-setpiece` at bar 0 (both blocks streamed): id 50 goes from 18,542,862 to 15,480,414 B and id 57 from 832,648 to 802,344 B; id 49 is unchanged. With every cell resident, its streamed pools hold 6,547,416 B live (was 8,792,520).
- Under `--uncompressed-irradiance` the bar is 0, so nearly every pair stores both termini. The closet has 37 two-sided pairs of 158, because the open pose moves those probes by at most 1.2e-4. On the setpiece all 13,844 pairs are two-sided. Depth does not depend on the encoding.
- Setpiece's live-tile CPU plan per travelling mover (dev profile, `a_travelling_movers_live_tile_plan_cost_on_a_real_map`) is 0.19 ms for the wall and 0.26 ms for the room, as in task 10. The host row holds 1,598,400 B: the stored tiles plus 94 base tiles.

**Proof.**

| Row | Test | Result |
|---|---|---|
| B3 | `closed_door_closet_bounce_matches_the_static_door_reference` | pass: 144 closet probes store S^b₀, worst \|composed − reference\| 0 against bar 0 (unchanged). None stores S^b₁, so the open door composes id 34 |
| B4 | `closed_door_depth_tiles_match_the_static_door_references_tiles` | pass: 28 of 40 reference tiles carried, 28 records (unchanged) |
| B7 base | `raised_room_bounce_is_nearer_the_static_room_reference_than_today` | pass: 224 interior probes, median 0.0747, max 2.78 levels, ratio 0.284 (unchanged) |
| B7 direct | `raised_room_direct_composes_within_one_q_step_of_the_static_room_reference` | pass: 1,025 pairs, worst 0.500011 q steps (unchanged) |
| B8 | `a_full_re_bake_differs_from_id_34_beyond_the_bar_only_where_the_domain_admits` (ignored, on demand). Extended: each stored probe stores terminus k exactly where k's re-bake leaves the round-trip bar | pass: closet 191 probes (191 one-sided); setpiece mover 0 11,313 (10,540 one-sided), mover 1 3,154 (502) |
| B16 | `mover_probe_channel_cache` (10 tests) | pass |
| B17 | `mover_probe_channel_prl_parity` against `t6/prl-build-parent` | pass |
| Memo | `states::tests::the_memo_round_trips_one_sided_states_and_refuses_a_probe_storing_nothing` | pass |
| Format | level-format `per_terminus_storage_sits_at_its_pinned_offsets_and_is_refused_when_malformed`, `a_bounce_entrys_probe_tiles_count_only_stored_termini`, and the round-trip tests, whose sample now has a one-sided entry and record | pass |
| C3, two movers, clamp, residency (synthetic) | `bounce_compose_tests`; the streamed fixture now has a one-sided probe | pass |
| Unstored bounce terminus | `an_unstored_terminus_composes_id_34_and_adds_no_difference` | pass: a probe one-sided at either terminus reads S^b_k, the exact mix, then id 34. At its unstored terminus its texels equal today's byte for byte, and a second mover at its unstored terminus adds nothing |
| Unstored depth terminus | `an_unstored_terminus_blends_toward_the_probes_base_moments` | pass: the base (id-59 tile, or flat averaged moments) is held bit for bit at that terminus, with the exact mix between. The two-mover nearer rule takes the base as that mover's pair |
| D1–D14 | `mover_channel::depth` and `mover_depth_tiles` tests | pass |
| Residency (real map) | `streamed_mover_channel::a_streamed_id_57_payload_places_the_resident_builds_words_per_resident_cell` (setpiece default and bar 0) | pass: 179 direct and 303 bounce cells place the resident words |
| C9 renderer half | `closet_mover_channel::c9_…` (`off`) | pass: 15,504 indirect and 3,890 depth halves move, each at the mix |
| C10 | `mover_channel_modes::c10_…` on the five PRLs. Its base-out formula now counts each lerp's weight w on an unstored terminus: with c = n − 1 − Σw, (with state) + c × (no state) is mode-independent. Outputs reading no base are held bit for bit; one mover reading id 34 is held within one f16 step | pass. Closet at t′ 1: no indirect half moves in either mode, so the open door composes today's atlas. Setpiece, one mover reading id 34: worst 1.0 step at t′ 0.5, 0 at t′ 1; two movers ≤ 0.94. The depth texture is bit-identical across modes at every t′ |
| C7 | `mover_channel_lag::c7_…` (closet, setpiece bar 0) | pass |
| K1 | `k1_…`, now running `sync-proof` and `off` in one test | pass, see below |
| K2 | `k2_…` | pass: closed 27.556 against 29.041; open 31.164 against stripped 31.158 (was 31.174) |
| K3 | static and animated | pass: set-piece 5.025 / 5.495 against 5.495; closet 0; animated 0.000 / 1.050 against 1.050 |
| D2 frames, C10 frames | `d2_…`, `c10_…` captures | pass, figures as task 11 |

**K1** (luma 0–255, reflections off):

| mode | station | closed outside-door (reference) | open | open, id 57 stripped | pixels differing, open |
|---|---|---|---|---|---|
| sync-proof | 2 | 0.0056 (0.0059) | 13.8891 | 13.8891 | 0 |
| sync-proof | 3 | 0.0160 (0.0171) | 29.7218 | 29.7218 | 0 |
| off | 2 | 0.0057 (0.0060) | 14.0072 | 14.0072 | 0 |
| off | 3 | 0.0160 (0.0178) | 29.8428 | 29.8428 | 0 |

- The open door now renders today's frame exactly on both paths. It missed by 0.093 / 0.132 under `sync-proof` and by 0.213 / 0.259 under `off`.
- Pools read 0.000, and the door's closet face reads 0.000 against the reference's 0.000.

**Gates.**
- `cargo test` with `POSTRETRO_REQUIRE_GPU=1` on level-compiler, level-format, level-loader, renderer and postretro: 7,747 passed, 0 failed, 101 ignored. With `--features postretro/capture`: 7,839 passed, 0 failed, 129 ignored. The ignored rows named above were run on demand.
- Clippy `--all-targets -D warnings` is clean on the same set, with and without `--features postretro/capture`. `cargo fmt` applied.
- No windowed run; nothing stalled.

**For task 12 (docs).** `build_pipeline.md` §PRL section IDs and the brief's Wire format table need epoch 4: the bounce entry and depth record rows, their payload rows, and the new refusal. `rendering_pipeline.md` §4 needs the unchanged-terminus compose and live-tile base. `build_pipeline.md` §Build Cache needs `mover_probe_bounce` v3. K1 now runs `sync-proof` and `off` in one test.

### Task 12a

Code close-out, 2026-11-09. Only code changed; docs are task 12b's.

**Dev switches removed.**
- Compiler: `mover_probe_channel/dev_pose.rs` (`POSTRETRO_DEV_POSE_MOVER`), `dev_bounce.rs` (`POSTRETRO_DEV_MOVER_BOUNCE_MEASURE`, `_DUMP`), `dev_floor.rs` (`POSTRETRO_DEV_BOUNCE_FLOOR`, `_DEPTH_FLOOR`, `_PROBE_TABLE`) and `dev_trace.rs` (`POSTRETRO_DEV_PATCH_SEED_SALT`, `_PROBE_RAY_DUMP`, `_PATCH_DUMP`), and their call sites in `pipeline.rs` (pose install, bounce substitution, three cache bypasses), `sh_bake/probe.rs`, `surface_radiance/{patches,stage}.rs` and `mover_probe_channel/bake.rs`.
- The A/B alternatives only the switches could select: `InsideBrushRule::SkipContainingBrush` (the enum is gone; `PosedOccluder` always takes the dark rule) and `PosedSurfaces::Black` (the enum is gone; posed sides are always patches). `PosedSolid::segment_blocked` lost its brush-skip parameter. The dev report's fields went too: `PosedBounce`'s label, patch count, re-shaded pairs and timings, `PosedProbe`'s distance and slot, `PosedScene`'s mover id and label, `PosedRadiance::reshaded_pairs`, `write_compact_rgb`.
- The two memo keys no longer fold an inside-brush or surfaces byte. Old cache entries miss once; no PRL byte moves.
- `keep_every_tile`, `keep_poses` and `PosedBounce` stay as bake row B8's test seam; `MoverBounceOutcome::poses` and `PosedBounce`'s mover and terminus are `#[cfg(test)]`.
- Renderer: `POSTRETRO_DEV_MOVER_BACKFACE` in `kinematic_brush.rs`. `sh_sample.wgsl`'s three backface modes stay: none (billboards, skinned meshes), hard (forward, fluids) and smooth (movers) are all production. A new shader-contract test pins the mover pass to the smooth form on both SH reads with a non-zero floor.
- `dev_pose_mover` and `park_dev_pose_mover` have no reference left. The FGD had no dev keys.

**Task 10's rule pinned.** `compose_tests::an_animated_record_without_id_45_composes_ungated_and_subtracts_nothing`: with no animated family (absent or disabled at load), a borrowing and an own-tile animated-baked record both lay out ungated, the load warns once, and both atlases equal the static light gated alone at t′ 0, 0.5 and 1. The warning now reads "absent or disabled at load".

**Task 9 leftovers.**
- (a) The whole-load composes bind `MoverChannelResources::whole_load_tables`: the live table when it never pools, an empty table in a pooled one's place. A pooled table means a streamed level, whose whole-load composes never dispatch, so nothing but the streamed passes (which rebind) and in-flight work holds a grown pool's old buffer. `bounce_compose_tests::the_whole_load_composes_never_bind_a_pooled_table_that_grows` grows a pool from empty and checks buffer identity. No buffer counter exists on this wgpu configuration (wgpu's `counters` feature is off, and Metal has no allocator report), so the proof is identity, not a count. The SH ledger records only each table's fixed part at install; pool bytes reach the snapshot through `pool_bytes`, which reports the grown capacity alone.
- (b) Precedent: a chunk omitting a required dense member is refused as malformed (`validate_isolated_atlases`). `stage_mover_channel` now refuses a cluster whose dense rows cover a pooled channel cell it carries no id-57 row for. `mover_channel_tests::a_dense_row_without_its_pooled_cells_channel_row_refuses_the_cluster`. The setpiece at bar 0 (`streamed_mover_channel`, C7, C10) installs every cluster under the guard.

**One resolve.** `pipeline.rs` resolves `PosedMovers` once after `MoverPoses` and hands it to the bounce plan and the direct half; it drops after the direct half. The solids now live through the delta bakes, so `MoverChannelCost::solid_bytes` prices them into the SH gate's direct phase.

**Blank light keys** (coordinator, 2026-11-09). A blank `light_bounces`, `light_ramp_start` or `light_ramp_end` now reads as absent, as other `kinematic_mover` keys do; negative, fractional, non-numeric and out-of-order values still fail naming the mover. Parse tests cover blank and whitespace values. `docs/level_design.md` lines 539 and 841 still say an empty value is an error (the coordinator's fix).

**Clippy debt.** `postretro-model`'s two `type_complexity` errors (`clip_library.rs`, `gltf_loader.rs`, test code) take a local `Patch` type alias.

**Proof** (PRLs built by this tree's release `prl-build`, `--no-cache`: closet, setpiece, setpiece at bar 0).
- B8 passes with task 11c's numbers: closet 191 probes, setpiece 11,313 and 3,154. B17 passes against `t6/prl-build-parent`.
- C10 (three PRLs), C7 (closet, setpiece bar 0) and the streamed residency row (setpiece default and bar 0) pass.
- Capture, `--ignored`: C10 and D2 frames, K1, K2, K3 static and animated pass.
- `cargo test --workspace` with `POSTRETRO_REQUIRE_GPU=1`: 15,423 passed, 0 failed, 120 ignored. With `--features postretro/capture`: 15,515 passed, 0 failed, 148 ignored.
- `cargo clippy --workspace --all-targets -D warnings`, with and without capture: clean. `cargo fmt --check`: clean.

### Rising-room probe blobs

Diagnosis, 2026-11-09, at bc7537a86 (AMD Radeon Pro 5300M, Metal). `mover-setpiece` built by the release `prl-build` (BC6H, every block resident). Captures force `setpiece_room` to t, reflections off, 960×540. The camera rides the room at map (860, 320, floor + 64), facing east, pitched −28° at the floor or +30° at the ceiling. Stills: `captures/blobs-*.png`.

**Metric.** Mean luma of 8×8-pixel blocks over the room's floor crop. Each block's excess is measured over the median of the blocks within 6 blocks of it. Reported as max excess (luma levels), then the count of local-max blocks more than 3 levels above that median. Large-scale lighting, such as the doorway's shadow, scores about 6–7 with no lattice pattern.

| t | (a) as built | (b) id 57 stripped | (c) depth block stripped | (d) bounce block stripped | (e) direct only |
|---|---|---|---|---|---|
| 0.50 | 7.2 / 10 | 4.8 / 9 | 7.2 / 10 | 6.2 / 5 | 6.2 / 5 |
| 0.80 | 8.2 / 19 | 2.8 / 0 | **15.8 / 24** (dots) | 6.3 / 12 | **14.0 / 21** (dots) |
| 0.86 | **14.6 / 35** | 2.6 / 0 | 6.5 / 15 | 11.7 / 24 | 5.9 / 16 |
| 0.88 | **14.6 / 64** | 2.5 / 0 | 6.4 / 18 | 12.7 / 44 | 5.7 / 15 |
| 0.90 | **11.3 / 58** | 2.6 / 0 | 6.7 / 16 | 10.5 / 48 | 6.0 / 15 |
| 0.92 | **13.8 / 55** | 2.5 / 0 | 7.0 / 15 | 13.1 / 50 | 6.0 / 15 |
| 0.95 | **15.1 / 44** | 2.7 / 0 | 7.2 / 16 | 14.6 / 36 | 6.1 / 16 |
| 0.96 | 8.3 / 31 | 2.8 / 0 | 7.4 / 16 | 7.9 / 26 | 6.0 / 16 |
| 0.97 | 7.2 / 17 | 2.8 / 0 | 7.3 / 15 | 6.0 / 17 | 6.0 / 14 |
| 1.00 | 6.9 / 22 | 2.6 / 0 | 6.9 / 22 | 5.6 / 18 | 5.6 / 18 |

- The interior ceiling scores at most 2 at every t and variant, with no pattern.
- The roof's top, seen from the arena, shows faint lattice dots at t = 0.95 (10 against 4.9 stripped). Variants a, c and e score alike there, so those dots come from the values, not from depth.
- With light-term masks at t = 0.9 and 0.95, the floor's diamonds are all in baked static direct (mask 8: 12–16, the same as the full frame). Indirect alone (mask 2) stays under 2.

**What removes the blobs.** Stripping the depth block, variants (c) and (e), removes the diamonds over t ≈ 0.85–0.965. Stripping bounce (d) does not. A second, narrower pattern shows when depth is stripped: bright dots at t ≈ 0.80, the moment the floor passes the z −34.5 probe layer. In the shipped frame, depth masks those dots.

**Probes.** The grid is 1 m (39.37 u), with layers at map z …, −34.5, 4.9, 44.2, …, 162.3. The interior columns run x 886.3–1122.5 and y 182.7–458.3. No probe lies in the floor slab, which spans z −16 to 0 when raised. Every probe below is valid, untiled in id 59, and carries a room depth record storing both termini. None carries a wall record.

| Layer | e.g. probe (map) | Depth toward the floor | S^b₀ / S^b₁ (tile mean) | q₀ / q₁ (gated lights 1, 2, 3, 5, 11) | Composed handoff direct, floor-facing, t = 0.9 |
|---|---|---|---|---|---|
| z 4.9, interior side of the floor | 5913 (925.7, 300.8) | D₀ = D₁ downward (0.1248, 0.01558), σ 1.9 mm. Base: averaged (8.6, 111), σ 6 m | ≈ 0.05 / ≈ 0.004 | 255 / 0 | 0.0000–0.039 across the layer (5284: 0; 5913: 0.039) |
| z −34.5, under the floor | 5871 | D₀ = D₁ upward (0.4795, 0.2299), σ 2.6 mm. Base: averaged (7.2, 79) | 0 / 0 | 0 / 0 | 0–0.023 (5872: 0; 5873: 0.023, ungated lights) |
| z 162.3, in the roof slab at t = 1 | 6083 | D₁ ≡ 0 | ≈ 0.027 / 0 | 255 / 0 | — |

**Mechanism A: depth, the diamonds.** At both termini a mover surface sits 0.1248 m below the z 4.9 layer: the roof's top at the bottom (flush with the arena floor) and the floor's top at the top. So D₀ equals D₁ in the down hemisphere, and the live tile holds "a surface 12 cm below" at every t′. Mid-travel the floor is 0.25–0.9 m below. The fragment is farther than the stored mean, and the texel variance is about 2 mm², so the Chebyshev weight floors at 0.03 in a cone around the probe's foot.

| t | d to the foot (m) | live E[d] | Chebyshev at r = 0 / 8 u / 16 u |
|---|---|---|---|
| 0.90 | 0.470 | 0.1248 | 0.03 / 0.03 / 0.03 |
| 0.95 | 0.247 | 0.1248 | 0.03 / 0.13 / 0.04 |
| 0.96 | 0.202 | 0.1248 | 0.12 / 0.38 / 0.76 |
| 0.97 | 0.157 | 0.1248 | 1 / 0.93 / 1 |

- The cone opens out as the floor drops away and closes to a dot as the floor nears the baked plane at t ≈ 0.967. This is the growing and shrinking the owner saw.
- The lower layer's up-tile holds the same coincident pair: the roof's underside when lowered and the floor slab's underside when raised, both at z −16. It floors from t ≈ 0.93.
- The weight therefore moves, in a pattern locked to the lattice, onto grazing neighbours and onto the probes behind the surface. Mover fragments give those the smooth backface weight, 0.03 at the floor to about 0.4. Baked direct varies 0 to 0.039 between neighbouring probes.
- The rejected region is bounded by the octahedral tile's texel footprint, which projects as a diamond on the floor.
- A CPU mirror of the mover sampler, fed the GPU-composed tiles, reproduces the pattern under the live tiles and none under base moments (`captures/blobs-sim-t0.90-*.png`).
- This is not hypothesis 1's in-slab case. The floor has no in-slab probe, and the z 162.3 roof-slab probes show nothing on the interior ceiling. It is not the two-mover rule or an unstored terminus either: every probe involved stores both termini, and no wall record touches them.

**Mechanism B: values, the dots at t ≈ 0.80.** Both terminus states are black for the z −34.5 layer: inside the sealed room when lowered, in the sealed shaft when raised. At t ≈ 0.8 that layer sits in the moving room, just above its floor. The floor's sample is 3 u above the layer, so these black probes take most of the trilinear weight. The smooth backface weight dips to 0.03 only right above each probe, and there the lit z 4.9 layer wins. The result is a lattice of bright dots. With base moments, a sharper smooth form ((dot + 1) / 2)⁸ + 0.03 nearly removes it in the CPU mirror; ⁴ halves it. The hard form removes it.

**Candidate fixes.**
1. *Depth blends through the base mid-travel.* The live tile goes D₀ → base → D₁ (base at t′ = 0.5, held at an unstored terminus as today). Any base share above about 1% makes the variance large, so mid-travel depth goes permissive and termini stay exact. The CPU mirror removes mechanism A at every t. For a door, D₁ is about the base already, so a door's ramp barely moves. IP: moment sets blend; nothing is flagged, zeroed or excluded; the 0.03 floor and masks are unchanged; bounce is untouched. It changes the "Closed-pose depth blends with travel" Decision and depth row 1 ("their linear mix at t′ = 0.5"), so it goes to the owner. Cost: one CPU base tile per depth probe (991 × 400 B on setpiece), no GPU change. On its own it uncovers mechanism B at t ≈ 0.80.
2. *A sharper smooth backface for movers*, ((dot + 1) / 2)^k + 0.03 with k = 4–8. It stays smooth and floored (ip-372 lever 5) and is never zero, so it fits the IP rules. It changes the owner's form choice (Delegated answers, the DDGI square), not a Decision. Task 8's west-face and upper-wall captures need a re-run. It fixes mechanism B, not mechanism A. The hard form fixes B too, but it reverses the owner's IP ruling.
3. *Values for a mover that sweeps through the probe lattice.* Terminus states cannot describe a probe the room passes through, so the lower layer is black mid-travel when it should be lit. Blending bounce through id 34 and direct q through 1 mid-travel is the same shape as fix 1. It changes the Bounce and Direct Decisions, and it lights the room's interior mid-travel as if the shaft were open. Only if fixes 1 and 2 leave visible residue.
