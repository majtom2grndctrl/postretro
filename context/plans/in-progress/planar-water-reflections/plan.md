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
- Task 6 clarifications (same meaning as AC 10 and AC 11):
  - The pixel-style resolve takes c = 0 where the *untilted* texel holds no mirror content, because d is undefined there. This is a coverage cut, independent of the offset, so D1 holds.
  - R stays measured at the receiving fragment, not the cell centre, so the resolve's R cut matches the mask's.
  - The AC 11 GPU test excludes texels where either the tilted or the zero-offset lookup lands on an empty texel: the zero-offset reprojection can round one texel off. Measured result: 2,011 of 2,026 water texels compared, all with equal c.
  - The AC 10 GPU frame pair runs at 256² and Ultra. At 64² a one-texel tilt moves a reflection only about two mirror texels, too few for a safe margin. Measured: the reflected edge moved −57 texels with planar on and −57 with SSR.
  - The resolve layout is 0 main camera, 1 world material (texel grid), 2 planar inputs (272-byte uniform) plus mirror colour and depth, and no probe group. Landing records it in `rendering_pipeline.md` §10.
- Task 4 (crop), findings and clarifications:
  - **Pad.** `MIRROR_CROP_PAD_RADIANS` ≈ 0.4031 rad (23.1°): twice the largest normal tilt, (2 + 2) texels × 0.05 = 0.4 rad of ray turn, plus π·`FLUID_REFLECTION_RAY_MIN_Y` for the horizon clamp. The same pad applies in both styles, so the walk never depends on style.
  - **Grid alignment (Path addition).** The crop rect is widened to an integer zoom of the walk's 1/256 snap grid (`align_to_walk_grid`), so the cropped walk's snapped rects nest in the uncropped walk's. Without it, the lake sweep reached 8 cells outside the uncropped walk (one cell each). §7.3a records this at landing.
  - **Crop source.** The crop projects the selected plane's faces that pass the main frustum, not its union bounds. One implementation lives in `visibility/src/mirror_crop.rs`, used by render-cpu (`MirrorCamera::cropped_to_plane`, which replaces only `cull_vp`) and by both sweeps. The renderer's mirror cull takes it through `cull_planes()`, unedited.
  - **Empty crop.** When no in-frustum face of the selected plane projects on screen, no mirror camera is kept that frame and the held selection is kept. No pixel of that plane is on screen, so routing it to SSR shows nothing different, and the planar pass records nothing.
  - **Measured effect is small on today's maps.** Cropped vs uncropped over lake, e24 and probes: summed portal tests −1.1% to −1.8%, summed reached cells about −1.5%. 11 of 3,258 poses where the plane covers under half the screen reach fewer cells, which satisfies AC 36's row. 648 walks with nothing of the plane on screen are skipped outright. The 23° pad covers most of the screen. The reduction in mirror draws from the cropped cull is not measured yet (perf rows).
  - **Residual edge case.** At the screen edge a cell centre can sit up to half a texel off screen while its fragment is on screen, so the crop can miss by up to half a texel's angle along that edge. This could only matter for a very coarse texture right at the screen edge.
  - The crop-off seam `set_crop(bool)` is compiled under `cfg(any(test, feature = "capture"))`; Task 10 wires it into capture as the `crop` field.
