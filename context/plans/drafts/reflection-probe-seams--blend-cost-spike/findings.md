# reflection-probe-seams--blend-cost-spike — findings (1660 half)

Spike code: branch `claude/blend-cost-spike` (`e59e491fe` modes, `084b111c7` honesty gates, `ab271b90f` fixture, `b6a179b9c` canal as two abutting boxes), on `feature/reflection-probes` `3c57450ee`. Measured 2026-10-16. The Mac (5300M) half is pending; kit in `measurements/reflection-blend-cost-spike/`.

## Correction to the brief
The brief was drafted at 949a79411. Since then, reflection-probes task 11 added a distance search (up to 3 sphere steps, 2 refinements) and a capture-shadow fade (2–4 taps) to every probe sample, so each extra blend sample repeats both. Spike-only: the band uses distance to the neighbour probe's box rectangle, not to its water region (no nearby-probe bake); neighbours come from the nearest same-height probes chosen at install. That is why the fixture's canal uses two authored boxes abutting at the doorway: a derived doorway piece gets a box spanning both rooms and would put the whole canal inside the band.

## Honesty gates
All pass. render-cpu 457 tests, among them: self-neighbour blend/precomputed equal single within 1e-5; precomputed ratio within 1e-4 of the per-fragment ratio over 10k+ cases; stochastic frequencies within 2% over 24 000 cells and stable per cell; coverage gives three nonzero weights. Renderer 999 tests, with the adapter test (modes 1–3 equal mode 0 at 0/255 under self-neighbours) run on the GPU, not skipped. Capture 73 tests. DX12 FXC passes; the reflection-free reachability test passes. All 38 timed runs exited 0 and logged the fluid pass in every window, at P0 (graphics median 1890–1950 MHz in steady windows).

## Setup
GTX 1660 Super, Vulkan, release `capture,dev-tools`, `POSTRETRO_SH_STREAMING=sync-proof`, `POSTRETRO_GPU_TIMING=1`, 240 warmup + 720 samples, steady = mean of windows 3–6. 1080p: two runs per arm, interleaved. 4K: one run per arm. Fresh `--no-cache --release` PRLs from the spike branch. Capture draws fluid at opacity 0.6, reflectivity 1, pixel style.
- **worst**: fixture room A, three probes on one plane, each storing K = 4 curve-animated lights, camera straight down, coverage forced (three samples on every fluid fragment).
- **typ**: fixture canal, low eye looking through the doorway, real 1 m band.
- **e24**: e24 east-wing pool straight down (the parent's pose), coverage forced; e24 has no animated lights.
- **blendw0**: blend code present but band 0 (one sample everywhere): what a third pipeline variant would save.
- **base**: the parent's final reflecting binary on the same PRL.

## Fluid pass, ms (steady; 1080p mean of two runs)
| Pose | single | blend | precomputed | stochastic | blendw0 | base |
|---|---|---|---|---|---|---|
| worst | 3.044 | 5.417 (1.78×) | 3.166 (1.04×) | 3.223 (1.06×) | | |
| typ | 0.289 | 0.309 (+0.020) | 0.229 (−0.060) | 0.302 (+0.013) | 0.303 (+0.014) | |
| e24 | 5.375 | 5.861 (+0.486) | 5.722 (+0.347) | 5.481 (+0.106) | 5.463 (+0.088) | 5.356 |
| worst, 4K | 12.163 | 21.651 (1.78×) | 12.552 (1.03×) | 12.885 (1.06×) | | |
| e24, 4K | 22.011 | 23.762 (1.08×) | 23.406 (1.06×) | 22.273 (1.01×) | | |

Run-to-run spread is within 0.06 ms at 1080p. Mode 0 equals the parent binary within that spread, so the lever itself costs nothing. Precompute CPU cost: 1–4 µs per frame, 80 B uploaded per probe (5 probes 400 B, 12 probes 960 B).

## Reading
- **Per-fragment live lighting dominates a sample's cost** when probes store curve-animated lights. With it precomputed per probe per frame, three samples cost 3–4% over today's single sample at forced full coverage, against 78% without. At the typical pose, precomputed is 21% cheaper than today's single sample: the precompute pays for the parent's reflection term on its own.
- **Without animated lights** (e24), the extra samples' search and fade cost 6.5% (precomputed) or 9% (blend) at forced full coverage. Real bands are strips about 1 m wide at probe boundaries, so this is an upper bound.
- **Code presence** (blendw0) costs +0.014 ms at the typical pose and +0.088 ms (1.6%) on e24: the NVIDIA codegen sensitivity again, small.
- **Stochastic** is the cheapest at e24 (one sample) but costs more than precomputed where lights animate, and it trades a crossfade for a dither.
- **Hypothesis**: blend under 2× single at the worst case holds (1.78×), but only barely; precomputed is far under. The typical delta under 0.1 ms holds (+0.02 ms).
- **Linear in fragments**: 4K ratios match 1080p, so the pass is fragment-bound at both.

## Limits
- Straight-down poses give Fresnel ≈ 0.02, so the images cannot judge the crossfade or the dither. The manual-visual read (blend band, stochastic dither in both styles) is the owner's in-engine look at an oblique view and is not done.
- The typical pose's canal covers few pixels, so its absolute deltas are small by construction.
- e24 has no animated lights; the worst fixture has K = 4 curve lights on every probe. Real maps sit between.
- The precompute is CPU-side over every installed probe (≈0.6 µs per probe); at the 256 cap that is ≈150 µs per frame unless limited to probes of drawn volumes. That matters on the Mac, which is CPU-bound on the hallway map.
- Box-rectangle distance stands in for region distance; cost is comparable, not identical.
- The Mac half is not run.

## Recommendation (pending the Mac half)
Adopt the **per-probe ratio precompute**, limited to probes of drawn volumes, as the cost lever. It makes the parent's single sample cheaper too, so it belongs in reflection-probes or as the seams brief's first task. Then **blend as written**: three deterministic samples, no dither. No third pipeline variant (≤ 0.09 ms here); revisit if the Mac shows a larger code-presence cost. Keep stochastic as the fallback only if the 5300M shows the precomputed blend over budget.
