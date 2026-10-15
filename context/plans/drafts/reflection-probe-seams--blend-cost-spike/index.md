# reflection-probe-seams--blend-cost-spike

Brief · compact · spike (`context/lib/experimental_spikes.md`) · reads: `rendering_pipeline.md` §7.3a, §12 (GPU Pass Timing, Without timestamp support, Pin the GPU clock, Machine-state confounders) · `testing_guide.md` §Resource bounds · read at 7ec1679b7 · builds on `reflection-probes` once its two fluid pipelines (task 8b) exist, when no timed run owns the Windows machine · settles the **blocks build** question in `reflection-probe-seams`

## Problem
Owner-requested experiment. `reflection-probe-seams` blends up to three probes per fragment inside a band at probe boundaries. Each sample repeats the parent's per-fragment live-lighting evaluation, and on e24 the reflecting shader's mere presence already cost 1.29 ms from NVIDIA codegen, so instruction counts cannot price the blend. The seams brief cannot be finalized until it is known whether three samples are affordable as written, or which lever applies: a per-probe ratio precompute, a stochastic per-cell probe pick, or a third pipeline variant confining cost to band draws. When this is done, a findings note gives the fluid-pass cost of each candidate on the GTX 1660 Super and the Radeon Pro 5300M at a worst-case and a typical pose, a visual read of each, and a recommendation the owner can write into the seams brief.

## Decisions
- **Measure the real code shape, fed synthetic neighbours.** The blend is the seams brief's region-distance blend, as the shader would run it: up to three samples, each with its own box projection, tile tap and live-lighting ratio, mixed by normalised weights before Fresnel. Nearby-probe data comes from a debug lever, not the compiler bake, which the spike does not build. Codegen sensitivity is the risk, so a stand-in loop of extra taps would measure the wrong shader.
- **Candidates**, chosen at runtime by a debug-tool selector (`experimental_spikes.md` §Tuning levers), never a player setting:
  - single: today's reflecting variant, the baseline;
  - blend: three samples with per-fragment ratios;
  - precomputed: three samples whose ratios come from a per-probe, per-lobe value computed once per frame on the CPU and uploaded, so the fragment does one tap and a lobe blend per probe;
  - stochastic: one sample per water texel cell, the probe drawn by its weight from a stable hash of the cell.
  The third-pipeline lever is not a shader; its value is the band-draw fraction at the typical pose times the blend's per-fragment cost, computed from the same runs.
- **Worst case is forced.** A coverage lever puts every reflecting fragment of a drawn face inside a three-probe band, reusing the level's other probes as synthetic neighbours, so the worst-case pose measures three samples on every fluid fragment. The typical pose uses the band a real doorway crossing would give.
- **Real share tables.** The worst-case fixture binds every probe to K animated lights with curves, the parent's costliest live-lighting case, so a ratio cost of zero cannot flatter the blend.
- **Fixtures.** A new dev map, `content/dev/maps/reflection-blend-spike.map`: one view-filling pool of three abutting brushes of one fluid entity, each under its own authored `reflection_volume` box so each binds its own probe, each covered by K curve-animated lights, plus an unsplit canal through a wall for the typical pose. And e24's east-wing pool at the parent's measured pose, the codegen-sensitive case. Fixed poses in a capture scene file, so both machines render the same frames.
- **Machines and method.** Windows: GTX 1660 Super, Vulkan, release `capture,dev-tools`, `POSTRETRO_GPU_TIMING=1`, offscreen 1920×1080 and 3840×2160 (fragment-bound check), the parent's task 9b setup (240 warmup, 720 samples, one process at a time, graphics clock recorded). Mac: the owner's MacBook Pro (Radeon Pro 5300M), built on the Mac, Metal System Trace per `rendering_pipeline.md` §12, at 4K to hold the GPU clock, `ioreg` clock and idle VRAM polled during capture; the owner runs it, and the plan of record carries the exact commands.
- **Spike code stays on its branch.** The findings note and the fixture map land; the debug selector and synthetic-neighbour path are not merged. The seams build reuses the blend body if the findings keep it.
- **Non-goals.** The nearby-probe bake, cross-entity grouping, continuity and pixel-style contracts: the seams brief owns them. A per-probe precompute that is production-ready (compute pass, dirty tracking): the spike proves its cost with the simplest CPU upload.

## Acceptance
### Honesty gates (automated)
- [ ] With all three synthetic neighbours set to the face's own probe, blend and precomputed output equal single within a pinned epsilon (the weights are normalised and the path is wired).
- [ ] Precomputed ratio equals the per-fragment ratio for the same probe and direction within quantization, against the render-cpu ratio reference.
- [ ] Stochastic: every fragment of one water texel cell samples one probe; over many cells of one weight triple, each probe's pick frequency matches its weight within a pinned tolerance; a cell's pick does not change across frames when nothing moves.
- [ ] The coverage lever makes every reflecting fragment of a drawn face take three samples in blend mode (CPU reference of the shader's weight path); off, the shader takes one sample wherever no synthetic neighbour lies within the band.
- [ ] Every timed run's log shows the fluid pass sampled in every readback window, the actual surface and scene extents, and on Windows the graphics clock; a run missing any is rerun, not reported.
- [ ] DX12 FXC compiles the spike's fluid shader (ignored-gated, Windows); the Mac build boots on Metal and draws the fixture.
### Measured findings (measure and report)
- [ ] Fluid-pass ms and frame ms for single, blend, precomputed and stochastic at the worst-case pose, the typical pose and e24's east-wing pool, on the 1660 at 1080p and 4K and on the 5300M at 4K, steady-state windows only, rerun once to show spread.
- [ ] At the typical pose, the band-draw fraction and the resulting third-variant estimate.
- [ ] The precomputed candidate's CPU cost per frame and upload bytes at the fixture's probe count.
- [ ] Manual-visual read at play speed on the fixture: the blend's crossfade, and whether the stochastic dither reads as part of pixel style or as noise, in both reflection styles.
- [ ] Findings note in this folder: each figure against the hypothesis below, the visual read, and a recommendation for the seams brief: blend as written, which lever, or a combination.

Hypothesis, not a gate: blend costs under 2× single at the worst-case pose on both machines, and the typical pose's delta is under 0.1 ms.

## Path
- **Shader.** `fluid_reflection_sample` is the per-probe body; the blend loops it over up to three probes with weights from the seams brief's falloff. Synthetic neighbours ride a spike vertex buffer beside location 4, or a debug uniform when the coverage lever is on; either way the reflecting variant's code shape is what gets timed. Keep the single variant untouched so the baseline is task 8b's.
- **Precompute.** `render_cpu::sh_compose::animated_light_scale` is the CPU reference for live values; the lobe blend is `reflection_probe_lobe_weights`. Upload into spare table space or a debug uniform; the spike may exceed the parent's binding counts only inside the debug path, recorded in the findings.
- **Stochastic hash.** Key on the pixel-style cell's integer coordinates and the face, never on screen position or frame, so it is stable under camera motion.
- **Process.** Check `tasklist` for `postretro`, `prl-build` and `cargo` before any build or run, stop only PIDs this work launched, close the engine after each run, and delete Metal traces and their `$TMPDIR` ktrace files once exported.
- **First slice.** Blend mode at the worst-case pose on the 1660 only: if it is already within the hypothesis there, the Mac run decides alone whether a lever is needed.

## Open questions
- K, the band width and the falloff used — **delegated**: use the parent's K and the seams brief's pinned W and falloff if set, else a stated provisional value, and record them in the findings