- Task 5 order of selection rules: area floor (at install), then probe gate (per face at install: a face adds area only with a valid probe binding), then seed-in-reach filter (the same predicate the seeds use, over fog reach on a walk path and the drawn set on a fallback), then eye band, then score, then hysteresis (only for a held plane scoring above 0). A post-walk guard hands out no mirror camera when every seed fell outside the cropped frustum (`NoSeeds`), so that water reads SSR. This closes the Task 3 open item. The guard has no direct fixture test; the seed filter test covers the case it was raised for. §7.3a records the floor and the order at landing.
- Task 7 findings and open items:
  - **Mirror forward depth (closes the Task 1 item).** Of the screen-space reads in the world forward shader, only the SDF shadow factor and the dev-tools SDF debug views read main-screen data. `mirror_camera_uniform` zeroes the SDF flags and maps debug modes 2–4 to On inside the mirror. Surface-depth AO and parallax are texel-space from the mirror eye, and shadow sampling is world-space, so both are correct in the mirror. **Remaining gap:** SDF-shadowed static lights reflect unshadowed in the mirror, because the SDF factor is a main-screen product. A fix needs an SDF trace from the mirror view; it's for the owner to weigh at the motion read, and a follow-up if wanted.
  - **Billboard facing.** The facing comes from the mirror raster's rows. `flip_x` negates the right row, which is exactly the right vector of a camera at the reflected eye, so sprites face the mirror camera and read mirrored in the water like a real object. The billboard pipeline has no culling, so winding doesn't apply.
  - **Sprites over empty mirror texels** (no world behind them) are invisible, because the resolve keeps c = 0 there. Accepted limit, recorded in §7.3a at landing.
  - **Movers are not split by reach.** The mover collector still takes main ∪ mirror cells, so mirror-only movers also record in the main mover pass. They produce no pixel there (off screen, GPU-clipped) but cost some vertex work. There is no mover cap, so admission rank is unaffected.
  - **Mesh instances.** `MirrorReach { Hidden, AlsoMain, Only }` lives on mesh instances. Any main-view reach, shadow casting included, makes an instance `AlsoMain` (the O15 seam). `Only` instances plan at priority 3, after ghosts, never cast into dynamic or promoted-static depth, and draw only through `record_mirror_draws`. Task 8 adds the light half of the tag.
  - **File size.** `render-cpu/src/mesh_instances.rs` is now ~690 non-test lines, past the 600 split threshold. The split goes to the review-fix pass.
- Task 8 notes:
  - **Tagging.** Light tags come from frame N's reaches. The main view's `light_reachable_cell_mask` and `reachable_cell_aabbs` are now exactly as with planar off. Cells only the mirror reaches go to a separate `mirror_light_cell_aabbs`, handed over with `set_planar_mirror_light_reach`; play and held frames both call it.
  - **Ranking.** `assign_slots_with_hysteresis` ranks in two phases over one sort. The main phase is the old algorithm on main-view inputs alone. The mirror phase seats only in free slots and in the promoted budget the main phase leaves. A proptest checks the main assignment is identical with and without mirror-only entries.
  - **Mirror light records.** The mirror has its own light records and bind groups (`renderer_mirror_lights.rs`), built at full init and level install. Fog skips mirror-only lights.
  - **Instrument (AC 39).** `Renderer::admission_occupancy()` returns mesh-cap and shadow-pool occupancy split main and mirror-only. A throttled `[Admission]` debug line and a `POSTRETRO_SHADOW_DEBUG` segment report it. Capture adds nothing yet: it runs no planar mirror and no dynamic lights.
  - **Limit (for the owner).** For mirror-only *promoted* static and animated lights, the main view drops only the shadow slot. The promotion blend (direct-SH and Pass B (1−w) compose, the shadowmask union, the w-weighted forward record) stays shared between views. That is exact wherever no main-view receiver lies in the light's influence, which the influence-vs-cell tag gives except where geometry overhangs its cell bounds. A full split would need per-view SH atlases. AC 23 is proven with a dynamic spot.
  - The mirror camera uniform's `total_light_count` is one frame stale. This predates Task 8: it is copied before slot update.
