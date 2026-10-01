# hidpi-render-scale — plan of record

mode: compact
status: active
read at: 2d183ec12

No source changed under `crates/`, `sdk/`, `content/` or `core/` between the brief's `read at` (c0def0e9e) and 2d183ec12, so the grounded Decision reads stand.

## Corrections
- Path: "UI blend is standard alpha" → the quad pipeline and glyphon (0.11, hardcoded `BlendState::ALPHA_BLENDING`) both blend colour `SrcAlpha/OneMinusSrcAlpha` and alpha `OVER` (`One/OneMinusSrcAlpha`, wgpu-types 29). Over a transparent clear that yields a premultiplied layer with correct coverage alpha. Planning around it by compositing `scene·(1−ui.a) + ui.rgb` in the resolve; no glyphon change needed.
- Path: "`WindowEvent::Resized` … the camera aspect is set beside it" → also set once in `resumed` from `window.inner_size()`. Both become record-only; aspect is taken from the committed scene extent instead.
- P6 ordering → the options bridge applies menu writes in `App::update_player_options` (main.rs gameplay frame, before `assemble_frame_eye`; frontend frame, before `render_frontend_frame`). The extent commit sits after that call and before the camera/view-projection is built, so a resize plus an option change in one frame rebuilds once. Renderer render entries also commit defensively (a no-op when nothing is pending), covering splash and loading frames.
- `fog_pixel_scale` install path (`Renderer::set_fog_pixel_scale`, renderer_lighting.rs) passes `surface_config` dims → it passes the scene extent.
- Capture: `Renderer::new_offscreen` has no surface. Recorded sizes and policies do move its extents (the windowless extent test relies on it), but both capture entries first call `commit_native_capture_extents`, which pins divisor 1 at the offscreen surface extent. That satisfies "divisor 1 regardless of option". Capture is offscreen-only, so the pin never touches a player's setting.
- Path: "the upscale crops only the overshoot" → the resolve anchors the upscaled scene at the top-left, so the overshoot (up to divisor − 1 surface pixels) is cropped from the right and bottom only. World-anchored presentations therefore project into the upscaled span (scene × divisor), not the window. A surface-centred HUD crosshair can sit up to (divisor − 1)/2 px from the scene centre (1.5 px at 1/4); accepted under A22.

