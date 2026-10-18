# fluid-base-pass-cost — plan of record

mode: compact
status: landed
read at: 96ab6c9f4

No crate source changed between the brief's `read at` (2f1f96297) and this plan's base; the Decision reads stand.

## Corrections
- Path: "`ShVolumeResources` keeps neither origin nor cell size on the CPU … both values are plumbed to the packer from level data" → `ShVolumeResources` keeps `grid_origin`/`cell_size` CPU mirrors (dead-code-allowed today) beside `present`, and `ShResidencyState` keeps the manifest base (`base_metadata`). Planning around it by selecting the lattice inside the renderer at fluid install, through one seam beside `FullRenderer::sh_bind_group`: the streamed base when streaming, else the legacy section's when `present`, else none. The binary passes nothing new; a device-rejected or absent volume (`present == false`) yields no grid; the placeholder lattice is never read.
- Decision "Bounded … takes the finest spacing that fits" → clarification, same meaning: the cap counts an upper bound computed per face from its extent and the spacing alone, before any face is cut (P18), exact for a face no plane crosses (today's count). A gridded face therefore triangulates without added vertices (convex ear clipping that keeps every collinear vertex a corner) rather than today's centroid fan, so the packed mesh never exceeds the counted bound. A face no plane crosses keeps today's triangulation verbatim.
- Path "Large files: `fluid_surface.wgsl` is past 800 lines; split before extending" → split by responsibility into `fluid_surface.wgsl` (interface, shading, entries), `fluid_motion.wgsl` (warp, drift, wave) and `fluid_reflection.wgsl` (reflection term), concatenated by `SHADER_SOURCE`; source-pinning tests read the fluid texts through one helper.

## Delegated answers
- Grid spacing and per-level vertex cap — half a probe cell per axis (`FLUID_GRID_DIVISIONS = 2`), coarsening by doubling (planes nest), cap 2^18 vertices per level (`FLUID_GRID_VERTEX_CAP`, about a 256 m square ocean at 0.5 m). Kept from measurement: e24's 12 pools pack to 3,154 vertices on the 0.5 m grid, and the gridded pass costs 2.16 ms at near 4K against the no-SH floor's 2.05 and the coarse-mesh `vertexsh` probe's 2.11, so grid vertex work is about 0.05 ms; parity is already within 1/255 at that spacing, so no finer spacing is warranted.
- Parity bound — reported from measurement, no threshold set (brief): max 1/255 per channel at every view (Results).

## Measurement (5300M, Metal System Trace)
Kit: `measurements/fluid-base-pass-cost/metal/`, A/B through the experiment hook in one binary built from this branch at ef4ad05d7: `stock` = this branch's shader, `today` = `main`'s composed module (`landing_variants.py`), both on the gridded mesh; mask 0x13F (reflection-free), 2 interleaved rounds. Results: `results-landing1.{md,json}`; parity: `parity.py` → `results-landing-parity.json`.

| view | today fluid ms | new fluid ms | today whole frame | new whole frame | notes |
|---|---|---|---|---|---|
| near 4K | 53.87 | 2.16 | 79.62 | 27.93 | 2/2 valid each, clocks ~1150–1220 |
| probes 4K | 21.72 | 2.13 | 50.73 | 25.92 | 2/2 valid each |
| near Auto 1536×960 | 9.90 | 0.51 | 14.96 | 6.16 | new arm's clock low (998; 81 median r2): directional |
| probes Auto | 4.14 | 0.54 | 9.98 | 7.53 | new 1/2 valid, clock ~550–590: directional |
| mid Auto | 4.18 | 0.54 | 9.20 | 8.20 | 1/2 valid each, new clock ~577: directional |

Auto-resolution runs do not saturate the GPU, so its clock drops (`rendering_pipeline.md` §12); those rows show direction, not exact ratios.

Parity at 960×540, |today − new| in 8-bit sRGB over water pixels (water mask from a flat-colour variant; near-wall = world point within one spacing of a pool wall, from the ray's hit on the pool surface and the maps' fluid brush rectangles):

| view | water px | max (r, g, b) | mean (r, g, b) | near-wall px | near-wall max | near-wall mean |
|---|---|---|---|---|---|---|
| near (e24) | 518 400 | 1, 1, 1 | 0.002, 0.004, 0.004 | 0 (walls off screen) | — | — |
| probes | 518 400 | 1, 1, 1 | 0.004, 0.005, 0.005 | 0 (walls off screen) | — | — |
| mid (e24, pitch −30) | 258 805 | 1, 1, 1 | 0.002, 0.004, 0.003 | 41 075 | 1, 1, 1 | 0.004, 0.006, 0.005 |
| under (e24, 1 m below, pitch +60) | 518 400 | 1, 1, 1 | 0.000, 0.001, 0.001 | 0 | — | — |

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 fragment entries sample no SH atlas / probe depth / SH grid; `vs_main` samples each; runtime lights in both fragment entries; reflecting entry keeps the shared sampler; per-fragment pins inverted | `only_the_vertex_entry_samples_the_sh_volume` (naga `global_uses`), `fluid_baked_light_is_per_vertex_and_runtime_lights_per_fragment`, `the_reflection_free_variant_compiles_no_reflection_into_its_executable_path` | achievable as stated | **pass** |
| A2 layout keeps groups, entries, visibilities and vertex stride | existing layout/stride/binding tests unedited; no BGL builder in the diff | achievable as stated | **pass** |
| A3 grid packing (areas, planes, UV, shared edges P9–P12, T-junctions P13, originals P10, contiguity + streams P15, uncut face, cap P5–P7 P17 P18) | `fluid_surface::grid` (24 tests), `probe_indices_cover_every_grid_vertex_of_a_bound_face` | achievable as stated | **pass, one accepted limit:** P13 on an edge at a shallow angle to a plane whose T-vertex lies within tolerance of that plane: the long side gains a crossing up to (tolerance + T offset)/sin θ along the edge from the T-vertex, on the open edge's own line (crack no wider than the compiler's T-junction). Owner-accepted at landing (Review loop). Area sums hold to f32 packing (~1e-4 relative at 1 km on oblique edges). |
| A4 lattice the SH sampler uses (section's or manifest base, never placeholder, P3) | `the_fluid_grid_takes_the_lattice_the_sh_sampler_reads`; adapter: `the_fluid_grid_takes_each_levels_sh_lattice_and_a_level_without_one_draws_ambient_only`, `a_streamed_level_cuts_its_fluid_on_the_manifest_base_lattice` | achievable as stated | **pass** |
| A5 no usable SH volume → packs as today, ambient only, no error (P4, P20); no fluid → no grid, no log (P14) | `no_fluid_or_no_lattice_builds_no_grid_and_logs_nothing`, `an_unusable_lattice_is_refused`; adapter: zero-dimension, zero-cell and NaN-origin levels draw the ambient floor alone (renderer now refuses a degenerate lattice) | achievable as stated | **pass** |
| A6 fluid install after geometry install on level and capture paths; level change repacks on its own lattice / cap warning (P1, P2, P19) | `fluid_surfaces_install_after_level_geometry_on_the_level_and_capture_paths`, `a_second_level_takes_its_own_spacing_and_warning`, adapter level-change test | achievable as stated | **pass** |
| A7 GPU frame tests: variant parity; uniform field vs formula within 1/255 across scales and masks incl. fog; viewer side above/below/crossing on both variants (P16) | `fluid_baked_frame_test` (4 adapter tests, ran on the 5300M Mac, none skipped) | achievable as stated | **pass** (worst 1/255; reflection composite 2/255) |
| A8 budget pins pass unedited, vertex stage included | `pipeline_budget_tests` unedited (one comment line reworded) | achievable as stated | **pass** |
| A9 diff touches no level-compiler, level-format or wire code | `git diff --stat main...HEAD -- crates/level-compiler crates/level-format crates/net crates/netcode` empty | achievable as stated | **pass** |
| M1 5300M near + probes 4K ≤ 3.0 ms, whole frame reported | Metal kit, `results-landing1` | manual | **pass:** 2.16 / 2.13 ms (whole frame 27.9 / 25.9; today 53.9 / 21.7 ms, 79.6 / 50.7) |
| M2 5300M at Auto, near/probes/mid vs today | Metal kit, `results-landing1` | manual | **measured:** 0.51 / 0.54 / 0.54 ms vs 9.90 / 4.14 / 4.18 ms; new-arm clocks low, directional |
| M3 Windows DX12 FXC compile, both variants | owner, Windows | manual | **pass** (GTX 1660 Super, Windows 11, at 25d738377: `dx12_fxc_compiles_every_full_init_pipeline -- --ignored` 1 passed on DX12/FXC, both fluid pipelines built, no FXC errors) |
| M4 GTX 1660 Vulkan no regression; reflecting-minus-free at e24 near 4K before/after | owner, Windows | manual | **pass** (GTX 1660 Super, Vulkan, 96ab6c9f4 → e00a42fc5, 64/64 runs valid): every view and mask faster, forward unchanged; reflecting − reflection-free at e24 near 4K 9.99 → 2.48 ms (GTX 1660 table below) |
| M5 parity at near, probes, mid, under; near-wall subset | Metal kit, `parity.py` | manual | **measured:** max 1/255 per channel at every view, near-wall included (mid: 41 075 px) |
| M6 owner look | owner, in engine | manual | **pass** (owner, Windows Vulkan + DX12 and Mac; approved landing). A sudden dimming of reflecting water while strafing beside a partly shadowed probe volume reproduces on `main` too, so it is pre-existing and not this brief's; it goes to the reflection term's owner (`reflection-probe-fluid-pass-performance`) |

## 5300M two-binary before/after
`results-landing2.{md,json}`: BEFORE 96ab6c9f4 and AFTER e00a42fc5, each its own release binary (`capture,dev-tools`, no hook), interleaved, 2 rounds, 24/24 runs valid, 4K.

| view | mask | fluid before → after ms | GPU frame before → after ms | forward Δ ms |
|---|---|---|---|---|
| near | reflection-free | 52.35 → 2.16 | 78.18 → 27.88 | −0.12 |
| probes | reflection-free | 21.28 → 2.14 | 49.91 → 25.91 | −0.79 |
| mid | reflection-free | 18.42 → 1.25 | 40.74 → 23.45 | −0.04 |
| near | reflecting | 64.15 → 12.33 | 92.61 → 41.23 | +0.35 |
| probes | reflecting | 35.57 → 14.82 | 65.08 → 40.95 | −0.23 |
| mid | reflecting | 25.06 → 6.52 | 55.65 → 32.44 | −2.51 |

Reflecting − reflection-free fluid on the 5300M stays large: near 11.80 → 10.16 ms, probes 14.29 → 12.69, mid 6.65 → 5.27. Unlike the 1660, this GPU's reflection term costs about the same with baked SH gone; it is the reflection brief's to remove. The probes and mid-reflecting frames drop 3–5 ms more than the fluid pass does; not attributed here. The same-binary override A/B (`results-landing1`) agrees within 3%.

## GTX 1660 Super (Vulkan)
Release `--features capture,dev-tools` (capture alone reports GPU timing `plain-build-unavailable`), `POSTRETRO_GPU_TIMING=1`, sync-proof SH, 120 warmup + 360 samples, ABBA over 2 rounds; GPU frame = sum of per-pass averages; spreads ≤ 0.10 ms. Measured by the Windows session.

| config | fluid before → after ms | forward before → after | GPU frame before → after |
|---|---|---|---|
| near 4K reflection-free | 12.542 → 0.831 | 7.293 → 7.325 | 20.810 → 9.119 |
| near 4K reflecting | 22.531 → 3.313 | 8.488 → 8.306 | 32.002 → 12.583 |
| probes 4K reflection-free | 6.577 → 0.792 | 6.162 → 6.158 | 14.393 → 8.649 |
| probes 4K reflecting | 11.068 → 4.574 | 6.920 → 6.934 | 19.626 → 13.179 |
| mid 4K reflection-free | 5.341 → 0.462 | 6.450 → 6.433 | 12.790 → 7.886 |
| mid 4K reflecting | 7.546 → 1.751 | 7.720 → 7.720 | 16.280 → 10.460 |
| near 1080p reflection-free | 3.170 → 0.255 | 1.928 → 1.928 | 5.393 → 2.464 |
| near 1080p reflecting | 6.129 → 0.888 | 2.197 → 2.180 | 8.618 → 3.354 |

Reflecting − reflection-free fluid: near 4K 9.99 → 2.48 ms, probes 4K 4.49 → 3.78, mid 4K 2.21 → 1.29, near 1080p 2.96 → 0.63. Removing baked SH from `fs_main` removes most of the near-4K read penalty; about 3.8 ms remains on the probes pool, which is what the reflection brief's split draw still has to win.

## Review loop
- Preflight: green (fmt, clippy, 11 691 tests, release check, crate graph; dev-tools clippy + 3 346 tests). `capture,observability,dev-tools` fails to compile on `main` already (missing `swim`/`vault` in observability initializers), unrelated.
- Panel 1 (2 tracers + adversarial, opus; hygiene, sonnet): 0 🔴; fixed: sliver parts dropped beyond ~105 m from the origin, i64 overflow on a huge origin with a tiny cell, overlapping halves on an on-plane vertex run, mismatched range/side predicates, unsaturated count, degenerate SH lattice still sampled (P20), tests not exercising the tolerance, comment drift and the measurement kit after the shader split.
- Panel 2 (adversarial on the fix round): 0 🔴; closed two test gaps (run rule at ring vertex 0, tolerance-boundary predicate), mutation-checked.
- Owner decision (2026-10-18): land with the P13 shallow-angle case in A3 as a known limit (the crack stays no wider than the compiler's open T-junction); closing every fluid T-junction at bake is drafted as `plans/drafts/fluid-surface-t-junction-closure`, which supersedes this brief's carve-out.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Grid packing in render-cpu: lattice type + validation, spacing/cap choice from per-face bounds, per-face plane cutting with edge-determined crossings, Steiner-free triangulation, warn once; shared-edge and cap tests | integrating executor | — | done: `fluid_surface::grid` (16 tests), `probe_indices_cover_every_grid_vertex_of_a_bound_face` |
| 2 | Renderer: lattice seam (streamed base / legacy present / none), `FluidSurfacePass::install` takes it, install log reports spacing | integrating executor | 1 | done: `FullRenderer::sh_sampler_lattice`; `the_fluid_grid_takes_the_lattice_the_sh_sampler_reads` |
| 3 | Shader: split `fluid_surface.wgsl`; baked terms to `vs_main` (viewer-side normal, masks, scales), interpolated to both fragment entries; invert per-fragment pins, add `global_uses` test | integrating executor | — | done: `only_the_vertex_entry_samples_the_sh_volume`, `fluid_baked_light_is_per_vertex_and_runtime_lights_per_fragment`; 60 fluid pins + budget tests pass unedited; 14 adapter frame tests pass |
| 4 | 5300M measurement + parity on the Metal kit; settle spacing and cap | integrating executor | 2, 3 | done: Measurement section |
| 5 | Adapter frame tests (A7) over an in-memory SH section; lattice selection and level-change adapter tests (A4, A6) | worker | 2, 3 | done: `fluid_baked_frame_test` (4 adapter tests, run on this Mac), `a_streamed_level_cuts_its_fluid_on_the_manifest_base_lattice` |
| 6 | Install-order source tests (A6), no-fluid/no-lattice log tests (A5) | integrating executor | 2 | done: `fluid_surfaces_install_after_level_geometry_on_the_level_and_capture_paths`; A5 log pins in `fluid_surface::grid` |
