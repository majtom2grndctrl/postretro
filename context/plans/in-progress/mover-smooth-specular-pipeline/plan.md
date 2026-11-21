# mover-smooth-specular-pipeline — plan of record

mode: compact
status: active
read at: 1a66523ae (no source change to c56f894e7; Decision reads reused)

## Corrections
- "A style committed before full init" → offscreen test renderers are full-ready at construction (`Renderer::new_offscreen`), so no test can commit before the first full init. `finish_full_init` rebuilds the full renderer through the same `build_full_renderer` path the first init takes; planning around it by proving the "before full init" and "before a full-renderer rebuild" halves of R2 with one commit → `finish_full_init` → install → frame case.

## Delegated answers
- Whether the mover's style word survives — **retired.** Each pipeline is its own style, so a word nothing reads would be a second style signal that can drift from the bound pipeline. Bytes 28..32 of `KinematicLightParams` become padding; `SpecularStyle::kinematic_word` and `KINEMATIC_SPECULAR_STYLE_SMOOTH` go; their tests move to pin pipeline selection.
- Albedo read before or after the loop in the smooth pipeline — **after.** Both placements screen at 119 VGPRs and time within noise in the M2 re-time (before: +0.12 ms setpiece, −0.03 ms ramp, identical output), so the spike's measured V5s placement stays.

