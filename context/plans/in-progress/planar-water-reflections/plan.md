# planar-water-reflections — plan of record

mode: resumable
status: approved
read at: 25db4c78f (crates/ identical to main 8ea68d8be; spike `claude/planar-water-spike` 8201dfb8e, base 473185e88)

**Approved 2026-10-25** by the owner, with the AC 10 restatement below applied to the brief as proposed.

## Corrections

- Brief header `read at 8ddb0eab7` → that sha resolves in no ref. Planning around it by re-reading every cited symbol at 25db4c78f, which resumable mode requires anyway. `crates/` is unchanged since the brief's revision commit beee40918.
- Path "`split_planar_ranges` already splits each material range" (and research §Coexistence premise) → `split_planar_ranges`, `record_planar_material_range` and `with_reflection_target` exist only on the spike. HEAD's `FluidSurfacePass::record_draws` (`fluid_surface.rs:631`) sets group 5 once and draws whole material ranges. Planning around it by porting the split from the spike's `planar_override.rs` as new routing (Task 1).
- Path "Cap `MAX_PORTAL_WALK_STEPS` (5,000), shared with the spike's `DEFAULT_MIRROR_WALK_STEPS`" → on the spike that constant aliased the old cap of 20,000, and a spike test pins 20,000. Every spike mirror measurement ran under 20,000. Planning around it by having the mirror walk use the seeded entry's hardcoded 5,000 (no mirror constant) and rewriting the two spike tests that pin 20,000. The headroom sweep (Task 2) checks 5,000 early.
- Path "forced-trip row needs a test-only cap seam" → the seam has to cross crates. The app-side trip test lives in `postretro`, and `walk_from_seeds` (`region_walk/mod.rs:220`) is crate-private. Planning around it with a `test-seams` cargo feature on `postretro-visibility` that exposes a capped seeded entry. Only `postretro`'s dev-dependency enables the feature, so normal builds never compile it.
- Decision "On any fallback they add no new reads" → on HEAD that rule is enforced only for the lightmap (`controller.rs:470`, `reads.rs:59`). SH turns every drawn cluster into Visible demand on every path (`sh_streaming/targeting.rs:30,373`). Planning around it with a new app-side gate: mirror cells join SH visible targeting only on a completed mirror walk with the main view on a walk path. The trip set is a subset of the main view's drawn cells, so it needs no gate.
- Decision "they never request probes" → HEAD has no probe demand path; probes are compile-time and bound per face. The rule holds by construction. Proof is a review gate plus an assertion that the mirror feeds no probe-demand surface. Probe streaming (another brief) inherits the rule.
- Path "Collection. The spike collects particles against the main view's visible cells and frustum" → the spike never draws billboards or particles in the mirror at all (`planar_reflection/record.rs` draws only world, movers and skinned meshes). Also, `SmokePass::record_draws` (`smoke.rs:1391`) writes instance buffers from offset 0, so a second same-frame call overwrites the first. Planning around it with a mirror particle collection over the mirror's cells and frustum, a separate instance region for mirror draws, and billboard facing taken from the mirror camera's group 0 (`billboard.wgsl:237`, `camera_right_up`). The flipped X row needs checking in Task 7.
- Path "Mirror-only instances draw" → `MeshPass::record_draws` (`mesh_pass.rs:1832`) draws only `forward_visible` instances. Planning around it by adding a mirror-visible instance flag, planned after ghosts in `plan_mesh_frame_plans` (`render-cpu/src/mesh_instances.rs:377`).
- Path "spike's `select_plane`… hysteresis" → the spike also runs planar at Low and above (`has_planar_reflecting_draws` gates only on quality != Off), and its GPU tests run at High or Low. Planning around it by gating at `VeryHigh | Ultra` and moving those tests to very_high.
- Brief "`cpu_stages.rs`" role → it holds only walk timing stages. The visibility result comes from `determine_visible_cells` (`visibility.rs:413`), wrapped by `VisibleRenderPreparation::for_level` (`render_preparation.rs:25`). The mirror's preparation sits beside that wrapper. The spike's `VisibilityStage::MirrorWalk` returns as a stage.
- AC "Lake spawn … the reveal frame selects the plane the held frames selected" → the `[Settling] entry` line (`startup/settle_log.rs:107`) has no planar field. Planning around it by adding `planar_held=` and `planar_reveal=` (the plane's height, or `none`) and `mirror_only_blocks=` to that line. The AC names the line, so the new fields become contract.
- AC row "The full pool includes ghost draws and shadow-only casters" → ghosts never enter shadow plans (`rendering_pipeline.md` §7.9), so this applies to the mesh-instance cap: ghosts plan after real groups, and shadow-only casters are priority 1. The shadow-pool half fills with main-view lights, shadow-only lights included. The meaning is unchanged.
- Measured row "a second water fixture" → every e24 fluid surface sits at z −16, so its pools form one plane (selection groups faces by height). Its floating blocks add other heights. Planning around it by computing the crop from the faces that pass the frustum, not the plane's union bounds, so a coplanar pool in another room does not widen the crop.
- Source pins the brief names, read in `fluid_surface_tests.rs`:
  - `record_draws` must contain the exact group-5 `set_bind_group` line.
  - `record_reflection_draws` must name no `FLUID_SCENE_INPUTS_GROUP`.
  - `self.scene_inputs.rebuild(` appears exactly twice across `fluid_surface.rs` and `reflection_pass.rs`.
  - `fluid_scene_inputs.rs` holds one `device.create_bind_group(` and two `=create_bind_group(`.
  - `upload_scene_inputs` has an exact body.
  - `frame_reflection_quality` appears exactly 4 times and `reflection_quality()` once in `renderer_render_frame.rs`.
  - `.set_reflection_target(` appears exactly 4 times.
  - `has_reflecting_draws` starts with `quality.runs_screen_space()&&`.

  Planning around them so all pass unedited:
  - The planar group is a second `FluidSceneInputs` instance with its own uniform. It is owned and rebuilt from a planar sibling file, built with the shared layout through existing constructors.
  - Per-range group-5 overrides run after the pinned default line.
  - The SSR skip goes after the pinned `continue` in `record_reflection_draws`, and as an `&&` term after the pinned prefix of `has_reflecting_draws`.
  - The planar pass records from a sibling file, so `renderer_render_frame.rs` gains one call and no tier read. O8 still holds, because the tier store changes only between frames.

  Task 1 is the proof.
- Spike tests rewritten, not deleted (AC 1). Each will be renamed to assert the Decision:
  - `mirror_distance_far_plane_and_water_halfspace_bound_step_limit_fallback`, `mirror_multiple_seeds_share_a_step_budget_and_no_portals_fallback_stays_bounded`, `mirror_walk_budget_zero_and_small_take_the_bounded_frustum_fallback`: assert the trip set equals camera visible ∩ cropped mirror frustum, the 5,000 cap, and no fallback widening.
  - `planar_preparation_unions_empty_mirror_cells_for_entities_residency_and_sh`, `planar_preparation_preserves_unisolated_fog_sentinel_on_main_fallback`, `planar_mirror_cells_request_lightmaps_on_main_visibility_fallbacks`: assert no new reads on fallback, and demand only on a walk path.
  - `planar_gpu_other_water_plane_remains_probe_only`: asserts the other plane reads SSR.
  - `planar_gpu_asymmetric_world_lands_on_mirrored_pixel_and_ssr_records_zero`: asserts SSR records for the other plane and skips the selected one.
  - `planar_shaders_preserve_ip_boundaries`: becomes the D4-aware scan of AC 3.
  - `planar_launch_*` (3) and `planar_capture_*` (2): asserted under dev-tools and capture only, with the `on` knob replaced by the tier.
  - `planar_reflection_fade_and_settings_use_absolute_metres`: asserts the engine-constant presets.
  - `planar_reflection_selection_hysteresis_and_eye_band`: gains the floor and seed rules.
- Path "Keep … the mirror cull" (Task 1) → the spike's `MirrorCull` (its own `CandidateCullPipeline`, indirect buffers and `draw_indirect_buckets`) breaks `indirect_contract_production_consumers_keep_guarded_ownership` (`crates/renderer/src/indirect_contract_tests.rs`). That test pins one `BufferUsages::INDIRECT` site and two `.draw_indirect` sites, as a release-safety contract. Planning around it with CPU-culled direct draws (`planar_reflection/mirror_draws.rs`): each frame, the BVH leaves of the mirror's visible cells are tested against the mirror cull planes and merged into `draw_indexed` runs per bucket, with no allocation. The GPU cull can come back only through owner review of the indirect contract. The 1660 and Mac perf rows will show the CPU cost.
- Path "the mirror cull and mask" (Task 1) → the spike's separate water-coverage texture and `planar_mask.rs` face triangles are gone. The mask rasters the selected plane's own fluid draws and writes far depth into the mirror depth (cleared to 0), so the world draws only inside it. The resolve treats depth ≤ 0 or ≥ 1 as no content (c = 0).
- Mirror forward binds the main view's group 5 (spot shadow pool plus main-screen scene depth), as on the spike. The SDF flag is zeroed in the mirror camera uniform; any other screen-depth-based term the world forward shader reads is wrong inside the reflection. Task 7 checks which terms read it and gives the mirror its own group or zeroes them.
- Pre-existing test-suite faults, not from this branch (measured 2026-10-25 on `main`'s `crates/`, restored in place):
  - `POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro-renderer --lib` in parallel crashes the test process with `STATUS_ACCESS_VIOLATION` in about half of runs: 2 of 4 on `main`; 3 of 4 on this branch with the planar tests skipped; 3 of 6 with them included. Single-threaded (`--test-threads=1`) it never crashed.
  - Two ghost tests fail on `main` and here alike: `ghost_depth_frame_test::a_nearer_ghost_blends_over_a_farther_one_and_each_hides_its_own_inner_surface` and `uploads::renderer_tests::ghost_draws_record_through_blend_and_dither_pipelines` (device lost).
  - Planning around it: renderer gates run single-threaded, and both are reported to the owner as findings outside this brief.
- Task 2's real-map mirror mode (`region_walk/real_map_mirror.rs`) uses the exact water plane as the mirror near plane, while `MirrorCamera`'s clip plane sits `PLANAR_CLIP_BIAS` (0.02 m) lower. Task 3 makes the app's mirror walk and the sweep take one near plane, so the sweep measures the walk the app runs. The mirror stage `mirror_walk` is not yet in `rendering_pipeline.md` §12's stage tree; landing adds it.
- Decision "Mirror demand follows the path rules" (Task 3, clarification; same meaning) → only *reads* follow the walk-path rule. The draw-side consumers — mesh and mover collection, SH sample regions, animated compose and light reach — take mirror cells whenever the mirror draws, a main-view fallback included. None of them issues a read, and the mirror does draw those cells. AC 19 needs the lightmap to keep holding and counting resident mirror blocks on a fallback, which this gives.
- Path "Lightmap demand … `StreamingFrame` extra cells" (Task 3) → no spike `additional_cells` seam was needed. Passing main visible ∪ mirror reach as the frame's drawn cells already gives Visible demand on a walk path and held, read-free demand on every fallback, through the existing `PathDemand` rules (`demand.rs` and the controller are unchanged). The one new seam is `StreamingFrame::sh_visible_cells`, the SH gate: the union only on main `PrlPortal` plus mirror `Walked`, else the main set.
- Open until Task 5: a selected plane with no usable seed (`NoSeeds`) still keeps a mirror camera, so that water reads an empty planar layer (probe only, not SSR). Task 5's seed-in-reach candidate filter removes such planes before scoring and closes it; its tests must cover it.
- The spike's light-term-mask gate on the app side is dropped: the renderer already records no planar pass without a reflecting selected-plane draw. Under a dev lighting mask the app may still walk the mirror and add its demand; dev-only.
- `include_mirror_lighting` appends the mirror's cell AABBs to `reachable_cell_aabbs`, which `for_level` already allocates fresh each frame. Frames where the mirror adds cells grow it once. All the mirror's own buffers are reused.
- Pre-existing, machine-specific: `startup::ghost_links_surface_tests::ghost_link_twins_install_the_scripting_surface_with_zero_warnings` fails on this machine because `fontdb` warns that `C:\Windows\Fonts\mstmc.ttf` (4 KB) is malformed. Not this branch; reported as a finding.
- Doc ahead of code: `rendering_pipeline.md` §7.3a, SSR Reflection pass paragraph (line 614), already says it "skips the selected plane's draws". The code catches up in Task 1, and landing updates the "decided, not yet built" labels.

## Restated (owner-approved 2026-10-25)

- **AC 10** (Pixel style, tilted ray equals SSR's ray). Under IP rule D3 the planar ray must start from the *unpulled* cell centre, and must never call `fluid_reflection_cell_within_box` or read the probe box. SSR and the probe term start from the centre after `reflection_cell_within_box` (`render-cpu/src/fluid_surface/reflection_style.rs:172`, `fluid_reflection_pass.wgsl:144`). That function pulls rim cells, those within the inset margin of a box face, toward the box. The tilted normal is identical for every cell. The ray direction differs at rim cells, because the view vector starts from a different point. As written the row fails at rim cells, by design. The Decision ("tilts each water cell's ray exactly as SSR and the probe term do") holds as worded: the tilt is identical. Proposed wording:

  > - [ ] Pixel style: for the same cell, clock and texels-per-pixel, the planar pass's tilted normal equals the SSR pass's, and its tilted ray equals the SSR pass's for every cell the probe box's rim pull leaves in place (CPU reference); at a rim cell the planar ray starts from the unpulled centre (D3). On one GPU frame pair at the same pose and inputs, an asymmetric reflected edge on the selected plane shifts in the same direction and by the same texel count with planar on as with planar forced off, away from depth edges and the probe box rim. Smooth style: the planar layer applies no tilt.

## Delegated answers

- **Area-floor value.** Measured so far by the Task 2 sweep (total up-facing area per height group, every fluid, before any probe filter): `reflection-lake-test` 3,048.6 m²; `e24-environment-test` 436.0, 7.9 and 5.9 m²; `reflection-probes-test` 119.2 and 23.8 m². `trap-pools` is still to measure. Set in Task 5 from the measured total up-facing area of every candidate plane on `reflection-lake-test`, `e24-environment-test`, `reflection-probes-test` and `trap-pools`. The floor sits between the largest puddle-scale plane and the smallest lake-scale one, as one engine constant in render-cpu. The value and the measured areas get recorded here when Task 5 commits. Measuring both brackets places the floor on content instead of a guess.
- **Capture-scene `planar_reflection` controls.** Kept as §7.8 capture-only fields: `scale`, `d`, `band_d`, `r`, `band_r`, plus `crop` (default true) for the crop-parity row. The spike's `on` is retired: planar runs when the scene's reflection tier is very_high or ultra, as in play. Capture defaults to High, so default captures stay planar-free and the byte-for-byte row compares like with like. Capture is the instrument for the measured rows, and the presets Decision allows capture-only controls. An `on` switch would duplicate the tier.
- **Second bake-gap water fixture.** `e24-environment-test`, if its install streams lightmaps (checked in Task 11 from the `[Lightmap streaming]` line and the capture report's `lightmap_streaming` block). Otherwise `reflection-probes-test`. e24 has portal-separated rooms and several water heights, which exercises mirror-only cells across portals.
- **Ghost pass order (Path).** Planned frame order:
  1. Skinned Mesh Pass.
  2. Ghost Pass (skinned and mover ghosts).
  3. **Planar Reflection Pass**:
     - mirror cull, depth and forward world;
     - movers, skinned and mirror-only meshes;
     - billboards and particles;
     - then the planar resolve raster over the selected plane's fluid draws.
  4. Fluid gate: SSR reflection pass, then Fluid Surface Pass.
  5. Billboard Sprite Pass.
  6. Billboard Ghost Pass.

  No ghost pass draws into the mirror, and the planar pass records before the fluid pass that samples it (O6). This gets recorded in `rendering_pipeline.md` §7.3a at landing.
- **Spike records.** `planar-water-spike-{contract,findings,drafting-handoff,windows-handoff}.md` move from the spike branch to `context/plans/done/planar-water-spike/`. The `-evidence/` tree stays on the spike branch, as the brief's Path says.

## AC-to-proof

AC numbers follow the brief's Acceptance order.

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Spike planar tests pass, non-zero per filter; replaced-Decision tests rewritten | `cargo test` filters `planar_` in render-cpu, visibility (`mirror_`), renderer, postretro; rewrites listed in Corrections | achievable as stated |
| 2 | Existing `fluid_` renderer tests pass unedited; protected paths untouched | `cargo test -p postretro-renderer fluid_` (~101) + `git diff --stat main -- <protected paths>` empty | achievable as stated (Task 1 proves the pins) |
| 3 | Shader source-scan IP tests (D4 helpers, no Fresnel/probe, direction identifiers not in comparisons) | `planar_shaders_preserve_ip_boundaries` rewritten over planar-authored text | achievable as stated |
| 4 | Grep gates: frame-encoder only; no spike-contract citation; seeds/panel dev-tools or capture only; no option/map key | `planar_recording_uses_only_the_frame_encoder`, `planar_sources_cite_no_spike_contract`, `planar_seeds_and_panel_compile_only_under_dev_tools`, options-catalog scan | achievable as stated |
| 5 | id-51 bake and format untouched; no PRL section | review gate: `git diff main -- crates/level-compiler crates/level-format` shows no id-51 or section change | achievable as stated |
| 6 | FXC compiles every full-init pipeline incl. planar | `cargo test -p postretro-renderer --lib dx12_fxc -- --ignored` on this Windows machine, output pasted here | achievable as stated |
| 7 | Two layers: large plane planar, second plane SSR (very_high, ultra) | GPU test `planar_gpu_two_planes_read_planar_and_ssr_in_one_frame` (both tiers) | achievable as stated |
| 8 | SSR records for second plane, nothing for selected; selected-only → no SSR pass | GPU test `planar_gpu_ssr_skips_the_selected_plane_and_records_only_for_others` | achievable as stated |
| 9 | Pixel: each layer read at its own scale | GPU test `planar_gpu_layers_read_at_their_own_scale` | achievable as stated |
| 10 | Pixel tilt: normal parity everywhere, ray parity off the rim (CPU) + GPU edge shift away from rim | CPU test `planar_tilt_matches_ssr_normal_and_unpulled_ray`; GPU test `planar_gpu_edge_shifts_like_ssr` with test-only planar-off seam | achievable as restated |
| 11 | Tilt never changes c (offset forced zero) | GPU test with test-only zero-offset seam `planar_gpu_tilt_moves_samples_not_confidence` | achievable as stated |
| 12 | Same-frame mirror (O6) | GPU test `planar_gpu_mirror_shows_this_frames_object` | achievable as stated |
| 13 | Billboard + particle mirrored, facing mirror camera, incl. off main view | GPU test `planar_gpu_billboards_and_particles_face_the_mirror` | achievable as stated |
| 14 | Mover + skinned in mirror-only cells off screen | GPU test `planar_gpu_mirror_only_mover_and_skinned_mesh_reflect` | achievable as stated |
| 15 | Ghost + viewmodel absent; real mesh at ghost pose present | GPU test `planar_gpu_ghosts_and_viewmodel_stay_out_of_the_mirror` | achievable as stated |
| 16 | Main visible + fog sets identical planar on/off (O1) | app test `planar_preparation_leaves_main_visibility_unchanged` | achievable as stated |
| 17 | Synthetic headroom: uncropped + cropped modes, cap ≥ 2× worst, cropped ⊆ uncropped, one strictly smaller | `seeded_walk_cap_keeps_double_headroom_over_seeded_sweeps` extended | achievable as stated |
| 18 | Crop pad covers every tilted ray (CPU), near-clipped planes included | proptest `mirror_crop_pad_contains_every_tilted_reflection` | achievable as stated |
| 19 | Main on fallback: mirror adds no LM/SH reads; resident blocks draw; misses counted | app tests `planar_mirror_adds_no_reads_on_main_fallback` (lightmap controller + SH targeting) | achievable as stated |
| 20 | Forced mirror trip: camera-visible ∩ cropped frustum, no new reads, no SolidCellFallback log | app test via `test-seams` cap + `test-log-capture` negative assertion | achievable as stated |
| 21 | Mirror-only cells on walk path reach LM, SH regions, compose; no probe; drained pose zero misses | app test `planar_mirror_cells_join_demand_on_a_walk_path` + controller drain loop | achievable as stated (no-probe holds by construction) |
| 22 | Pool yield: no main eviction; room → mirror admits; yield vs incumbency across light kinds and promoted cap (O7, O15) | lighting/render-cpu tests on `assign_slots_with_hysteresis` inputs + `plan_mesh_frame_plans` | achievable as stated |
| 23 | Mirror-only light slot: pixels off selected plane match mirror-lighting-excluded frame | GPU test `planar_gpu_mirror_only_light_shows_only_in_the_reflection` | achievable as stated |
| 24 | Settling on main fallback + completed mirror walk (O3): settle set holds mirror blocks; first world frame requests no new block | app test over `prepare_held_level_frame` + `BlockDemand::update_capture_view` | achievable as stated |
| 25 | Area floor: below never selected; just above selectable | render-cpu test `planar_selection_respects_the_area_floor` | achievable as stated |
| 26 | No seed in reach → not selected; fallback drawn-set reach (O12, O14); no probe → never | render-cpu + app tests | achievable as stated |
| 27 | Eye below / in fog band → not selected, SSR at tier; none qualifying → none | render-cpu test `planar_selection_refuses_eye_below_or_in_band` | achievable as stated |
| 28 | Hysteresis 1.25×; held plane leaving frustum yields (O13) | render-cpu test | achievable as stated |
| 29 | Plane switch frame (O4, O5), incl. trip on that frame | app + GPU test `planar_switch_frame_routes_new_plane_to_this_mirror` | achievable as stated |
| 30 | Level change ignores old held plane (O11) | app test `planar_install_resets_selection_history` | achievable as stated |
| 31 | High and below: no selection, walk, demand, pass, target (O8) | app + renderer tests | achievable as stated |
| 32 | high → very_high → high: no pipeline, target alloc/release, no walk on drop frame; no-plane frames keep target (O10); resize incl. allocating frame (O9); unload | renderer tests with pipeline-creation counters + app tier test | achievable as stated |
| 33 | Zero selected → nothing records; no candidate → no target; candidate → target from install; refresh removing last → release | renderer + app tests | achievable as stated |
| 34 | M: capture at high byte-identical to base commit | capture A/B, base binary built from 25db4c78f | achievable as stated |
| 35 | M: cropped real-map mirror sweep worst ≤ ½ cap, no ordinary trip | `real_map_sweep` mirror mode on lake, e24, probes | achievable as stated |
| 36 | M: cropped image matches uncropped (pixel, smooth); under-half-screen pose reaches fewer cells | capture with `crop: false` vs true; sweep cell counts | achievable as stated |
| 37 | M: bake-gap with planar on ≤ planar off, completed-reads; per-walk selected frames + mirror-only counts non-zero | `walk_measurement` mirror mode, lake + second fixture | achievable as stated |
| 38 | M: lake spawn settle: zero reveal misses, same plane, settle peak > planar off; mirror-only blocks outside baked set non-zero (or spawn override) | `[Settling] entry` new fields | achievable as stated |
| 39 | M: mesh-instance cap and shadow occupancy with/without mirror | capture report / instrumented run, same poses | achievable as stated |
| 40 | Manual: owner motion read, presets fixed and recorded | owner, in-engine | manual |
| 41 | Manual: Windows 1660 release perf matrix | owner run (`long-bake`/timed runs are owner-launched); executor prepares commands | manual |
| 42 | Manual: Mac 5300M native bounded run | owner, Mac | manual |
| 43 | Manual: resident memory at 4K ultra lake pose | executor computes from allocation sizes; owner confirms in-engine | manual |

## Tasks

Workers never edit this file or commit. Cargo runs one at a time (disk-bound machine), so concurrent workers edit in parallel and the integrating executor builds and tests after they finish.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Routing slice (riskiest).** Port render-cpu `planar_reflection` core (`MirrorCamera`, candidates, `select_plane` as-is), renderer planar targets and mirror recording (world, movers, skinned), gated at very_high/ultra. Second group-5 `FluidSceneInputs` over the planar layer with its own uniform. Per-draw plane routing after the pinned default bind. SSR skips selected-plane draws and stays alive. Planar resolve becomes a raster over selected fluid draws (untilted). Proof: unedited `fluid_` tests pass; GPU ACs 7, 8, 9, 12. | integrating executor | — | done: render-cpu `planar_` 8/8; renderer `planar_` 15/15 (`POSTRETRO_REQUIRE_GPU=1`, ACs 7, 8, 9, 12 green); `fluid_` 103/103 with no test edited; renderer lib single-threaded 1061 passed, 2 failed (the pre-existing ghost pair); `dx12_fxc` "ran: every full-init pipeline compiled"; clippy `-D warnings` clean; `cargo check -p postretro` (+dev-tools) clean |
| 2 | **Mirror walk port + early cap check.** Port `mirror_visibility` onto `portal_walk_seeded` with the mirror's own `PortalWalkScratch`. Add the `test-seams` capped seeded entry. Trip set = camera visible ∩ mirror frustum. Add a mirror mode to `real_map_sweep` (reflected eye, plane near plane); run it uncropped on the three maps under the 5,000 cap. Rewrite the visibility spike tests. | worker (visibility crate) | — (concurrent with 1) | done: visibility crate 107 passed (8 `mirror_`); uncropped real-map mirror sweep on lake, e24, probes: 5,213 walks, worst 1,416 tests (lake), cap 5,000 = 3.53×, 0 trips; camera sweep unchanged, worst 1,511 (3.31×); clippy with `test-seams` clean; `cargo check -p postretro --tests` clean; cpu_timing label tests 19/19 |
| 3 | **App preparation in O1 order.** Port `PlanarRenderPreparation` beside `VisibleRenderPreparation`, in new sibling files (`frame_loop`, `render_preparation`). Wire the demand path rules: lightmap via `StreamingFrame` extra cells, SH visible gate on walk path only, animated compose union, mesh and mover collection. Light reach union stays untagged until Task 8. ACs 16, 19, 20, 21. | integrating executor | 1, 2 | done: postretro bin 2,062 passed, 1 failed (pre-existing font warning test); `planar_` 15/15; AC 16, 19, 20, 21 tests green; renderer lib single-threaded 1,062 passed, 2 failed (pre-existing ghost pair); visibility `mirror_` 8/8; real-map mirror sweep on the biased near plane unchanged (worst 1,416, 3.53×, 0 trips); clippy `-D warnings` clean incl. dev-tools; capture `--tests` check clean |
| 4 | **Crop.** Project the frustum-passing faces of the selected plane through the mirror cull VP, clip at the near plane, pad by twice the largest normal tilt, and compose the result into the walk and cull VP. Crop-pad proptest; synthetic cropped headroom mode; cropped real-map sweep. ACs 17, 18, 35. | worker (render-cpu + visibility) | 2, 3 | |
| 5 | **Selection.** Area floor (measured, delegated), seed-in-reach filter (O12, O14), hysteresis on non-zero held score (O13), install reset (O11), probe-bound filter, eye band, tier read once before selection (O8). ACs 25–28, 30, 31 (selection half). | worker (render-cpu + `fluid_presentation`) | 3 | |
| 6 | **Pixel tilt (P2).** Planar resolve composes the D4 helpers from `fluid_motion.wgsl` and `fluid_reflection_ray.wgsl`, reprojects `P + r′·d` with `d` from the untilted texel's mirror depth, and clamps off-frustum (D2). Add a CPU reference in render-cpu `planar_reflection` composing the public `fluid_surface` helpers, and the IP source scans. ACs 3, 10 (as restated), 11. Every IP note goes into the brief in full. | worker (renderer shader + render-cpu) | 1 | |
| 7 | **Mirror content.** Mirror particle collection over mirror cells and frustum, a separate smoke instance region, mirror-camera facing (check the X flip), a mirror-only mesh flag planned after ghosts, and ghost and viewmodel exclusion. ACs 13, 14, 15. | worker (renderer + app collectors) | 3 | |
| 8 | **Admission rank.** Tag main-view vs mirror-only lights and instances from frame N's reaches, shadow casting counting as main (O15), before shadow ranking and the instance planner. Mirror-only incumbents get no margin against a main challenger (O7) across dynamic, promoted-static, animated-baked and the promoted cap. Mirror-only lights are excluded from main-view lighting. ACs 22, 23, 39 (instrument). | integrating executor | 3, 7 | |
| 9 | **Settling and lifetime.** The held frame runs O1 from the held pose (O2, O3); add the `[Settling] entry` planar fields. Target lifecycle: candidate-plane install, tier changes compile nothing, keep across no-plane frames, resize incl. allocating frame, unload, fluid refresh. ACs 24, 29, 31–33. | integrating executor | 3, 5 | |
| 10 | **Scaffolding and gates.** Gate env seeds and the dev panel under dev-tools (panel renamed off "spike"); capture controls per Delegated answers; drop the `POSTRETRO_UNCAPPED` spike seed unless already on main; repoint source comments to `context/lib/`; move the spike records; grep-gate tests; FXC run. ACs 1, 4, 5, 6. | integrating executor | 1–9 | |
| 11 | **Instruments and measured runs.** `walk_measurement` mirror mode (second scratch, mirror pose, union demand); bake-gap pair; capture byte-match at High against a 25db4c78f binary; crop image parity; lake spawn settle; occupancy. Results recorded here. ACs 34–39. | integrating executor | 10 | |
| 12 | **Manual handoff.** Commands and poses for the owner motion read, 1660 perf matrix, Mac run and 4K memory count; record the owner's presets. ACs 40–43. | integrating executor + owner | 11 | |