- Task 9 notes:
  - **Held frame.** The Settling held frame runs the play frame's mirror step (`prepare_planar_mirror_at`) from the held pose before the streaming step.
  - **Settle set.** Lightmap blocks reach it on any path through `drawn_cells` and `update_capture_view`. SH stays on its gate and mandatory tier.
  - **Entry line.** `[Settling] entry` gains `planar_held=<h|none> planar_reveal=<h|none> mirror_only_blocks=<n|n/a>`, appended. `mirror_only_blocks` counts blocks of mirror-reach cells the main view does not draw that lie outside the camera cell's baked set and the pins: stricter than the brief, since it also excludes blocks the main view draws.
  - **Candidate verdict.** The renderer's candidate default is `PlanarCandidates::from_volumes` over all volumes, so the probe gate and the 14 m² floor give one source. The app's verdict (`set_planar_candidate`, at install and fluid refresh) narrows it to reflecting volumes.
  - **O9 resize.** A layer build while a scene-extent change is recorded but uncommitted waits for the commit, so the build happens once at the new extent.
  - **Pipeline creation.** It is pinned by a source scan of the lifecycle call graph, since wgpu exposes no counter.
  - **Out of scope.** One file outside both task scopes changed: `render-cpu/src/render_extent.rs` gained `ExtentState::scene_change_pending()`.
- Task 10 notes:
  - **Renderer `capture` feature.** The renderer gains an empty `capture = []` feature, and `postretro/capture` enables it. It's the only way capture reaches the preset override without compiling it into production.
  - **Preset override.** `set_planar_preset_override` is now tier-tagged: a tier change drops it and resets to the tier's preset, as the SSR dev sliders do.
  - **Dev panel.** "Planar reflection", in the Lighting tab: Planar off (plane identity routes to SSR), crop, scale ¼/½/1, D and R with bands, and reset.
  - **Launch seeds** (dev-tools only, read once): `POSTRETRO_PLANAR_SCALE`, `_D`, `_R`, `_BAND_D`, `_BAND_R`, `_OFF`, `_CROP`. The spike's `_REFLECTION` and `_WALK_STEPS` are dropped. The preset half of a seed applies to the first planar tier the renderer keeps.
  - **Capture.** The capture `planar_reflection` block takes scale, d, band_d, r, band_r and crop, with no `on` field. At very_high and ultra, capture runs the play frame's mirror step from the capture camera. Below that, the block warns and is ignored, so a High capture makes no planar call (AC 34). The capture report gains a `planar_reflection` block at planar tiers. Capture draws no particles, so the mirror gets none there.
  - **Grep gates (AC 4):**
    - `planar_recording_uses_only_the_frame_encoder`
    - `planar_sources_cite_no_spike_contract`
    - `planar_seeds_and_panel_compile_only_under_dev_tools` (a module-chain cfg resolver)
    - `planar_fields_stay_out_of_the_player_options_catalog` (postretro)
    - `planar_fields_stay_out_of_map_keys` (level-compiler: one `#[cfg(test)]` module line in `lib.rs`; no format or section change)
  - **Spike records.** The four spike docs are in `plans/done/planar-water-spike/`, with a README pointing at the evidence on `claude/planar-water-spike` 8201dfb8e.
  - **File sizes.** `capture/prepared.rs` is now 768 lines, near the 800 threshold. Together with `render-cpu/src/mesh_instances.rs` (~690 non-test lines) it goes to the review-fix split.
  - **AC 5 review gate: passes.** `git diff main..HEAD` touches nothing under `crates/level-compiler` (except that test-module line), `crates/level-format` or `crates/level-loader`. No section id, version or `PORTAL_WALK_EPOCH` changed. The only section references added are test-side reads of EnvironmentVolumes and ReflectionProbes.
  - **AC 6 FXC output** (2026-10-26, this machine):
    ```
    DX12 FXC test ran: every full-init pipeline compiled
    test render::dx12_fxc_test::dx12_fxc_compiles_every_full_init_pipeline ... ok
    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1095 filtered out; finished in 33.63s
    ```
  - **AC 1 tally** (all non-zero): render-cpu `planar_` 18 (1 ignored); visibility `mirror_` 12; renderer `--lib planar_` 35 (38 with dev-tools); postretro `planar_` 27 (dev-tools 27, capture 29); level-compiler `planar_` 6.
