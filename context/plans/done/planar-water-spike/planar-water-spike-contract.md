# Planar water reflection spike: contract

> **Kind:** build-to-learn spike (`context/lib/experimental_spikes.md`). The deliverable is a findings note and a recommendation, not a shipped feature. Nothing here folds into `context/lib/` unless the owner decides to ship.
> **Branch:** `claude/planar-water-spike`, cut from `main` at `473185e88`.
> **Every track reads this file first,** then `context_style_guide.md` and `development_guide.md` §2.

## Goal

Replace screen-space reflections (SSR) on large flat water with a planar reflection: the scene rendered a second time, from a camera mirrored across one horizontal water plane, into a screen-aligned target. The reflection probe still blends in. The spike measures whether planar plus probe beats SSR plus probe on GPU cost and image quality, and whether limiting the mirror's draw distance (D) and the water that receives it (R) buys performance at an acceptable visual cost.

## Decisions

| # | Decision | Why it carries weight |
|---|----------|-----------------------|
| 1 | The owner lifts `resource_management.md` §9 "no render-to-texture for mirrors" **for this spike only**. Leave §9 unedited. | A shipping decision later reopens it in `context/lib/`. |
| 2 | In planar mode, the SSR pass (`fluid_reflection`) records nothing for any water. The selected plane composites F·[c_p·Planar + (1−c_p)·s·Probe]. All other water is probe-only, like Low; per-face draw ranges bind the zero screen layer for nonselected planes. | The point is to replace SSR, not add to it. The savings are measured against SSR. |
| 3 | The planar target takes SSR's slot, group 5 binding 2 (`reflection_target`), as premultiplied (c_p·rgb, c_p). `fluid_surface.wgsl`, `fluid_reflection.wgsl`, the CPU reference in `render-cpu/src/fluid_surface/reflection.rs` and the text pins in `fluid_surface_tests.rs` stay **unedited**. | The composite contract (`fluid_reflection_layered_composite`) treats the screen layer as opaque (c·X, c). Zero shader churn keeps the comparison clean and the IP pins intact. |
| 4 | The swap lives in `FluidReflectionPass` as a planar override. `bound_view()` and `target_scale()` return the planar target. `wanted_reflection_target()` returns `None`, so the SSR target is released. The rebind goes through the existing `rebind_reflection_target`. | It keeps the "exactly two `scene_inputs.rebuild(` callers" pin and the `has_reflecting_draws` frame gate unchanged. |
| 5 | Planar mode is a dev override: a dev-panel toggle seeded from `POSTRETRO_PLANAR_REFLECTION=1`. No player-facing tier changes. Planar applies at every `reflection_quality` tier except Off. | Spike levers live in the dev panel. The env var makes headless and Metal System Trace runs possible. |
| 6 | The SDF shadow system is deprecated and is removed in a separate change. The mirror view must not depend on it: its uniform sets `sdf_shadow_flags = 0`, so forward skips the SDF upsample. Do not touch the SDF code. | The removal must not block or tangle with the spike. |
| 7 | One plane per frame, chosen automatically: up-facing faces of reflecting-variant volumes are grouped by height (1 cm tolerance) and scored by estimated solid angle, with 1.25× hysteresis. No plane is chosen when the eye is within `FLUID_FOG_BAND` of the plane or below it. No FGD or authoring changes. | It caps cost at one extra view whatever the map holds. |
| 8 | The mirror view draws the static world, movers and skinned meshes, lit by lightmaps, SH and dynamic lights that reuse this frame's shadow maps. It skips fog and smoke raymarching, billboards and particles, SSR, bloom and the viewmodel. | Cutting passes is a main GPU lever. Static world is always drawn (IP row 1). |
| 9 | A water mask makes the mirror pass shade only pixels on the selected, unoccluded water, within R. Mirror depth is cleared to 0, and the mask writes 1.0 where shading is wanted, so early-z rejects everything else. | This is expected to be the main fragment saving, and it is how R saves anything at all. |
| 10 | Visibility comes from a new `determine_mirror_visible_cells` that floods from seed cells and never calls `locate_cell`. Seeds are main-visible, non-solid cells that intersect the selected water's slab and XZ bounds, within R. On a step-limit trip, it frustum-culls against the mirror frustum (water plane plus far plane D). | The mirror eye usually sits in solid, which today triggers the solid-cell fallback and a warning every frame. D bounds the walk, because `narrow_frustum` keeps the far plane. |
| 11 | Mirror-visible cells are unioned into: mesh and mover collection, `ShSampleRegionSets`, lightmap residency demand, and the animated-lightmap compose input (§7.1 step 4). | Without the unions, reflections of cells the main camera can't see read stale or missing lighting. A spike with corrupt data teaches nothing. |
| 12 | Ripple-normal distortion of the planar lookup is **deferred**. | It would edit the pinned `fs_main_reflecting` and the screen layer. Pixel style already reads per-cell blocks. |
| 13 | Planar only reaches probe-bound faces, because the reflecting gate needs a probe. That's accepted for the spike; the lake fixture has one. | Water without a probe takes no reflecting variant today. |
| 14 | Owner clarification (2026-10-23): the intended image is planar plus probes. D/R deliberately hand distant reflection to probes; losing distant planar detail is acceptable. Judge near-field usefulness and the transition in motion, rather than fidelity to unlimited planar. | Large water should retain reflection when content leaves the main screen; the probe substrate is part of the design, not an error state. |

