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
- `include_mirror_lighting` appends the mirror's cell AABBs to `reachable_cell_aabbs`, which `for_level` already allocates fresh each frame. Frames where the mirror adds cells grow it once. All the mirror's own buffers are reused. *Superseded (Task 8, review):* `include_mirror_lighting` is gone; `PlanarRenderPreparation` fills a reused `mirror_light_cell_aabbs` buffer.
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
  - **Empty crop.** When no in-frustum face of the selected plane projects on screen, no mirror camera is kept that frame and the held selection is kept. No pixel of that plane is on screen, so routing it to SSR shows nothing different, and the planar pass records nothing. *Superseded in review:* the plane now yields that frame (empty crop or no seed in the crop). It scores zero, and selection retries once, still holding last frame's plane, so an on-screen challenger draws instead.
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
  - **Entry line.** `[Settling] entry` gains `planar_held=<h|none> planar_reveal=<h|none> mirror_only_blocks=<n|n/a>`, appended. `mirror_only_blocks` counts blocks of mirror cells the main view does not draw (since review: the mirror's drawable cells, not its whole reach) that lie outside the camera cell's baked set and the pins: stricter than the brief, since it also excludes blocks the main view draws.
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
- **Owner direction (2026-10-26): the local player's body reflects.** The shadow-only body (`shadowOnly: true`, the local player's `exo_red` and its attachments) now reaches the planar mirror like any world mesh. The main view still draws it only as a shadow. Changed: `mesh_render.rs` (mirror reach no longer gated on `shadow_only`) and its test `collect_with_mirror_keeps_shadow_casters_main_ranked` (body reach `AlsoMain`). The `shadowOnly` doc text now says "renders into shadow-depth passes and planar water reflections only", in `sim/scripting/primitives/mod.rs`, `sdk/types/postretro.d.{ts,luau}` and the typedef fixtures. It was triggered by the AC 42 trace: the Planar Skinned Meshes pass opened with nothing to draw, because the body was planned but excluded. Peer avatars were already forward-drawn, so they already reflected.

## AC-to-proof

AC numbers follow the brief's Acceptance order.