## Delegated answers
- Label text and placement of the options row — "Render resolution" on the dev frontend's Graphics tab, after Surface Depth, as a radio set Auto / Native / 1/2 / 1/3 / 1/4 like the neighbouring graphics rows. It sits beside the other GPU-cost rows, and the ASCII fractions render in every bundled typeface.
- Covers-HUD switches: uniform flags or compile-time constants — uniform flags in `EffectUniform`, packed from one renderer-owned constant (all off). WGSL has no variant system (`rendering_pipeline.md` §8). A uniform also lets a test flip one switch without a second pipeline, at zero added GPU cost.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 scene extent = ceil(surface/divisor), ≥1×1, oversize divisor clamps | `render_extent` unit tests (render-cpu) | achievable as stated | pass — `render_extent` scene-extent tests |
| A2 Auto divisor table (13 rows) | render_profile test: chokepoint policy → render-cpu resolver, real 1440 cap | achievable as stated | pass — `auto_render_resolution_resolves_the_brief_divisor_table` |
| A3 1× ≤1440 rows at Auto → scene = surface | render_profile/render_extent unit test | achievable as stated | pass — `auto_render_resolution_keeps_one_x_surfaces_up_to_the_cap_native` |
| A4 resize rebuilds scene targets at scene extent, swapchain at surface; no scene target reads surface size | GPU-harness offscreen test (target sizes) + source-scan test allow-listing `surface_config.width/height` reads | achievable as stated | pass — `extent_changes_rebuild_every_scene_target_at_the_scene_extent`, `only_surface_consumers_read_the_surface_configuration_size` |
| A5 option change / scale change + resize → exactly one rebuild | `ExtentState` commit unit tests | achievable as stated | pass — `ExtentState` single-commit tests + windowless commit test |
| A6 fog scatter dims from scene extent on resize and pixel-scale install (P10) | GPU-harness offscreen test | achievable as stated | pass — `fog_pixel_scale_install_divides_the_scene_extent` + resize assertions |
| A7 capture at requested resolution, divisor 1 regardless of option | GPU-harness offscreen test | achievable as stated | pass — `capture_renders_at_its_requested_resolution_regardless_of_render_resolution`, `capture_entries_commit_native_extents_before_recording` |
| A8 UI into native UI layer w/ own depth; resolve sole gameplay swapchain writer; debug UI after; frontend composites | source-order tests on the frame recorder + UI-layer module | achievable as stated | pass — `gameplay_ui_records_into_its_layer_before_the_sole_swapchain_resolve` |
| A9 covers-HUD switches default off; opaque UI unchanged, translucent only via scene; one switch on → only that effect | GPU-harness resolve test | achievable as stated | pass — `effects_with_switches_off_…`, `one_covers_hud_switch_lets_only_its_effect_reach_the_ui` (GPU, run on the Mac) |
| A10 settings: missing → Auto, unknown → Auto, round-trip | options unit tests | achievable as stated | pass — options persistence tests (missing/unknown/round-trip) |
| A11 exhaustive match, no wildcard | render_profile mapping + per-variant test | achievable as stated | pass — `every_render_resolution_maps_to_its_renderer_policy`, `render_resolution_catalog_values_round_trip_through_slot_parser` |
| A12 SDK typedefs (TS + Luau) carry `options.renderResolution`; dev frontend graphics tab reads/writes it | typedef fixture/drift tests + scripts-build compile of frontend-menu.ts | achievable as stated | pass — typedef snapshot/drift tests; scripts-build bundles frontend-menu.ts; graphics-row structural test |
| A13 P1 2× boot → first frame scene = surface/2 with no scale event | `ExtentState` test seeded with window scale + source test that `Renderer::new` reads `window.scale_factor()` | achievable as stated | pass — `a_state_seeded_from_a_2x_window_starts_at_half_the_surface`; `Renderer::new` reads `window.scale_factor()` |
| A14 P2 saved non-Auto in effect on first full-ready frame, no rebuild after full init | `ExtentState` test + source-order test (render resolution applied before `ensure_full_ready`) | achievable as stated | pass — `a_policy_recorded_before_the_first_commit_needs_no_later_rebuild`, `full_init_applies_render_resolution_before_ensure_full_ready` |
| A15 P3–P6 single rebuild from final values; scale-only rebuilds iff divisor changes | `ExtentState` unit tests | achievable as stated | pass — P3–P6 `ExtentState` tests |
| A16 P7, P8 zero resize / option change while minimized | `ExtentState` unit tests | achievable as stated | pass — P7/P8 `ExtentState` tests |
| A17 P9 Auto has no history at 1440↔1441 | `ExtentState` unit test | achievable as stated | pass — `auto_crossing_the_row_cap_and_back_rebuilds_once_per_step` |
| A18 bloom chain from scene extent after resize, option change, manifest reload (P11) | GPU-harness offscreen test | achievable as stated | pass — resize/option assertions + `bloom_profile_change_keeps_the_chain_on_the_scene_extent_without_an_extent_rebuild` |
| A19 P13–P15 no stale UI; layer always swapchain-sized | GPU-harness UI-layer test (clear on empty composition) + layer rebuilt with the swapchain in the commit | achievable as stated | pass — `ui_layer_is_cleared_every_frame_even_with_no_ui`; layer rebuilt in the surface commit |
| A20 P16 suspend/resume keeps extents | `ExtentState` test (fresh state from re-read scale + re-applied option) + source-order test | achievable as stated | pass — `a_fresh_state_from_reread_inputs_restores_the_pre_suspend_extents` + full-init order test |
| A21 P17 camera and viewmodel aspect = scene aspect after resize and after option change | App-level unit test of the commit→aspect seam | achievable as stated | pass (structural) — `gameplay_frame_commits_extents_after_option_writes_and_before_the_camera`; aspect set unconditionally from the committed scene extent |
| A22 non-divisible surface: each scene pixel covers exactly d×d, edge overshoot cropped | GPU-harness resolve test at odd sizes, d=2 and 3 | achievable as stated | pass — `resolve_replicates_each_scene_pixel_into_a_divisor_square_and_crops_the_edge` (GPU) |
| A23 grep gate: no 1440 cap in renderer crate | source-scan test | achievable as stated | pass — `renderer_crate_holds_no_auto_row_cap` |
| A24 windowless renderer extent change, read back every scene target size | GPU-harness offscreen test (same as A4) | achievable as stated | pass — `extent_changes_rebuild_every_scene_target_at_the_scene_extent` (GPU) |
| A25 opaque UI pixel reaches swapchain untonemapped | GPU-harness resolve test (run on the Mac) | achievable as stated | pass — `opaque_ui_pixel_reaches_the_target_untonemapped` (GPU, run on the Mac); text/quad parity: `ui_text_through_the_layer_matches_text_drawn_into_scene_colour`, `ui_pass_quads_composite_as_straight_alpha_over_the_scene` |
| M1 Auto vs Native GPU/CPU ms on two maps | owner, Metal System Trace | manual | outstanding — owner (task 5's Auto-vs-Native check folds in here) |
| M2 crisp uniform upscale; HUD sharp under every value | owner, in-engine | manual | outstanding — owner |
| M3 shake/flash/vignette spare the HUD; HUD colours match | owner, in-engine | manual | outstanding — owner |
| M4 drag between displays updates scene extent, no stretched frame | owner, two displays | manual | outstanding — owner |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Pure extent chokepoint in render-cpu: `RenderResolutionPolicy`, Auto divisor, scene extent, `ExtentState` record/commit machine (A1, A5, A13–A17, A20) | integrating executor | — | done — `render_extent` tests (13) |
| 2 | Renderer scene extent: record-only setters, frame-start commit, every scene target + fog/bloom/SDF/group-5 from the scene extent, integer nearest upscale in the resolve, capture pinned at divisor 1 (A4, A6, A7, A18, A22–A24) | integrating executor | 1 | done — `render_extent_gpu_test` (9), `resolve_composite_gpu_test` (4) |
| 3 | Binary wiring: scale factor read at `Renderer::new`, Resized/ScaleFactorChanged record-only, commit after the options update, camera + viewmodel aspect from scene extent (A13, A14, A20, A21) | integrating executor | 2 | done — `app::render_extents` tests (3) |
| 4 | Render-resolution option: `PlayerOptions` field, `options.renderResolution` slot, bridge live apply, render-profile chokepoint with the 1440 cap, full-init re-apply, SDK typedefs, dev frontend Graphics row (A2, A3, A10–A12) | worker | 1 | done (worker) — options/render_profile/splash_lifecycle/bridge/catalog/typedef tests |
| 5 | First-slice falsification: release build, Auto vs Native `[CpuTiming]` on the Mac (forward-pass ms stays with M1) | integrating executor | 3, 4 | handed to owner — launches from the shell rendered no frames (machine idle/locked, the documented confounder); research.md's 640×360 measurement already supports the fill-bound premise |
| 6 | Native UI layer: sRGB premultiplied layer at surface extent, cleared every frame, own depth; recording moved out of `renderer_render_frame.rs`; resolve composites it (A8, A19, A25) | integrating executor | 2 | done — built with task 2; `ui_layer_is_cleared_every_frame_even_with_no_ui`, `opaque_ui_pixel_reaches_the_target_untonemapped`, source-order tests |
| 7 | Covers-HUD switches in `EffectUniform` + resolve, all off (A9) | integrating executor | 6 | done — `effects_with_switches_off_…`, `one_covers_hud_switch_…` |
| 8 | Preflight, review panel, fix loop, full gate | integrating executor | 1–7 | done — preflight green; panel 7 lenses, 0 🔴, fixes applied; full suite 8978 pass (1 pre-existing postretro-tool temp-dir flake, 5/5 on rerun) |

## Review loop
- Round 1: 7-lens panel (2 tracers, adversarial, contract verifier, seam tracer — opus; 2 hygiene/drift — sonnet). 0 🔴 code, 3 🟡 code, ~10 🟢, 2 🔴 + ~10 🟡 comment drift. All acted on except accepted items (8-bit sRGB UI blend banding risk; 0×0-at-build seeds 1×1, pre-existing; right/bottom crop offset ≤ (divisor−1)/2 px). No Decision or Acceptance change. No second round: fixes were mechanical, comment-only, or new tests, all green.