## IP constraints (hard; engineering triage, not legal advice)

| # | Rule | Source |
|---|------|--------|
| 1 | Split planar from probe by **distance only** (D, R). The mirror always draws the static world, never only dynamic content. | NetEase CN117496039A, listed in `context/plans/done/fluid-ssr-reflections/ip-check.md` |
| 2 | Mirror-pass recording stays on the frame's single encoder, single-threaded. | Tencent CN116778049B |
| 3 | The mask and the resolve cut on R, occlusion, coverage and D only. No Fresnel or view-direction threshold, skip or re-weight. | Microsoft US10586375B2 fence, `rendering_pipeline.md` §7.3a |
| 4 | The planar passes read no capture shadow s, probe colour, probe distance tile or `fluid_probe*` binding. | Tencent WO2026113651A1 claim 15 fence, §7.3a |
| 5 | SSR's own boundaries (D1–D8 in the SSR `ip-check.md`) stay intact for SSR. | — |

A source-scan test enforces rows 3 and 4 on the new shaders: no `fresnel`, no `fluid_probe` binding, and no `dot(` against a view direction.

## Invariants and conventions

- **Plane:** y = h, world up is +Y. The reflection matrix M maps (x, y, z) to (x, 2h − y, z). Mirror view V′ = V·M, mirror eye e′ = M·e.
- **Cull and walk matrix:** P(far = D)·V′. **Raster matrix:** FlipX · Oblique(P, water plane in V′ space) · V′ (Lengyel's method, z row only), with the raster far plane left at the default.
  - Plane points keep their main-camera screen x and y once the X flip is undone, so the target is screen-aligned.
  - M and FlipX each flip handedness, so their product has a positive determinant. The existing Ccw/Back pipelines draw the mirror view unchanged, and no new world pipelines are allowed.
  - The resolve undoes the X flip by reading the mirror texel at (W − 1 − i, j).
- **Clip side:** keep the side the eye is on. The clip plane sits at h − 0.02 m, which hides the seam where posts enter the water.
- **D** is the total reflected path length |e′ − p| in metres, where p is the reflected surface point reconstructed from mirror depth. It's at least the view depth, so faded content is never cut off by the far plane.
- **R** is the 3D distance |e − w| in metres, where w is the main-camera ray's hit on the plane.
- **Fade:** fade(x, L, b) = clamp((L − x)/b, 0, 1), with the band b in absolute metres. Default bands are 4 m for D and 4 m for R. D and R default to off, meaning unlimited, so the first measurements are a no-limit baseline.
- **c_p** = coverage · fade(dist, D, bandD) · fade(r, R, bandR). Coverage is mirror depth strictly between 0 and 1. Output is (c_p · clamp(rgb, 0, 65504), c_p).
- **Resolution scale** is relative to the scene extent, matching `ssr_scale`. Slider values are ¼, ½ and 1, default ½.
- **Per-view state:** the mirror has its own 128-byte group-0 uniform buffer and bind group (viewmodel precedent), its own candidate-cull pipeline, indirect buffer, cull-status buffer and `VisibleSpanRanges`, and its own depth and colour targets. Nothing shares a buffer with the main view, so "a later overlapping write wins" never applies.
- **Pass order:** after the skinned pass, before the fluid pass: mirror cull, mask, world prepass (Less), world forward (Equal), movers, skinned, then resolve. With planar on and no plane selected, a clear-only resolve leaves c = 0.
- **GPU timing:**
  - `TIMING_PAIR_PLANAR_REFLECTION`, labelled `planar_reflection`: an encoder span over cull through resolve.
  - `TIMING_PAIR_PLANAR_FORWARD`, labelled `planar_forward`: the mirror forward pass.
  - Every mirror pass gets a distinct "Planar …" label for Metal System Trace.
- **CPU stage:** `VisibilityStage::MirrorWalk`, labelled `mirror_walk`.
- **Dev panel:** planar sliders go in their own file beside `debug_ui/mod.rs`. That file is 1,511 lines, so don't grow it.

## Tracks (sequential, on the branch, warm target)

| Track | Owns | Establishes / consumes | Gate |
|-------|------|------------------------|------|
| **T1, CPU contracts** (`opus`) | `crates/render-cpu/src/planar_reflection.rs` and its tests; `crates/visibility/src/{visibility.rs, portal_vis.rs, cpu_stages.rs}` | Establishes `MirrorCamera` (eye, cull_vp, raster_vp, clip_plane, h), `PlanarCandidates`, `SelectedPlane`, `PlanarReflectionSettings` (on, scale, D, bandD, R, bandR), `select_plane`, `planar_fade`, `determine_mirror_visible_cells` | `cargo test -p postretro-render-cpu planar_reflection` and `cargo test -p postretro-visibility mirror`, each with a non-zero count. Covers: plane points keep x and y; oblique depth is 0 on the plane, in (0, 1) above it within D, below 0 beneath it; the raster matrix preserves the main projection determinant sign (the combined handedness correction is positive); fade end points; hysteresis; an eye below the plane gives no selection; an apex in solid floods from its seeds with no `SolidCellFallback` log (test-log-capture); cells beyond D excluded; empty seeds give an empty result. |
| **T2, renderer** (`opus`) | `crates/renderer/src/render/planar_reflection/` (new); `shaders/planar_mask.wgsl` and `planar_resolve.wgsl`; the override in `fluid_surface/reflection_pass.rs`; a new `fluid_surface/planar_mask.rs`; frame recording, pre-scene, extent, compute-cull accessor, timing pairs, init; the dev-panel file | Consumes T1. Establishes `set_planar_reflection_view(...)` and the settings handle | `cargo test -p postretro-renderer --features dev-tools planar` (GPU harness: c = 0 off the water and beyond R, c > 0 within R; an asymmetric object lands on the mirrored pixel; the SSR pass records 0 while planar is on; the source-scan IP test). `cargo test -p postretro-renderer --features dev-tools fluid_` passes with **no edits** to existing tests. `cargo test -p postretro-renderer dx12_fxc_compiles_every_full_init_pipeline`. `git diff --stat main -- crates/renderer/src/shaders/fluid_surface.wgsl crates/renderer/src/shaders/fluid_reflection.wgsl crates/render-cpu/src/fluid_surface/` is empty. |
| **T3, app wiring** (`opus`) | `crates/postretro/src/frame_loop/mod.rs`, `render_preparation.rs`, the install-time candidate table | Consumes T1 and T2. Does the unions in decision 11 | `cargo test -p postretro planar`. Run `RUST_LOG=info POSTRETRO_GPU_TIMING=1 POSTRETRO_PLANAR_REFLECTION=1 cargo run -p xtask -- run --features dev-tools -- content/dev/maps/reflection-lake-test.prl`. The `[GpuTiming]` window lists `planar_reflection` and `planar_forward` and no `fluid_reflection`. The log has no `SolidCellFallback` and no wgpu validation error. A mirror-only cell count is logged. Close the engine after reading the logs. |
| **T4, measurement** (orchestrator plus owner) | `context/plans/done/planar-water-spike/planar-water-spike-findings.md` | — | See the findings section. |

## Honesty gates (automated)

- Clean boot with planar on and off. Planar off is byte-identical in behaviour to `main`.
- No solid-cell warning from the mirror walk.
- The SSR pass records 0 times while planar is on.
- Existing fluid pins and binding budgets pass unedited.
- FXC compiles every pipeline.
- c = 0 off the selected water and beyond R.

## Measured findings (record, don't gate)

- GPU time: `planar_reflection` and `planar_forward`, against `fluid_reflection` at SSR High and Medium. Frame-time deltas.
- Sweeps: scale ¼, ½ and 1. D off, 30, 15 and 8 m. R off, 30, 15 and 8 m.
- Poses: looking across and looking down, on `reflection-lake-test`, `e24` (movers and skinned meshes) and `reflection-probes-test`.
- Hardware: GTX 1660 Super on this Windows machine. Radeon Pro 5300M through Metal System Trace (owner runs it).
- CPU: `mirror_walk` time and path (seeded or fallback), mirror-visible cells and leaves, mirror-only cell count, mask coverage %, draw calls, Mac `render_submit` delta.
- Manual visual read (owner): planar against SSR High; precision at grazing angles; the waterline seam; how visible the D and R fade bands are.

## Status and execution notes

- **Status (2026-10-22):** owner authorized implementation in this chat. Decisions and IP constraints surfaced before T1. T1–T3 implemented on the existing branch; local verification and findings recorded in `planar-water-spike-findings.md`. Hardware sweeps and owner visual acceptance remain outstanding. Local host is macOS; Windows measurements require the owner’s hardware.
- **Process:** follow `.claude/skills/build-session/SKILL.md` §3–§5, adapted for a spike. Run the tracks in order on this branch. Do one review track at the end, then `/preflight`. The final deliverable is the findings note with a recommendation (`experimental_spikes.md` §Deliverable). Don't open a shipping PR, and don't fold anything into `context/lib/`.
- **Environment:**
  - Agent edits on Windows can turn files into CRLF. Normalise to LF before running tests, because `include_str!` source-scan tests break on CRLF.
  - Close the engine and `xtask` processes as soon as a manual run's logs have been read.
  - Run focused tests only: `cargo test -p <crate> <filter>`, and require a non-zero test count.
- **Source map** (from the design sweep at `473185e88`; reopen each file before designing against it):
  - Composite: `fluid_reflection_layered_composite` in `shaders/fluid_reflection.wgsl:73`. It's called from `fs_main_reflecting` in `shaders/fluid_surface.wgsl:521`.
  - Screen layer: `fluid_reflection_screen_layer` at `:500`. It reads `reflection_target` (group 5 binding 2, `:177`) and scales by `fluid_scene.ssr_scale`.
  - Reflecting gate: `fluid_reflection_gate` at `:537`. It needs a probe.
  - SSR target ownership: `fluid_surface/reflection_pass.rs` — `bound_view` and `target_scale` at `:458`, `wanted_reflection_target` at `:555`, `rebind_reflection_target` at `:634`, `has_reflecting_draws` at `:705`, `reflection_pass_records` at `:805`.
  - Pins that must stay green: `crates/renderer/src/render/fluid_surface_tests.rs` — `:1630` and `:2033` (composite), `:3204` (rebuild callers), `:4737` and `:4915` (IP scan, screen layer).
  - Second-camera precedent: the viewmodel uniform and bind group in `mesh_pass.rs:1262` and `renderer_render_frame.rs:922`.
  - Main-view group-0 uniform: `build_uniform_data` and the SDF flag in `renderer_frame.rs:176` and `:216`. The forward SDF upsample skip is at `forward.wgsl:1153`.
  - Cull:
    - `candidate_cull.rs:446`: `dispatch` takes its buffers as arguments.
    - `compute_cull.rs`: the `bucket_ranges` field is at `:82`, and `draw_indirect_buckets` at `:774`.
  - Pipelines: depth prepass `renderer_init_pipelines.rs:376`, forward `:129`, mover `kinematic_brush.rs:413` (`record_draws` at `:813`), skinned `mesh_pass.rs:1186` (draws at `:1809`).
  - Frame order: the SSR slot sits after skinned and before fluid, at `renderer_render_frame.rs:663`. Main-pass bind groups are at `:519`. Animated-lightmap compose input is `renderer_pre_scene.rs:547`. Extent rebuild is `renderer_extent.rs:111` (resize before `:126`).
  - Visibility:
    - `determine_visible_cells` is at `crates/visibility/src/visibility.rs:599`.
    - The solid-cell fallback warning is at `:378`.
    - Flood start is `portal_vis.rs:290`; `narrow_frustum` keeps the far plane at `:761`; `MAX_PORTAL_WALK_STEPS` is at `:26`.
  - Timing pairs: `pipeline_layout.rs:189` (`TIMING_PAIR_COUNT`). Labels are at `renderer_init_resources.rs:610`.
  - App: mesh and mover collection `frame_loop/mod.rs:2015` and `:2091`; `ShSampleRegionSets` `sh_sample_regions.rs:30`; per-face normal `face_normal` in `render-cpu/src/fluid_surface/mod.rs`.

## Open questions

- Whether the seeded walk in the lake's open hall trips the step limit. Expected yes; the frustum fallback is accepted.
- Oblique-projection depth precision at grazing angles. If it z-fights, record it; `Features::CLIP_DISTANCES` is the larger fallback and is not in the spike's scope.
- Vertex and draw cost on Metal, where each indirect slot is a driver draw. A forward variant with Less+write and no prepass is a later lever, not in scope.

## Measurement automation extension (2026-10-22)

Owner authorized automated knob sweeps. Capture scenes accept optional `planar_reflection` controls (on, scale, D/R, fade widths), applied before mirror preparation and overriding the dev environment seed. This is measurement-only; player settings and pinned water shaders remain unchanged. See knob-sweep evidence and Windows handoff for bounds, defaults, replay and results.

Windows measurement checkpoint (2026-10-23): 128 focused checks passed, including adapter-backed planar proofs and explicitly executed FXC initialization. 144 matched captures plus four accepted live probes are recorded in `planar-water-spike-evidence/windows-2026-10-23/`. Large-water Vulkan GPU savings coexist with about 3 ms extra live CPU mirror-walk cost at unlimited D. Keep the shipping decision open; see findings for remaining evidence.

## Bounded live experiment (2026-10-23)

Owner authorized the next experiment after clarifying the hybrid image goal. Test live CPU/GPU/frame cost of bounded planar plus probe against unlimited planar plus probe and SSR High plus probe. Start with half-scale D15/D30, quarter-scale equivalents, and receiving-distance controls. Repeat in reverse order with the same binary, pose, extents and scene content. Use release optimization, ordinary streaming, and the release indirect-validation policy; keep previous dev/vsynced measurements separate.

Dev-tools launch seeds make the existing panel reproducible: `POSTRETRO_PLANAR_SCALE` accepts 0.25/0.5/1; `POSTRETRO_PLANAR_D` and `_R` accept 1–100 m or `off`; `POSTRETRO_PLANAR_BAND_D` and `_BAND_R` accept 0.1–16 m. They preserve defaults on malformed input, warn, and are ignored without dev-tools. Existing `POSTRETRO_PLANAR_REFLECTION=1` still enables the experiment. Effective settings log at initialization; capture scene settings retain precedence. `POSTRETRO_UNCAPPED=1` seeds the existing vsync diagnostic off on a windowed dev-tools renderer, and is ignored by offscreen/plain builds. No player setting, composite shader, GPU binding or visibility algorithm changes.

Completed: 16 uncapped release runs and eight accepted stills in `planar-water-spike-evidence/windows-bounded-live-2026-10-23/`. Half D15 cuts mirror CPU 2.749→0.061 ms and frame mean 7.830→5.070 ms; High is 4.828 ms at 1600×900. Half D15 is the motion-test candidate under the hybrid goal. Quarter D15 equals probes-only in this static pose; near-water scale behavior and moving-camera/entity acceptance remain open. See findings for complete provenance and limitations.