## Shape (Task 1)
- One module, two fragment entries over shared code: `mover_surface` (the prelude, as fluid's `fluid_shade`), per-record helpers both loops call (`mover_light_cache_layer`, `mover_light_drive`, `mover_light_shadow`, `mover_light_diffuse`, `mover_light_n_dot_l`), one loop per style (`accumulate_pixel_direct`, `accumulate_smooth_direct`) and one composition per style (`mover_pixel_shade`, `mover_smooth_shade`). The pixel loop is today's with the smooth branch compiled out.
- Register screen (this machine, Radeon Pro 5300M): `fs_pixel` 127 VGPRs / 104 SGPRs, `fs_smooth` 119 / 104. Both hold 8 waves/SIMD under wave32. Today's single pipeline: 128.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 smooth shade vs frozen loop composed the old way, within the tint bound, every whole-loop case, entry's own composition, non-vacuous | `smooth_shade_matches_the_frozen_pre_change_loop_within_reassociation` (harness splices `accumulate_smooth_direct` + `mover_smooth_shade`); `fs_smooth`'s call into its loop pinned in `forward_and_mover_share_the_specular_texel_functions` | achievable as stated | pass |
| A2 pixel cases bit-exact, oracles unchanged | pixel cases in `kinematic_specular_test` (`assert_matches_oracle` bitwise for pixel; `untinted_pixel_specular_matches_the_frozen_pre_change_function`) | achievable as stated | pass |
| A3 forward smooth and untinted identity unchanged | `world_specular_test` identity tests, untouched | achievable as stated | pass |
| A4 per style: diffuse tint-invariant, tinted specular matches the tint function | `diffuse_is_identical_with_and_without_tint`, `tint_mix_matches_the_cpu_reference` | achievable as stated | pass |
| A5 mover pass builds exactly three render pipelines | `building_the_mover_pass_creates_both_beauty_pipelines_and_its_depth_pipeline` | achievable as stated | pass |
| A6 both pipelines build with cube arrays off; both entries validate in the no-cube source | same test (cube on and off, under a validation scope); `kinematic_shader_source_validates_without_cube_arrays` | achievable as stated | pass |
| A7 each record reports its bound style per view; live, mirror, first-frame rows read it | `bound_styles` report; `transported` in `specular_style_frame_tests` | achievable as stated | pass |
| A8 entry↔style source pin; report is the selecting value; mirror report present | `each_beauty_pipeline_builds_its_styles_entry_and_reports_the_style_it_selected_by`; mirror row requires one Main and one Mirror record | achievable as stated | pass |
| A9 smooth loop and its helpers use no implicit-derivative sample | `smooth_light_loop_and_its_helpers_take_no_implicit_derivative_sample` | achievable as stated | pass |
| A10 live change binds the other pipeline next frame, both directions, no extra creation | `a_live_specular_style_change_reaches_the_next_frame_without_a_rebuild` (S, P, S, P) | achievable as stated | pass |
| A11 mirror binds the main view's pipeline, change frame included | `planar_mirror_reads_the_committed_specular_style_on_the_same_frame` | achievable as stated | pass |
| A12 every selection reads the frame snapshot | `every_mover_pipeline_selection_reads_the_per_frame_specular_style_snapshot` (crate scan: exactly two record sites) | achievable as stated | pass |
| A13 committed before full init / rebuild / install / load in flight → first frame | `specular_style_reaches_the_first_frame_of_each_install`, `specular_style_committed_mid_build_reaches_the_published_level` | achievable as stated | pass |
| A14 smooth keeps highlights through Surface Depth Off→On; style change at Off leaves the march off | `sharpness_and_tint_hold_through_surface_depth_off_and_on` (specular reaches at every step); `surface_depth_toggles_and_specular_style_changes_leave_each_other_alone` over a Concrete surface map whose Off and On bytes differ | achievable as stated | pass |
| M1 register screen of both pipelines holds 8 waves/SIMD wave32 | `tools/metal_register_screen`, counts in its README | manual (run here) | pass: `fs_pixel` 127, `fs_smooth` 119 VGPRs |
| M2 clock-pinned re-time, both poses, smooth ≤ pre-feature, pixel ≤ 1a66523ae + 0.15 ms | `measurements/specular-mover-cost/` kit; arms: pre-feature smooth, 1a66523ae pixel, branch smooth and pixel, branch smooth with albedo before the loop | manual | pass: 36/36 runs valid. Smooth −2.45 ms (setpiece) / −2.01 ms (ramp) vs pre-feature; pixel −2.68 / −3.32 ms vs 1a66523ae. Albedo before ≈ after (+0.12 / −0.03 ms). Pixel output bit-identical to 1a66523ae; smooth within 1/255 on 2 / 8 px. Kit README §Re-time |
| M3 dev map movers unchanged per style; toggle with mirror in view, no stall | owner, in-engine | manual | outstanding |
| M4 §7.3, §4, §8 text | land-the-plane context update | manual | outstanding (at landing) |
| M5 kit and README on the branch, no hook commit | 4f7efaa3d; no branch commit touches `capture/setup.rs`, no `POSTRETRO_CAPTURE_SURFACE_DEPTH_OFF` in `crates/` | manual | pass |

## Review loop
- Panel: 2 correctness tracers, contract verifier, adversarial tester (opus), hygiene/drift (sonnet). No must-fix code finding; pixel bit-exact and style selection held under trace.
- Acted on: the A14 frame test was vacuous over the placeholder material (now a Concrete surface map with an Off≠On precondition); `fs_smooth`'s and `fs_pixel`'s calls into their loops were unpinned (now pinned in full); the A12 pin now scans the crate for record sites; stale comments naming the mover style word or the mover's `fs_main` (`full_renderer.rs`, `renderer_frame.rs`, `direct_sh_resources.rs`, harness, `tools/README.md`, screen README heading); dead `Output::Frozen` harness path; `#[repr(C)]` dropped from the hand-serialized params struct; `record_draws` doc states the caller's `has_view_draws` gate; `_pad` says why it stays.
- Left: two beauty pipelines now compile at every full init and full-renderer rebuild; no row prices that (reported to the owner).

## Verification
- fmt, `clippy --workspace --all-targets --features postretro/dev-tools,postretro/capture -D warnings`, `cargo check --release`, crate-graph, both reducers: pass.
- `POSTRETRO_REQUIRE_GPU=1 cargo test --workspace --no-fail-fast`: 17516 passed, 2 failed. Both are `postretro --test telemetry_run`, a windowed engine run that stalls 30 s because the screen was locked (`CGSSessionScreenIsLocked=Yes`); environmental.
- Same with `--features postretro/dev-tools,postretro/capture` on renderer and app: also `playback_ring_times_marked_drain_and_frame_encoders_on_the_gpu`, which requires timestamp queries the 5300M lacks; it skips without `POSTRETRO_REQUIRE_GPU`.

- M2 re-time (2026-11-21): hooked binaries for 1c7fc9e3a, 1a66523ae, 68843610d and an albedo-before arm in a throwaway worktree; 1 pilot + 2 interleaved rounds, 12 arms, 36/36 valid. The first 16 runs had the screen locked (kit gate lifted by a logging wrapper; headless captures complete under lock); per-run medians agree across locked and unlocked runs and the old/head arms reproduce the spike within 0.04 ms.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Smooth entry (V5s shape) and pixel entry (smooth compiled out) over shared code; both beauty pipelines built in `KinematicBrushPass::new`; screen tool names its entry; screen both (and the smooth albedo-before arm). Stop if either leaves the tier. | integrating executor | — | done: 127 / 119, both in tier |
| 2 | Split `kinematic_brush.rs` tests into a sibling file, own commit | integrating executor | 1 | done (269f84100) |
| 3 | Per-frame selection from the style snapshot, main and mirror; test-only bound-style report; retire the style word | integrating executor | 2 | done |
| 4 | Harness and readback tests: smooth composition row, pixel splices, tint rows, A9 source test | integrating executor | 1 | done: `kinematic_specular_test` 13/13 under `POSTRETRO_REQUIRE_GPU=1` |
| 5 | Frame tests: A5–A8, A10–A14 | integrating executor | 3 | done: renderer lib 1438 passed |
| 6 | Bring the kit from `spike/specular-mover-cost` without the hook commit | integrating executor | — | done (4f7efaa3d) |
| 7 | Preflight, review panel, fixes | integrating executor | 1–6 | done |