| AC | Row (short) | Proof | Status | Result |
|---|---|---|---|---|
| 1 | Spike planar tests pass, non-zero per filter; replaced-Decision tests rewritten | `cargo test` filters `planar_` in render-cpu, visibility (`mirror_`), renderer, postretro; rewrites listed in Corrections | achievable as stated | pass: final gate; `planar_` filters non-zero in render-cpu, visibility (`mirror_`), renderer, postretro, level-compiler |
| 2 | Existing `fluid_` renderer tests pass unedited; protected paths untouched | `cargo test -p postretro-renderer fluid_` (~101) + `git diff --stat main -- <protected paths>` empty | achievable as stated (Task 1 proves the pins) | pass: `fluid_` unedited and green; `git diff` on the protected paths is empty |
| 3 | Shader source-scan IP tests (D4 helpers, no Fresnel/probe, direction identifiers not in comparisons) | `planar_shaders_preserve_ip_boundaries` rewritten over planar-authored text | achievable as stated | pass: scan hardened in review (snapshot pins on c, fade and mask; fn-name list; weight-argument check) |
| 4 | Grep gates: frame-encoder only; no spike-contract citation; seeds/panel dev-tools or capture only; no option/map key | `planar_recording_uses_only_the_frame_encoder`, `planar_sources_cite_no_spike_contract`, `planar_seeds_and_panel_compile_only_under_dev_tools`, options-catalog scan | achievable as stated | pass: grep gates green; the frame-encoder gate now also walks the shared mirror record functions |
| 5 | id-51 bake and format untouched; no PRL section | review gate: `git diff main -- crates/level-compiler crates/level-format` shows no id-51 or section change | achievable as stated | pass: review gate (Task 10 notes) |
| 6 | FXC compiles every full-init pipeline incl. planar | `cargo test -p postretro-renderer --lib dx12_fxc -- --ignored` on this Windows machine, output pasted here | achievable as stated | pass: FXC on the final tree, "every full-init pipeline compiled" |
| 7 | Two layers: large plane planar, second plane SSR (very_high, ultra) | GPU test `planar_gpu_two_planes_read_planar_and_ssr_in_one_frame` (both tiers) | achievable as stated | pass |
| 8 | SSR records for second plane, nothing for selected; selected-only → no SSR pass | GPU test `planar_gpu_ssr_skips_the_selected_plane_and_records_only_for_others` | achievable as stated | pass |
| 9 | Pixel: each layer read at its own scale | GPU test `planar_gpu_layers_read_at_their_own_scale` | achievable as stated | pass |
| 10 | Pixel tilt: normal parity everywhere, ray parity off the rim (CPU) + GPU edge shift away from rim | CPU test `planar_tilt_matches_ssr_normal_and_unpulled_ray`; GPU test `planar_gpu_edge_shifts_like_ssr` with test-only planar-off seam | achievable as restated | pass (as restated) |
| 11 | Tilt never changes c (offset forced zero) | GPU test with test-only zero-offset seam `planar_gpu_tilt_moves_samples_not_confidence` | achievable as stated | pass (plus c ∈ {0, 1} on every compared texel) |
| 12 | Same-frame mirror (O6) | GPU test `planar_gpu_mirror_shows_this_frames_object` | achievable as stated | pass |
| 13 | Billboard + particle mirrored, facing mirror camera, incl. off main view | GPU test `planar_gpu_billboards_and_particles_face_the_mirror` | achievable as stated | pass |
| 14 | Mover + skinned in mirror-only cells off screen | GPU test `planar_gpu_mirror_only_mover_and_skinned_mesh_reflect` | achievable as stated | pass |
| 15 | Ghost + viewmodel absent; real mesh at ghost pose present | GPU test `planar_gpu_ghosts_and_viewmodel_stay_out_of_the_mirror` | achievable as stated | pass |
| 16 | Main visible + fog sets identical planar on/off (O1) | app test `planar_preparation_leaves_main_visibility_unchanged` | achievable as stated | pass |
| 17 | Synthetic headroom: uncropped + cropped modes, cap ≥ 2× worst, cropped ⊆ uncropped, one strictly smaller | `seeded_walk_cap_keeps_double_headroom_over_seeded_sweeps` extended | achievable as stated | pass |
| 18 | Crop pad covers every tilted ray (CPU), near-clipped planes included | proptest `mirror_crop_pad_contains_every_tilted_reflection` | achievable as stated | pass |
| 19 | Main on fallback: mirror adds no LM/SH reads; resident blocks draw; misses counted | app tests `planar_mirror_adds_no_reads_on_main_fallback` (lightmap controller + SH targeting) | achievable as stated | pass |
| 20 | Forced mirror trip: camera-visible ∩ cropped frustum, no new reads, no SolidCellFallback log | app test via `test-seams` cap + `test-log-capture` negative assertion | achievable as stated | pass |
| 21 | Mirror-only cells on walk path reach LM, SH regions, compose; no probe; drained pose zero misses | app test `planar_mirror_cells_join_demand_on_a_walk_path` + controller drain loop | achievable as stated (no-probe holds by construction) | pass |
| 22 | Pool yield: no main eviction; room → mirror admits; yield vs incumbency across light kinds and promoted cap (O7, O15) | lighting/render-cpu tests on `assign_slots_with_hysteresis` inputs + `plan_mesh_frame_plans` | achievable as stated | pass, after the review fix: a mirror-only instance is no longer a promotion receiver |
| 23 | Mirror-only light slot: pixels off selected plane match mirror-lighting-excluded frame | GPU test `planar_gpu_mirror_only_light_shows_only_in_the_reflection` | achievable as stated | pass |
| 24 | Settling on main fallback + completed mirror walk (O3): settle set holds mirror blocks; first world frame requests no new block | app test over `prepare_held_level_frame` + `BlockDemand::update_capture_view` | achievable as stated | pass |
| 25 | Area floor: below never selected; just above selectable | render-cpu test `planar_selection_respects_the_area_floor` | achievable as stated | pass |
| 26 | No seed in reach → not selected; fallback drawn-set reach (O12, O14); no probe → never | render-cpu + app tests | achievable as stated | pass (per-face seeds since review) |
| 27 | Eye below / in fog band → not selected, SSR at tier; none qualifying → none | render-cpu test `planar_selection_refuses_eye_below_or_in_band` | achievable as stated | pass |
| 28 | Hysteresis 1.25×; held plane leaving frustum yields (O13) | render-cpu test | achievable as stated | pass (hysteresis kept through a yield since review) |
| 29 | Plane switch frame (O4, O5), incl. trip on that frame | app + GPU test `planar_switch_frame_routes_new_plane_to_this_mirror` | achievable as stated | pass |
| 30 | Level change ignores old held plane (O11) | app test `planar_install_resets_selection_history` | achievable as stated | pass |
| 31 | High and below: no selection, walk, demand, pass, target (O8) | app + renderer tests | achievable as stated | pass (a held mirror-only slot is released the frame the mirror reach is empty, since review) |
| 32 | high → very_high → high: no pipeline, target alloc/release, no walk on drop frame; no-plane frames keep target (O10); resize incl. allocating frame (O9); unload | renderer tests with pipeline-creation counters + app tier test | achievable as stated | pass (the deferred build survives a resize that commits no scene change, since review) |
| 33 | Zero selected → nothing records; no candidate → no target; candidate → target from install; refresh removing last → release | renderer + app tests | achievable as stated | pass |
| 34 | M: capture at high byte-identical to base commit | capture A/B, base binary built from 25db4c78f | achievable as stated | pass: byte-identical at two lake poses, rerun on the final tree |
| 35 | M: cropped real-map mirror sweep worst ≤ ½ cap, no ordinary trip | `real_map_sweep` mirror mode on lake, e24, probes | achievable as stated | pass: worst 1,416 (3.53×), 0 trips, rerun on the final tree |
| 36 | M: cropped image matches uncropped (pixel, smooth); under-half-screen pose reaches fewer cells | capture with `crop: false` vs true; sweep cell counts | achievable as stated | pass: 8 of 8 pairs byte-identical, rerun on the final tree |
| 37 | M: bake-gap with planar on ≤ planar off, completed-reads; per-walk selected frames + mirror-only counts non-zero | `walk_measurement` mirror mode, lake + second fixture | achievable as stated | pass, unstressed: 0 vs 0 on lake and e24; neither fixture can defer a block |
| 38 | M: lake spawn settle: zero reveal misses, same plane, settle peak > planar off; mirror-only blocks outside baked set non-zero (or spawn override) | `[Settling] entry` new fields | achievable as stated | pass at the e24 override pose; the lake can't show the peak half (all 144 blocks mandatory). **Owner: accept e24 for this row?** |
| 39 | M: mesh-instance cap and shadow occupancy with/without mirror | capture report / instrumented run, same poses | achievable as stated | reported: identical with and without the mirror at every pose; no pose on today's maps admits a mirror-only light or instance |
| 40 | Manual: owner motion read, presets fixed and recorded | owner, in-engine | manual | outstanding: owner |
| 41 | Manual: Windows 1660 release perf matrix | owner run (`long-bake`/timed runs are owner-launched); executor prepares commands | manual | outstanding: owner |
| 42 | Manual: Mac 5300M native bounded run | owner, Mac | manual | measured 2026-10-26, CPU side only: very_high reaches world frames natively and holds 60 at vsync on in all 22 runs; planar adds 0.6–0.8 ms CPU work over SSR-only very_high. GPU (Instruments, native 1440p): planar costs 1.0–1.1 ms and removes a 7.4–7.6 ms SSR pass, so very_high is about 7 ms cheaper per frame than SSR-only very_high; ultra's planar cost equals very_high's. Vsync-off frame time is not measured. See §Owner handoff AC 42 |
| 43 | Manual: resident memory at 4K ultra lake pose | executor computes from allocation sizes; owner confirms in-engine | manual | computed (47.5 MiB at 4K ultra, scale ½); owner confirms |

