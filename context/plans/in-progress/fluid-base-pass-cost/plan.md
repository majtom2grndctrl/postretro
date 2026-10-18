# fluid-base-pass-cost — plan of record

mode: compact
status: active
read at: 96ab6c9f4

No crate source changed between the brief's `read at` (2f1f96297) and this plan's base; the Decision reads stand.

## Corrections
- Path: "`ShVolumeResources` keeps neither origin nor cell size on the CPU … both values are plumbed to the packer from level data" → `ShVolumeResources` keeps `grid_origin`/`cell_size` CPU mirrors (dead-code-allowed today) beside `present`, and `ShResidencyState` keeps the manifest base (`base_metadata`). Planning around it by selecting the lattice inside the renderer at fluid install, through one seam beside `FullRenderer::sh_bind_group`: the streamed base when streaming, else the legacy section's when `present`, else none. The binary passes nothing new; a device-rejected or absent volume (`present == false`) yields no grid; the placeholder lattice is never read.
- Decision "Bounded … takes the finest spacing that fits" → clarification, same meaning: the cap counts an upper bound computed per face from its extent and the spacing alone, before any face is cut (P18), exact for a face no plane crosses (today's count). A gridded face therefore triangulates without added vertices (convex ear clipping that keeps every collinear vertex a corner) rather than today's centroid fan, so the packed mesh never exceeds the counted bound. A face no plane crosses keeps today's triangulation verbatim.
- Path "Large files: `fluid_surface.wgsl` is past 800 lines; split before extending" → split by responsibility into `fluid_surface.wgsl` (interface, shading, entries), `fluid_motion.wgsl` (warp, drift, wave) and `fluid_reflection.wgsl` (reflection term), concatenated by `SHADER_SOURCE`; source-pinning tests read the fluid texts through one helper.

## Delegated answers
- Grid spacing and per-level vertex cap — provisional: half a probe cell per axis, coarsening by doubling (planes nest), cap 2^18 vertices per level; final values and their 5300M cost and parity recorded under Results.
- Parity bound — reported from measurement, no threshold set (brief).

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 fragment entries sample no SH atlas / probe depth / SH grid; `vs_main` samples each; runtime lights stay in both fragment entries; reflecting entry keeps the shared sampler; per-fragment pins inverted | naga `global_uses` test over all three entries + inverted source pins (`fluid_surface_tests.rs`) | achievable as stated |
| A2 layout keeps groups, entries, visibilities and vertex stride | existing layout/stride/budget tests unedited; BGL builders untouched in the diff | achievable as stated |
| A3 grid packing (areas, planes, UV, shared edges P9–P12, T-junctions P13, originals P10, contiguity + streams P15, uncut face, cap P5–P7 P17 P18) | render-cpu `fluid_surface::grid` tests | achievable as stated |
| A4 lattice the SH sampler uses (section's or manifest base, never placeholder, P3) | GPU-free selection test + adapter test on legacy and streamed installs | achievable as stated |
| A5 no usable SH volume → packs as today, ambient only, no error (P4, P20); no fluid → no grid, no log (P14) | render-cpu lattice-validation + no-lattice pack tests; adapter frame test (ambient only); log-capture test | achievable as stated |
| A6 fluid install after geometry install on level and capture paths; level change repacks on its own lattice / cap warning (P1, P2, P19) | source-order tests (lifecycle, capture); adapter level-change test | achievable as stated |
| A7 GPU frame tests: variant parity; uniform field vs CPU formula within 1/255 across scales and masks incl. fog layer; viewer side above/below/crossing on both variants (P16) | `fluid_surface_frame_test.rs` adapter tests over an in-memory RGBA16F SH section | achievable as stated |
| A8 budget pins pass unedited, vertex stage included | existing budget tests, unedited | achievable as stated |
| A9 diff touches no level-compiler, level-format or wire code | `git diff --stat main` over those crates at preflight | achievable as stated |
| M1 5300M near + probes 4K reflection-free pass ≤ 3.0 ms, whole frame reported | executor, Metal kit (this machine is the 5300M) | manual |
| M2 5300M at Auto (1536×960), near/probes/mid vs today | executor, Metal kit | manual |
| M3 Windows DX12 FXC compile, both variants | owner, Windows | manual |
| M4 GTX 1660 Vulkan no regression; reflecting-minus-free at e24 near 4K before/after | owner, Windows | manual |
| M5 parity vs per-fragment at near, probes, mid, under e24; and within one spacing of a pool wall | executor, Metal kit captures | manual |
| M6 owner look: grid lines, wall leaks, wall match, dynamic light smooth | owner, in engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Grid packing in render-cpu: lattice type + validation, spacing/cap choice from per-face bounds, per-face plane cutting with edge-determined crossings, Steiner-free triangulation, warn once; shared-edge and cap tests | integrating executor | — | done: `fluid_surface::grid` (16 tests), `probe_indices_cover_every_grid_vertex_of_a_bound_face` |
| 2 | Renderer: lattice seam (streamed base / legacy present / none), `FluidSurfacePass::install` takes it, install log reports spacing | integrating executor | 1 | done: `FullRenderer::sh_sampler_lattice`; `the_fluid_grid_takes_the_lattice_the_sh_sampler_reads` |
| 3 | Shader: split `fluid_surface.wgsl`; baked terms to `vs_main` (viewer-side normal, masks, scales), interpolated to both fragment entries; invert per-fragment pins, add `global_uses` test | integrating executor | — | done: `only_the_vertex_entry_samples_the_sh_volume`, `fluid_baked_light_is_per_vertex_and_runtime_lights_per_fragment`; 60 fluid pins + budget tests pass unedited; 14 adapter frame tests pass |
| 4 | 5300M measurement + parity on the Metal kit; settle spacing and cap | integrating executor | 2, 3 | |
| 5 | Adapter frame tests (A7) over an in-memory SH section; lattice selection and level-change adapter tests (A4, A6) | worker | 2, 3 | |
| 6 | Install-order source tests (A6), no-fluid/no-lattice log tests (A5) | integrating executor | 2 | |