- Task 11 notes (measured 2026-10-26, GTX 1660 Super, Vulkan, main checkout's bakes):
  - **AC 34 (pass).** The base binary was built from 25db4c78f in a throwaway worktree (`CARGO_INCREMENTAL=0`, `-p postretro --features capture`, debug), then its target and worktree were deleted. Captures ran at 960×540 at two lake poses, with `reflection_quality` set to "high" and also omitted (the default is High). Base and branch PNGs are byte-identical:
    - lake1 `[-16.8656, 1.7, -48.768]` yaw 90 pitch −10 → `114b1c6b…f7563`
    - lake2 `[-48.768, 1.7, -16.256]` yaw 0 pitch −3 → `87cef7f3…81538`
  - **Capture needs `POSTRETRO_SH_STREAMING=sync-proof`.** A static capture refuses to run without it. Every capture here set it, base binary included.
  - **AC 35 (pass).** The final-tree sweep matches Task 4. Cropped: 4,565 mirror walks, worst 1,416 portal tests (lake), cap 5,000 = 3.53×, 0 trips. Uncropped: 5,213 walks, same worst. Worst per map: lake 1,416, e24 350, probes 424. The camera sweep is unchanged (worst 1,511, 3.31×).
  - **AC 36 (pass).** Very_high, crop on against crop off, at lake1, lake2, e24 spawn `[-6.5024, 1.6, -1.6256]` yaw 0 pitch −25, and lake3 `[-17.78, 1.7018, -17.78]` yaw 45 pitch +30. All 8 pairs (4 poses × pixel and smooth) are byte-identical PNGs. Every capture selected the plane on a walked mirror path, and the plane covers under half the screen at every pose.
    - At lake3 the crop actually drops cells (reach 72 against 77, visible 70 against 72), and the images still match.
    - 284 extra e24 smooth probe pairs are byte-identical too.
    - Fewer cells: in the sweep, 3,258 poses have the plane under half the screen. At 11 of them the cropped walk reaches fewer cells (lake 4, e24 6, probes 1).
  - **AC 37 (pass; the gate is unstressed on these maps).** `walk_measurement` gains a planar mode (`POSTRETRO_LIGHTMAP_WALK_PLANAR=1`, `walk_measurement/mirror.rs`). Each frame it runs the app's own `VisibleRenderPreparation::for_level`, then `PlanarRenderPreparation::prepare` at the very_high preset, on the mirror's own scratch. It then streams `drawn_cells()` and `sh_visible_cells()` as play does. Reflecting volumes follow capture's rule (every fluid volume with a probe-bound face), since the walk loads no manifest. The second fixture is e24: it streams lightmaps (84 blocks, 205 cells). Runs used completed reads, the default levers, pitch 0 and 2,000 steps.

    | Map / walk | Deferred off | Deferred on | Frames with a mirror view | Mirror-only cell-frames (distinct cells) |
    |---|---|---|---|---|
    | lake / random | 0 | 0 | 49,270 of 103,656 | 63,775 (128) |
    | lake / far-point | 0 | 0 | 49,085 of 111,217 | 60,201 (126) |
    | e24 / random | 0 | 0 | 42,079 of 88,399 | 123,101 (76) |
    | e24 / far-point | 0 | 0 | 46,912 of 85,700 | 123,041 (76) |

    Every mirror walk completed (0 trips). There were 0 visible misses in either mode, and the paired verdict was "within" every time. `RUNS=all` (caps 7/12/25, L 0/32 m) gave 24 paired runs, all 0 against 0. **Caveat:** neither fixture can defer a block at the default levers. On the lake every cell's mandatory set is all 144 blocks; on e24 the first pool generation holds all 84. The cap-7 and L-0 runs do grow the pool and still defer nothing. The mirror's demand is real: drawn blocks outside the baked set rise from 164 to 643 (lake, random walk), and the e24 random walk's requested pairs rise from 1,694 to 1,898.
  - **AC 38 (pass at an e24 override pose; the lake can't show the peak half).** The runs launched the `dev-tools` binary with `--app-name postretro-t11-<tier>`, so the owner's settings were untouched. "Planar off" is `high`.
    - **Lake spawn, very_high:** `lightmap_set_peak=144 reveal_misses=0 planar_held=-1.219 planar_reveal=-1.219 mirror_only_blocks=0`.
    - **Lake spawn, high:** `lightmap_set_peak=144 … planar_held=none planar_reveal=none`.
    - **Why the lake can't pass the peak half:** the lake level has 144 blocks, and every lake cell's baked mandatory set holds all 144. So no lake pose can raise the peak or show a mirror-only block (the boathouse pose was checked too).
    - **e24 override pose:** `--start-pose=-21.95,1.0,-44.30,180,-35` (over the force-3 trap pool, cell 69, portal path), found by a 24-pose sweep.
      - very_high (twice): `lightmap_set_peak=74 reveal_misses=0 planar_held=-0.406 planar_reveal=-0.406 mirror_only_blocks=2`.
      - high: `lightmap_set_peak=72 … mirror_only_blocks=0`.
      - Every condition holds there. After reveal the pawn sinks into the pool, but the line comes from the held pose. No standing pose with mirror-only blocks was found.
    - Running the row on e24 instead of the lake is for the owner to accept.
  - **AC 39 (reported; with and without the mirror are identical).** `[Admission]` lines at very_high and high, same poses:
    - lake spawn: `mesh main=3 mirror_only=0 | spot 0+0/96 | cube 0+0/6`.
    - e24 spawn and the override pose: `mesh main=3 mirror_only=0 | spot 0+0/96 | cube 2+0/6 (promoted 2+0)`.
    - campaign-test spawn and the same spot at pitch −30: `mesh main=7 mirror_only=0`, spot cycling 2–4+0/96, cube 1+0/6.

    No pose on today's maps admits a mirror-only light or instance, so a live run never exercises the yield; AC 22 and AC 23 prove it in tests. Main-view occupancy is unchanged by the mirror, and nothing was dropped.
  - **Correction for Task 12.** e24 holds no movers, particles, mesh entities or dynamic lights: its `.map` has 11 `light`, 12 `fluid_volume`, gravity and push volumes and a spawn. The AC 41 "e24 pose with movers and particles" therefore needs another map for movers and particles. campaign-test has dynamic spots and meshes near water.
- Doc ahead of code: `rendering_pipeline.md` §7.3a, SSR Reflection pass paragraph (line 614), already says it "skips the selected plane's draws". The code catches up in Task 1, and landing updates the "decided, not yet built" labels.

## Restated (owner-approved 2026-10-25)

- **AC 10** (Pixel style, tilted ray equals SSR's ray). Under IP rule D3 the planar ray must start from the *unpulled* cell centre, and must never call `fluid_reflection_cell_within_box` or read the probe box. SSR and the probe term start from the centre after `reflection_cell_within_box` (`render-cpu/src/fluid_surface/reflection_style.rs:172`, `fluid_reflection_pass.wgsl:144`). That function pulls rim cells, those within the inset margin of a box face, toward the box. The tilted normal is identical for every cell. The ray direction differs at rim cells, because the view vector starts from a different point. As written the row fails at rim cells, by design. The Decision ("tilts each water cell's ray exactly as SSR and the probe term do") holds as worded: the tilt is identical. Proposed wording:

  > - [ ] Pixel style: for the same cell, clock and texels-per-pixel, the planar pass's tilted normal equals the SSR pass's, and its tilted ray equals the SSR pass's for every cell the probe box's rim pull leaves in place (CPU reference); at a rim cell the planar ray starts from the unpulled centre (D3). On one GPU frame pair at the same pose and inputs, an asymmetric reflected edge on the selected plane shifts in the same direction and by the same texel count with planar on as with planar forced off, away from depth edges and the probe box rim. Smooth style: the planar layer applies no tilt.

## Delegated answers

- **Area-floor value: 14 m²** (`PLANAR_MIN_PLANE_AREA`, render-cpu). Total up-facing area per height group, all faces probe-bound:
  - `reflection-lake-test`: 3,048.6 m².
  - `e24-environment-test`: 436.0 m² (pools), plus 7.9 m² and 5.9 m² (floating water-block tops).
  - `reflection-probes-test`: 119.2 m² (canal) and 23.8 m² (pool-room pool).
  - `campaign-test`: 317.8 m².
  - `trap-pools`: no water (0 environment volumes), so it adds nothing to the bracket.

  The largest puddle-scale plane is 7.9 m² and the smallest pool-scale plane is 23.8 m². 14 m² sits near their geometric mean and clears each by about 1.7×. The floor applies at candidate install, so a puddle-only level has no candidate and allocates no planar target, which Task 9's lifecycle rule relies on.
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
| 4 | **Crop.** Project the frustum-passing faces of the selected plane through the mirror cull VP, clip at the near plane, pad by twice the largest normal tilt, and compose the result into the walk and cull VP. Crop-pad proptest; synthetic cropped headroom mode; cropped real-map sweep. ACs 17, 18, 35. | worker (render-cpu + visibility) | 2, 3 | done: crop-pad proptest green (AC 18); synthetic cropped mode: cropped ⊆ uncropped at every pose, worst 771 (cap 6.5×), 1 pose strictly smaller (AC 17); real-map sweep cropped ⊆ uncropped at all 4,565 cropped walks, cropped worst 1,416 (3.53×), 0 trips (AC 35); 11 under-half-screen poses reach fewer cells (AC 36 sweep half); render-cpu `planar_` 14/14; visibility 111 passed; postretro `planar_` 16/16; clippy and fmt clean |
| 5 | **Selection.** Area floor (measured, delegated), seed-in-reach filter (O12, O14), hysteresis on non-zero held score (O13), install reset (O11), probe-bound filter, eye band, tier read once before selection (O8). ACs 25–28, 30, 31 (selection half). | worker (render-cpu + `fluid_presentation`) | 3 | done: render-cpu `planar_` 18 passed (ACs 25, 27, 28, probe gate, seed parity); postretro `planar_` 21 passed (ACs 26, 27, 30, O12, O14, no-probe); `environment_install` 27 passed; area floor 14 m² (Delegated answers); clippy on render-cpu clean (postretro clippy rerun in the combined gate) |
| 6 | **Pixel tilt (P2).** Planar resolve composes the D4 helpers from `fluid_motion.wgsl` and `fluid_reflection_ray.wgsl`, reprojects `P + r′·d` with `d` from the untilted texel's mirror depth, and clamps off-frustum (D2). Add a CPU reference in render-cpu `planar_reflection` composing the public `fluid_surface` helpers, and the IP source scans. ACs 3, 10 (as restated), 11. Every IP note goes into the brief in full. | worker (renderer shader + render-cpu) | 1 | done: render-cpu `planar_tilt` 3/3 (6,400-cell normal and ray parity); renderer `planar_` 20/20 with `POSTRETRO_REQUIRE_GPU=1` (AC 3 scan, AC 10 GPU edge −57/−57, AC 11 2,011 of 2,026 texels equal c); `fluid_` 103/103 unedited; renderer lib single-threaded 1,066 passed, 2 failed (pre-existing ghost pair); FXC ran, every full-init pipeline compiled; clippy and fmt clean |
| 7 | **Mirror content.** Mirror particle collection over mirror cells and frustum, a separate smoke instance region, mirror-camera facing (check the X flip), a mirror-only mesh flag planned after ghosts, and ghost and viewmodel exclusion. ACs 13, 14, 15. | worker (renderer + app collectors) | 3 | done: renderer `planar_` 25/25 with GPU required (AC 13 particle and billboard incl. off main frustum; AC 14 mirror-only mover and skinned; AC 15 ghosts and viewmodel add 0 planar texels, real mesh reflects); `fluid_` 103/103 unedited; renderer lib single-threaded 1,071 passed, 2 failed (pre-existing ghost pair); render-cpu 613 passed; postretro collector filters green; FXC ran; clippy (dev-tools, capture) clean |
| 8 | **Admission rank.** Tag main-view vs mirror-only lights and instances from frame N's reaches, shadow casting counting as main (O15), before shadow ranking and the instance planner. Mirror-only incumbents get no margin against a main challenger (O7) across dynamic, promoted-static, animated-baked and the promoted cap. Mirror-only lights are excluded from main-view lighting. ACs 22, 23, 39 (instrument). | integrating executor | 3, 7 | done: lighting 71 passed (AC 22 and O7, plus the main-assignment-unchanged proptest); renderer admission tests 6/6; render-cpu `mesh_instances` 33 (instance cap full of main admissions drops every mirror-only; O15 shadow caster keeps main rank); AC 23 GPU: all 1,946 off-plane pixels match the mirror-lighting-excluded frame, 72 plane pixels show the shadow; renderer `planar_` 32, `fluid_` 103 unedited; clippy clean |
| 9 | **Settling and lifetime.** The held frame runs O1 from the held pose (O2, O3); add the `[Settling] entry` planar fields. Target lifecycle: candidate-plane install, tier changes compile nothing, keep across no-plane frames, resize incl. allocating frame, unload, fluid refresh. ACs 24, 29, 31–33. | integrating executor | 3, 5 | done: AC 24 (O3) and O2 app tests; `[Settling] entry` new fields with a log test; AC 29 app and GPU; AC 31, 32, 33 lifecycle tests; postretro 2,078 passed, 1 failed (pre-existing font test); renderer `planar_` 32, `fluid_` 103 unedited, extent 16; clippy (dev-tools, capture) clean |
| 10 | **Scaffolding and gates.** Gate env seeds and the dev panel under dev-tools (panel renamed off "spike"); capture controls per Delegated answers; drop the `POSTRETRO_UNCAPPED` spike seed unless already on main; repoint source comments to `context/lib/`; move the spike records; grep-gate tests; FXC run. ACs 1, 4, 5, 6. | integrating executor | 1–9 | done: AC 1 tally, AC 4 grep gates, AC 5 review gate and AC 6 FXC recorded in Corrections; renderer lib single-threaded 1,086 passed, 2 failed (pre-existing ghost pair); postretro bin default 2,079, dev-tools 2,113, capture 2,151, each with only the pre-existing font failure; clippy (default, dev-tools, capture, level-compiler) clean; fmt clean; smoke capture on the lake at very_high reflects, High has no planar block |
| 11 | **Instruments and measured runs.** `walk_measurement` mirror mode (second scratch, mirror pose, union demand); bake-gap pair; capture byte-match at High against a 25db4c78f binary; crop image parity; lake spawn settle; occupancy. Results recorded here. ACs 34–39. | integrating executor | 10 | done: AC 34 byte-identical at two lake poses; AC 35 cropped worst 1,416 (3.53×), 0 trips; AC 36 8 of 8 pairs byte-identical, plus 11 fewer-cell sweep poses; AC 37 0 vs 0 deferred on lake and e24 with the mirror view non-zero (unstressed: no fixture can defer); AC 38 e24 override pose passes, lake can't show the peak half (all blocks mandatory); AC 39 occupancy identical with and without the mirror, mirror-only 0 at every pose. Details in Task 11 notes. `walk_measurement` 7 passed; clippy and fmt clean |
| 12 | **Manual handoff.** Commands and poses for the owner motion read, 1660 perf matrix, Mac run and 4K memory count; record the owner's presets. ACs 40–43. | integrating executor + owner | 11 | |