## Owner handoff (Task 12)

Run from the worktree root. Maps are the main checkout's bakes: `M=C:/Users/danhi/Projects/Personal/postretro/content/dev/maps`. Poses are `--start-pose x,y,z,yaw,pitch`: capsule centre, engine metres, Y up. Options → Graphics → Reflection quality switches VERY HIGH and HIGH live.

**AC 40 — motion read (presets).**
```
cargo run -p xtask -- run --features dev-tools -- $M/reflection-lake-test.prl
cargo run -p xtask -- run --features dev-tools -- $M/e24-environment-test.prl
cargo run -p xtask -- run --features dev-tools -- $M/reflection-probes-test.prl
```
- **Style:** there is no dev toggle; style comes from the author. Pixel is the default. For smooth, add `reflections: { style: "smooth" }` to the `render` block of `content/dev/start-script.ts`, then revert it after.
- **Live tuning:** use the dev panel, Lighting tab → "Planar reflection": off, crop, scale ¼/½/1, D and R with bands, reset. Launch seeds (dev-tools): `POSTRETRO_PLANAR_SCALE`, `_D`, `_R`, `_BAND_D`, `_BAND_R`, `_OFF`, `_CROP`.
- **Plane switch:** the lake and e24 each have one candidate plane. Read the switch on `reflection-probes-test`, between the canal (119 m²) and the pool-room pool (23.8 m²).
- **Poses:**
  - lake: spawn; `-16.87,1.7,-48.77,90,-10` (south shore looking north); `-48.77,1.7,-16.26,0,-3` (west shore looking east, grazing); `-79.25,1.0,-65.02,-90,-30` (boathouse).
  - e24: spawn; `-6.50,1.6,-1.63,0,-25` (pool).
- **Look for:**
  - waterline seam, shimmer, grazing precision;
  - posts and far walls as they leave the main screen;
  - **known gaps:**
    - SDF-shadowed static lights reflect unshadowed.
    - Sprites over empty mirror texels are invisible.
    - Mirror world draws are CPU-culled. The GPU indirect cull needs your sign-off on `indirect_contract_production_consumers_keep_guarded_ownership`.
