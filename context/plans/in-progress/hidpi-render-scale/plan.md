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
- Capture: `Renderer::new_offscreen` has no surface. Its extents are fixed at the requested size with divisor 1, and `set_render_resolution` / `set_scale_factor` are recorded but never move them. That satisfies "divisor 1 regardless of option".

## Delegated answers
- Label text and placement of the options row — "Render resolution" on the dev frontend's Graphics tab, after Surface Depth, as a radio set Auto / Native / 1/2 / 1/3 / 1/4 like the neighbouring graphics rows. It sits beside the other GPU-cost rows, and the ASCII fractions render in every bundled typeface.
- Covers-HUD switches: uniform flags or compile-time constants — uniform flags in `EffectUniform`, packed from one renderer-owned constant (all off). WGSL has no variant system (`rendering_pipeline.md` §8). A uniform also lets a test flip one switch without a second pipeline, at zero added GPU cost.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 scene extent = ceil(surface/divisor), ≥1×1, oversize divisor clamps | `render_extent` unit tests (render-cpu) | achievable as stated |
| A2 Auto divisor table (13 rows) | render_profile test: chokepoint policy → render-cpu resolver, real 1440 cap | achievable as stated |
| A3 1× ≤1440 rows at Auto → scene = surface | render_profile/render_extent unit test | achievable as stated |
| A4 resize rebuilds scene targets at scene extent, swapchain at surface; no scene target reads surface size | GPU-harness offscreen test (target sizes) + source-scan test allow-listing `surface_config.width/height` reads | achievable as stated |
| A5 option change / scale change + resize → exactly one rebuild | `ExtentState` commit unit tests | achievable as stated |
| A6 fog scatter dims from scene extent on resize and pixel-scale install (P10) | GPU-harness offscreen test | achievable as stated |
| A7 capture at requested resolution, divisor 1 regardless of option | GPU-harness offscreen test | achievable as stated |
| A8 UI into native UI layer w/ own depth; resolve sole gameplay swapchain writer; debug UI after; frontend composites | source-order tests on the frame recorder + UI-layer module | achievable as stated |
| A9 covers-HUD switches default off; opaque UI unchanged, translucent only via scene; one switch on → only that effect | GPU-harness resolve test | achievable as stated |
| A10 settings: missing → Auto, unknown → Auto, round-trip | options unit tests | achievable as stated |
| A11 exhaustive match, no wildcard | render_profile mapping + per-variant test | achievable as stated |
| A12 SDK typedefs (TS + Luau) carry `options.renderResolution`; dev frontend graphics tab reads/writes it | typedef fixture/drift tests + scripts-build compile of frontend-menu.ts | achievable as stated |
| A13 P1 2× boot → first frame scene = surface/2 with no scale event | `ExtentState` test seeded with window scale + source test that `Renderer::new` reads `window.scale_factor()` | achievable as stated |
| A14 P2 saved non-Auto in effect on first full-ready frame, no rebuild after full init | `ExtentState` test + source-order test (render resolution applied before `ensure_full_ready`) | achievable as stated |
| A15 P3–P6 single rebuild from final values; scale-only rebuilds iff divisor changes | `ExtentState` unit tests | achievable as stated |
| A16 P7, P8 zero resize / option change while minimized | `ExtentState` unit tests | achievable as stated |
| A17 P9 Auto has no history at 1440↔1441 | `ExtentState` unit test | achievable as stated |
| A18 bloom chain from scene extent after resize, option change, manifest reload (P11) | GPU-harness offscreen test | achievable as stated |
| A19 P13–P15 no stale UI; layer always swapchain-sized | GPU-harness UI-layer test (clear on empty composition) + layer rebuilt with the swapchain in the commit | achievable as stated |
| A20 P16 suspend/resume keeps extents | `ExtentState` test (fresh state from re-read scale + re-applied option) + source-order test | achievable as stated |
| A21 P17 camera and viewmodel aspect = scene aspect after resize and after option change | App-level unit test of the commit→aspect seam | achievable as stated |
| A22 non-divisible surface: each scene pixel covers exactly d×d, edge overshoot cropped | GPU-harness resolve test at odd sizes, d=2 and 3 | achievable as stated |
| A23 grep gate: no 1440 cap in renderer crate | source-scan test | achievable as stated |
| A24 windowless renderer extent change, read back every scene target size | GPU-harness offscreen test (same as A4) | achievable as stated |
| A25 opaque UI pixel reaches swapchain untonemapped | GPU-harness resolve test (run on the Mac) | achievable as stated |
| M1 Auto vs Native GPU/CPU ms on two maps | owner, Metal System Trace | manual |
| M2 crisp uniform upscale; HUD sharp under every value | owner, in-engine | manual |
| M3 shake/flash/vignette spare the HUD; HUD colours match | owner, in-engine | manual |
| M4 drag between displays updates scene extent, no stretched frame | owner, two displays | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Pure extent chokepoint in render-cpu: `RenderResolutionPolicy`, Auto divisor, scene extent, `ExtentState` record/commit machine (A1, A5, A13–A17, A20) | integrating executor | — | done — `render_extent` tests (13) |
| 2 | Renderer scene extent: record-only setters, frame-start commit, every scene target + fog/bloom/SDF/group-5 from the scene extent, integer nearest upscale in the resolve, capture pinned at divisor 1 (A4, A6, A7, A18, A22–A24) | integrating executor | 1 | done — `render_extent_gpu_test` (9), `resolve_composite_gpu_test` (4) |
| 3 | Binary wiring: scale factor read at `Renderer::new`, Resized/ScaleFactorChanged record-only, commit after the options update, camera + viewmodel aspect from scene extent (A13, A14, A20, A21) | integrating executor | 2 | done — `app::render_extents` tests (3) |
| 4 | Render-resolution option: `PlayerOptions` field, `options.renderResolution` slot, bridge live apply, render-profile chokepoint with the 1440 cap, full-init re-apply, SDK typedefs, dev frontend Graphics row (A2, A3, A10–A12) | worker | 1 | done (worker) — options/render_profile/splash_lifecycle/bridge/catalog/typedef tests |
| 5 | First-slice falsification: release build, Auto vs Native `[CpuTiming]` on the Mac (forward-pass ms stays with M1) | integrating executor | 3, 4 | |
| 6 | Native UI layer: sRGB premultiplied layer at surface extent, cleared every frame, own depth; recording moved out of `renderer_render_frame.rs`; resolve composites it (A8, A19, A25) | integrating executor | 2 | done — built with task 2; `ui_layer_is_cleared_every_frame_even_with_no_ui`, `opaque_ui_pixel_reaches_the_target_untonemapped`, source-order tests |
| 7 | Covers-HUD switches in `EffectUniform` + resolve, all off (A9) | integrating executor | 6 | done — `effects_with_switches_off_…`, `one_covers_hud_switch_…` |
| 8 | Preflight, review panel, fix loop, full gate | integrating executor | 1–7 | |