- **Movers, skinned meshes, sprites and particles:** e24 has none. Use `campaign-test` (dynamic spots, meshes near water) and `spawner-test` (particles) if either has water in reach; otherwise this half of the read waits on a fixture.
- **Record here:** the fixed preset per tier (scale, D, band_d, R, band_r), and update `PLANAR_PRESET_START` / `PLANAR_PRESETS` in `render-cpu/src/planar_reflection.rs`.

**AC 41 — GTX 1660 perf matrix** (release, VSync off, window 1600×900 windowed, render resolution Native, window unoccluded; report, don't gate).
```
cargo build --release -p postretro
RUST_LOG=info POSTRETRO_GPU_TIMING=1 POSTRETRO_CPU_TIMING=1 target/release/postretro.exe --windowed --start-pose <pose> $M/<map>.prl
```
- **Order:** very_high then high at each pose, then repeat in reverse order (high then very_high).
- **Read:** frame time; the `mirror_walk` CPU stage (app) and the `rec_planar` CPU stage (renderer: the mirror's CPU cull and recording), both in `[CpuTiming]`; the `planar_reflection` GPU pass and the SSR pass (GPU timing lines).
- **Poses:** lake across `-48.77,1.7,-16.26,0,-3`; lake down `-16.87,1.7,-48.77,90,-30`; one movers-and-particles pose on the map chosen above.
- **Pairing:** compare very_high with planar against very_high with SSR only. The latter is `POSTRETRO_PLANAR_OFF=1`, a dev-tools seed. So either run both sides on a release build with `--features dev-tools`, or compare against high and say so.

- **Match the Mac's scene size**, or say so: the Mac ran a 1280×720 scene (see AC 42).

**AC 42 — Mac 5300M native run.** The protocol was agreed with the Mac session and run there on 2026-10-26 at 4d579df16.

- **Setup:**
  - Bake on the Mac with the release compiler; `.prl` files aren't committed. `cargo run --release -p postretro-level-compiler -- content/dev/maps/<map>.map -o content/dev/maps/<map>.prl` for `reflection-lake-test` and `e24-environment-test`.
  - Build with `cargo build --release -p postretro --features dev-tools` and `cargo build --release -p postretro-script-compiler --bin scripts-build`.
  - A release build needs `content/dev/start-script.js`. Make it with `scripts-build --in content/dev/start-script.ts --out content/dev/start-script.js`.
- **Launch (one process per configuration; seeds are read once at launch):** `RUST_LOG=info POSTRETRO_CPU_TIMING=1 POSTRETRO_GPU_TIMING=1 [seeds] target/release/postretro --app-name postretro-t11-<tier> --mod dev --windowed --start-pose=<pose> maps/<map>.prl`.
  - With `--mod dev`, the map path is relative to the mod folder.
  - Write `~/Library/Application Support/postretro-t11-<tier>/settings.toml` first. It holds `reflection_quality` (`very_high` or `high`), `window_mode = "windowed"`, `render_resolution = "auto"`, `shadow_quality = "high"` and `accessibility_panel_shown = true`.
  - VSync starts on. Only the Alt+Shift+V chord turns it off; there's no env var or flag. The window title shows `vsync:on|off`.
- **Matrix:**
  - lake across (`-48.77,1.7,-16.26,0,-3`) and lake down (`-16.87,1.7,-48.77,90,-30`) × {very_high, very_high with `POSTRETRO_PLANAR_OFF=1`, high}, run forward then reversed;
  - lake across × {`POSTRETRO_PLANAR_D=15`, `POSTRETRO_PLANAR_CROP=0`};
  - lake down × {`POSTRETRO_PLANAR_SCALE=0.25`, `=1`};
  - the e24 override pose (`-21.95,1.0,-44.30,180,-35`) × {very_high, high}, for the reveal.
- **Procedure:** a pilot before each configuration and a thermal cooldown between runs. Read a steady window after the `[Settling] entry` line.
- **Read:** `[CpuTiming]` frame total, CPU work, `wait_acquire`, `mirror_walk` and `rec_planar`; GPU `planar_reflection`, `planar_forward` and the SSR passes; the settle line.
- **Machine:** Radeon Pro 5300M, Metal, on AC power, thermal speed limit 100 before every run. Window 1280×720 client on a 5K display, `render_resolution = "auto"`: surface 2560×1440, scene 1280×720, divisor 2. Fresh Mac bakes, default bake mode; a cold cache wasn't confirmed.

**Results.** All 22 runs are valid. Each figure is the mean of the forward and reversed runs, in ms, at vsync on.

| Pose | Config | Frame | CPU work | `wait_acquire` | `mirror_walk` | `rec_planar` |
|---|---|---|---|---|---|---|
| lake across | very_high | 16.74 | 5.73 | 10.95 | 0.314 | 0.067 |
| lake across | very_high, planar off | 16.88 | 4.92 | 11.91 | – | – |
| lake across | high | 16.86 | 5.10 | 11.70 | – | – |
| lake down | very_high | 16.61 | 5.66 | 10.90 | 0.317 | 0.070 |
| lake down | very_high, planar off | 16.74 | 5.06 | 11.62 | – | – |
| lake down | high | 17.02 | 5.00 | 11.96 | – | – |
| e24 override | very_high | 16.57 | 4.65 | | | |
| e24 override | high | 16.70 | 4.59 | | | |

- **Reveal:** every run settled, with `step_limit_frames=0` and `reveal_misses=0`.
  - Lake very_high, every variant: `walk_frames=15 lightmap_set_peak=128 planar_held=-1.219 planar_reveal=-1.219 mirror_only_blocks=0`. Lake high and planar off: peak 128, `planar_held=none`.
  - e24 very_high: `walk_frames=16 sh_set_peak=9 lightmap_set_peak=80 planar_held=-0.406 planar_reveal=-0.406 mirror_only_blocks=1`. e24 high: peak 79. This reproduces the Windows peak-above-planar-off result (Windows 74 vs 72, `mirror_only_blocks=2`).
  - At e24 very_high, steady `[CpuTiming]` has no `mirror_walk` or `rec_planar`. That fits the pawn sinking into the pool after reveal (AC 38 notes): with the eye below the water, no plane is selected. Not verified.
- **60 fps:** holds at vsync on in all 22 runs. Single-frame spikes of 20–69 ms appear in every configuration, high included, so they aren't planar's.
- **Planar's CPU cost:** very_high adds 0.6–0.8 ms of CPU work over planar off. `mirror_walk` plus `rec_planar` account for about 0.38 ms of it. The rest is other mirror preparation, not broken out.
- **D, lake across:** D=15 cuts `mirror_walk` from 0.314 to 0.026 ms and `rec_planar` from 0.067 to 0.023 ms; CPU work goes from 5.73 to 5.40.
- **Crop, lake across:** `mirror_walk` is 0.314 with the crop and 0.312 without, so the crop saves nothing at this pose and a tighter pad has nothing to cut on CPU there.
- **Scale, lake down (¼ / ½ / 1):** `mirror_walk` 0.32 / 0.32 / 0.32 and `rec_planar` 0.071 / 0.070 / 0.072, so CPU cost is flat. `wait_acquire` is 10.73 / 10.90 / 11.09; scale 1's first run had a 62.8 ms spike, so its mean is inflated.
- **Not measurable on this Mac:**
  - GPU timing. The adapter lacks `TIMESTAMP_QUERY` or `TIMESTAMP_QUERY_INSIDE_ENCODERS`, so `planar_reflection`, `planar_forward`, the SSR passes and the seven-pass overhead are unavailable.
  - Vsync-off frame time. Scripting the chord needs macOS Accessibility permission, which the run didn't have. VSync state was read from the log (`[Renderer] vsync on`).
- **No movers-and-particles pose:** campaign-test's water is about 900+ map units from its movers, and spawner-test has no water.
- **Raw logs** are on the Mac, in the Mac session's scratchpad (`ac42/`: per-run logs, `runs.txt`, `parse.py`).
- **What this settles for the optional decisions:**
  - GPU indirect mirror cull: `rec_planar` is about 0.07 ms, far below a 1 ms bar, so the CPU cull stays.
  - D at very_high: D=15 saves about 0.29 ms of CPU on the Mac. That's small, and it changes what the reflection shows, so it's a motion-read call (AC 40).
  - Per-style crop pad: no case at the lake, on CPU or GPU (GPU below).

**GPU results (Instruments Metal System Trace, 2026-10-26, 4d579df16).** All 26 captures were valid on the first try, plus the e24 ultra settle run.
- **Method:**
  - §12's `--launch` recording never finalised for the windowed engine (xctrace at 100% CPU, a 40 GB ktrace, export failed). So the run launched the engine, waited for `[Settling] entry` plus 5 s, then ran `xctrace record --attach <pid> --time-limit 12s` and read the trailing ~10 s (about 465 frames).
  - Frames counted only where exactly one Depth Pre-Pass, Textured Pass and Fluid Surface Pass encoder appears (71–89% of frames).
  - Extent native: surface and scene 2560×1440, divisor 1, chosen to saturate the GPU. This differs from the CPU batch's 1280×720.
  - Settings dirs `postretro-t11-vhn`, `-hn` and `-ultra` with `render_resolution = "native"`.
  - GPU clock pinned at 1227–1230 MHz busy median; GPU temperature median 87–94 °C, max 96–100. Frames were GPU-bound at 20 ms or more under vsync, so the GPU didn't idle.
  - Minimum free VRAM during runs: 488 MB.
- **Table:** GPU ms per frame, mean of the forward and reversed runs. "Rest" is planar total minus planar world forward. Run-to-run spread on planar total is about ±0.01 ms.

| Pose | Config | Planar total | Planar forward | Rest | SSR (Fluid Reflection Pass) | Fluid Surface | Main Textured | GPU frame |
|---|---|---|---|---|---|---|---|---|
| lake down | very_high | 1.134 | 0.356 | 0.778 | – | 4.452 | 10.354 | 20.47 |
| lake down | very_high, planar off | – | – | – | 7.585 | 4.513 | 10.351 | 27.41 |
| lake down | high | – | – | – | 4.091 | 4.449 | 10.227 | 23.34 |
| lake down | ultra | 1.137 | 0.379 | 0.758 | – | 4.461 | 10.522 | 20.66 |
| lake down | ultra, planar off | – | – | – | 10.874 | 4.534 | 10.457 | 30.98 |
| lake across | very_high | 0.966 | 0.330 | 0.636 | – | 3.718 | 10.895 | 20.23 |
| lake across | very_high, planar off | – | – | – | 7.369 | 3.786 | 10.961 | 27.24 |
| lake across | high | – | – | – | 3.983 | 3.756 | 10.804 | 23.35 |
| lake across | ultra | 0.966 | 0.358 | 0.609 | – | 3.719 | 10.888 | 20.24 |
| lake across | ultra, planar off | – | – | – | 10.645 | 3.826 | 10.995 | 30.81 |

- **Net:**
  - At very_high the selected plane is all the lake's water, so the SSR pass disappears entirely. The GPU frame is 20.2–20.5 ms with planar, 27.2–27.4 SSR-only, and 23.3 at high.
  - Ultra's planar cost equals very_high's. Ultra SSR-only costs 10.6–10.9 ms.
  - Every configuration misses 16.7 ms at native 1440p, high included. The main forward pass alone is 10.3–10.9 ms, and the Skinned Viewmodel Pass about 2.2 ms (pilot).
- **Per planar pass, lake down, scale ½ (ms):** Water Mask 0.055, World Prepass 0.029, World Forward 0.356, Skinned Meshes 0.502, Resolve 0.193.
  - Movers and Billboards recorded no Metal encoders. That matches `record.rs`, which opens them only when they have draws.
- **Scale, lake down (¼ / ½ / 1):** planar total 0.500 / 1.134 / 3.449; forward 0.174 / 0.356 / 0.853; Skinned Meshes 0.181 / 0.502 / 1.805; GPU frame 19.69 / 20.47 / 22.64.
- **Crop, lake across:** planar total 0.966 with the crop, 0.965 without, so no difference on the GPU either. The per-style crop pad has nothing to save at these poses.
- **e24 ultra:** "Reflection quality: Ultra", `[Settling] entry outcome=settled frames=16`, and world frames reached (`[CpuTiming]` total 17.7–20.4 ms).
- **Open finding: Planar Skinned Meshes costs 0.36–0.50 ms at scale ½, the largest planar pass after forward.** It grows close to target area (1 : 2.8 : 10 against 1 : 4 : 16).
  - The Mac session read it as empty-pass attachment cost. `record.rs` contradicts that: the pass opens only when the frame has a mesh plan, and it draws only the instances whose mirror reach is drawn (`MeshPass::record_mirror_draws`).
  - On an immediate-mode AMD GPU, the load and store of an empty pass shouldn't cost 0.5 ms. Growth with area fits fill on real draws.
  - **Investigated 2026-10-26 (Windows run with temporary logging, then reverted): the pass draws nothing at that pose.**
    - The world mesh plan holds two instances: the local player's shadow-only body (`exo_red`) and its shotgun attachment. Both have mirror reach `Hidden`, so `record_draws_where` binds state and issues no draw. Every logged sample read `mirror_drawn=0`.
    - The pass opens because `record.rs` opens it whenever the plan is non-empty.
    - Excluding the body is deliberate: `mesh_render.rs` passes `with_mirror = !mesh.shadow_only` ("A shadow-only body never draws in colour, the mirror's included"), and the test `collect_with_mirror_keeps_shadow_casters_main_ranked` pins it.
  - **The 0.5 ms is probably misattributed, not this pass's own work.** In older Mac traces of the same scene (`context/plans/ready/occluder-first-partition/measurement/frame-cost/results/gpu/`), the empty main Skinned Mesh Pass swings from 0.020 to 0.764 ms while Textured plus Skinned stays about 3 ms. That fits an empty encoder absorbing the previous heavy encoder's tail. Here it follows Planar World Forward. Not verified on the Mac.
  - **Proposed guard (not applied):** add `MeshFramePlan::any_mirror_drawn()` and open the pass only when it's true. The same guard could apply to the main Skinned Mesh Pass on `forward_visible`. The expected real saving is small (one encoder and its Load/Store, about 0.02–0.07 ms). To confirm it, re-trace and compare planar total, not the per-pass split.
  - **Owner answer (2026-10-26): the body reflects** (see Delegated answers, "Owner direction"). At the lake pose this pass now draws the body and its shotgun, so the skip guard no longer applies there. It still helps any frame whose plan has nothing the mirror reaches.
- **The seven-pass merge item:** apart from Skinned Meshes, the non-forward passes total about 0.28 ms at scale ½ (mask, prepass, resolve). A merge has little to save.
- **Not measured:** the 1280×720 auto extent, a quarter of the pixels. By the scale rows, planar there should be well under 1 ms.
- **Raw results** are on the Mac, in the Mac session's scratchpad (`ac42-gpu/out/`: `results.md`, `aggregate.json` and per-run `runs/*.run.json`). Raw traces were deleted.

**AC 43 — 4K ultra resident memory (computed; owner confirms).**
- **Basis:** scene extent 3840×2160 (Native, the worst case; Auto picks divisor 2 on a 4K panel, a 1920×1080 scene, which quarters every figure) at the ultra preset scale ½ gives 1920×1080 = 2,073,600 texels.

| Allocation | Format | Bytes |
|---|---|---|
| Planar Mirror Color | Rgba16Float | 16,588,800 |
| Planar Mirror Depth | Depth32Float | 8,294,400 |
| Planar Reflection Layer | Rgba16Float | 16,588,800 |
| Planar Reflection Layer Depth | Depth32Float | 8,294,400 |
| **Total** | | **49,766,400 (47.5 MiB, 2.3% of 2 GB)** |

- **At scale 1:** 199,065,600 B (189.8 MiB, 9.3%).
- **Per-view cull buffers:** none on the GPU; the mirror cull runs on the CPU.
- **Small buffers:**
  - mirror lights: same size as the main light buffer, 64 B per record;
  - mirror sprite instances: start at 1 KiB, grow monotonically;
  - planar inputs: 272 B.
- **Confirm in-engine:** at 4K ultra on the lake, check against a GPU memory readout.

## Review loop

**Preflight.** On the branch after the merge of `origin/main` (0c5116ba3): fmt clean; clippy clean in default, dev-tools and capture.

**Round 1.** A 12-agent panel. Depth reviewers (opus): IP contract verifier; five tracers (frame record, target lifecycle, selection/crop/walk, app preparation/demand/Settling, the app→renderer reach seam); an adversarial pass on admission yield; and a performance pass the owner asked for. Plus four hygiene reviewers (sonnet).
- **🔴 Fixed.** A `MirrorReach::Only` mesh instance counted as a promotion receiver. Turning planar on could promote a main-view baked light and evict another (AC 22). Two reviewers found it independently.
- **🟡 Fixed (correctness):**
  - Seeds tested the plane's union box rather than its faces.
  - The empty-crop and NoSeeds guards held a stale selection.
  - A deferred target build was lost when the pending resize committed no scene change.
  - A held mirror-only slot persisted about 0.8 s after the mirror went away.
  - The mover-ghost cull read mirror-widened bounds.
  - Capture handed the mirror light reach over once per prepare.
  - The drawn set included the mirror's empty reach cells.
  - The IP scans left c, the fade and the mask unpinned.
- **Performance, done:**
  - a `rec_planar` CPU stage;
  - merged depth-prepass runs;
  - a mirror frustum cull for movers;
  - one fused particle collection for both views;
  - binary-search cell membership;
  - linear unions;
  - mirror-only occupancy counted during planning;
  - reused buffers instead of per-frame allocations.
- **Splits.** Each kept its public paths: `render-cpu/mesh_instances/`, `render-cpu/planar_reflection/`, `capture/receivers.rs`, and `walk_measurement/{walk,install_timing}.rs`. The mirror-admission items moved to `renderer_mirror_lights.rs`. Shared constants are now pinned between visibility and render-cpu.

**Round 2.** Three agents over the fix diff.
- **🟡 Fixed:** the retry after a yield dropped hysteresis between the remaining planes.
- **🟢 Fixed:**
  - the Settling count now reads the mirror's drawable cells;
  - NaN scores rank last;
  - comment drift.
- **Left alone:** the main view's two level-slot Vecs allocate per frame (this predates the branch); the mover cull has a redundant clip test.

**Gate on the final tree (2026-10-26).**
- fmt clean.
- clippy clean in default, dev-tools and capture.
- Renderer lib, single-threaded with GPU required: 1,095 passed, 2 failed (the pre-existing ghost pair).
- `postretro` bin: default 2,120 passed, dev-tools 2,154, capture 2,194, each with 1 failure, the pre-existing font test.
- The other 67 test binaries: 9,816 passed, 0 failed.
- FXC: every full-init pipeline compiled.
- `cargo check --release` clean; crate graph up to date.

**Measured rows rerun on the final tree.**
- AC 34: byte-identical to the base, with the same hashes.
- AC 35: unchanged.
- AC 36: 8 of 8 pairs byte-identical, with the same hashes.
- AC 37: 0 vs 0. Mirror-only cell-frames are now drawable cells only: lake random walk 63,731, e24 123,057.
- AC 38: unchanged (e24 override: peak 74 vs 72, `mirror_only_blocks=2`).
- AC 39: unchanged.

**Merge of `origin/main` 3275d1480 (merge commit 53c7756e3).** Clean, no conflicts. Main brought only `context/` changes: plan moves, the occluder-first-partition brief and its measurement scratch (whose `gran` crate is not a workspace member), and `context/lib` edits to `build_pipeline.md`, `index.md` and `rendering_pipeline.md`. No file outside `context/` differs from 06a41967a, so the compiled tree is the one gated above. The gate and measured rows were not rerun.

**For the owner (not done; each is a decision):**
- **D at very_high only.** A finite D cuts the mirror walk (research: D15 took it from 2.69 to 0.06 ms). Take it only if the Mac run shows `mirror_walk` matters. Mac result (AC 42): `mirror_walk` is 0.31 ms with D unlimited and 0.03 ms at D=15, about 0.29 ms of CPU saved.
- **Per-style crop pad.** Smooth style tilts nothing, so it could crop nearly tight. This reverses Task 4's single-pad choice and needs AC 17, 18 and 35 rerun.
- **GPU indirect mirror cull: not recommended on perf grounds.** On Metal every indirect slot is a driver draw, so the CPU cull's merged runs are likely cheaper. Reopen it only if the `rec_planar` numbers argue for it. Mac result (AC 42): `rec_planar` is about 0.07 ms, so they don't.
- **AC 38 row.** Accept the e24 override pose in place of the lake? The lake makes all 144 blocks mandatory.

**Follow-ups (not this branch):**
- `renderer_light_slots.rs` is about 2,245 non-test lines (pre-existing); it needs a responsibility split.
- The renderer's planar compose `union_cells` re-sorts main ∪ mirror; the app's merged drawn set could be handed over instead.
- Cluster a huge plane's faces into rectangles for the crop (bench at about 1k faces first).
- The layer depth target may be redundant (8 MB at 4K ultra ½).
- Seven render passes per planar frame; measure before merging.
- A fixture that admits mirror-only lights and instances, so AC 39 reads a non-zero yield in a live run.

**Full gate after the body-mirror change and the merge of `origin/main` 45c7670cc (merge 878a01576), 2026-10-27:**
- fmt clean; clippy `-D warnings` clean in default, dev-tools and capture.
- Workspace tests excluding renderer and postretro: 0 failures.
- Renderer lib, single-threaded: 1,095 passed, 2 failed (the pre-existing ghost pair).
- `postretro` bin, default / dev-tools / capture: 2,120 / 2,154 / 2,194 passed, each with only the pre-existing font failure.
- FXC: every full-init pipeline compiled. Release check clean; crate graph up to date.

This matches the baseline. The measured rows weren't rerun: High captures (AC 34) don't run planar, and crop parity (AC 36) compares two very_high runs that the body change affects equally.

**Landing docs drafted 2026-10-26, ahead of "land the plane":** every `context/lib` item below and in the earlier landing list is written. That covers `rendering_pipeline.md` §4, §7.3a, the tier table, §7.8 capture `planar_reflection`, §9, §10 and §12, plus `boot_sequence.md` §1, `player_options.md` §4, `index.md`, `resource_management.md` §9 and `entity_model.md` (`shadowOnly`). §12's "Planar cost" cites `plans/done/planar-water-reflections`, so the brief must land there. Left for landing: move the brief to `done/`, clean up, push.

**For landing (`context/lib`):** besides the items already listed:
- **§7.3a** names the shared horizon floor in `fluid_reflection_above_horizon` as the one direction-conditioned placement composed from the pinned helper. It also words "never c" as ip-check D1 does, including the empty-texel cut, and scopes "reads no probe table" to the planar passes: routing and selection read only whether a face binds a probe.
- **§12** gains `rec_planar` beside `mirror_walk`.
- **§7.3a content** lists the local player's shadow-only body (and its attachments) as reflected, unlike the viewmodel and ghosts. §7.3a pool rank still ranks shadow-only casters as main-view admissions.

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
| 12 | **Manual handoff.** Commands and poses for the owner motion read, 1660 perf matrix, Mac run and 4K memory count; record the owner's presets. ACs 40–43. | integrating executor + owner | 11 | prepared: commands, poses and the AC 43 count in §Owner handoff; owner runs and the preset record are outstanding |
