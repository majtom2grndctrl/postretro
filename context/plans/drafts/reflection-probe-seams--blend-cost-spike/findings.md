# reflection-probe-seams--blend-cost-spike — findings

Spike code: branch `claude/blend-cost-spike` (`e59e491fe` modes, `084b111c7` honesty gates, `ab271b90f` fixture, `b6a179b9c` canal as two abutting boxes), on `feature/reflection-probes` `3c57450ee`. GTX 1660 Super measured 2026-10-16; Radeon Pro 5300M measured 2026-10-17 by the owner's Mac session (`measurements/reflection-blend-cost-spike/results.md`, `noprobe-arm-720p.md`).

## Correction to the brief
The brief was drafted at 949a79411. Reflection-probes task 11 later added a distance search (up to 3 sphere steps, 2 refinements) and a capture-shadow fade (2–4 taps) to every probe sample, so each extra blend sample repeats both. The spike has two shortcuts:
- **Band distance**: it measures to the neighbour probe's box rectangle, not to its water region, because there is no nearby-probe bake.
- **Neighbours**: they are the nearest same-height probes, chosen at install.

That is why the fixture's canal uses two authored boxes that abut at the doorway. A derived doorway piece gets a box spanning both rooms, which would put the whole canal inside the band.

## Honesty gates
All pass.
- **render-cpu, 457 tests:**
  - With each probe as its own neighbour, blend and precomputed equal single within 1e-5.
  - The precomputed ratio is within 1e-4 of the per-fragment ratio over more than 10k cases.
  - Stochastic pick frequencies are within 2% over 24 000 cells and stable per cell.
  - Coverage gives three nonzero weights.
- **renderer, 999 tests:** the adapter test ran on the GPU, not skipped. Modes 1–3 equal mode 0 at 0/255 when each probe is its own neighbour.
- **capture, 73 tests.** DX12 FXC passes, and so does the reflection-free reachability test.
- **1660:** all 38 timed runs exited 0 at P0.
- **5300M:** all 28 traced runs were valid. Fluid time comes from complete frames framed by the Candidate Cull Pass, and the label-independent GPU frame time agrees between complete and incomplete frames where both exist.

## Setup
| | GTX 1660 Super | Radeon Pro 5300M |
|---|---|---|
| Backend, timing | Vulkan, `POSTRETRO_GPU_TIMING=1` | Metal, Metal System Trace |
| Frames | 240 warmup + 720, steady windows 3–6 | 240 warmup + 240, complete steady frames |
| Resolution, runs | 1080p ×2 interleaved; 4K ×1 | 4K ×2 interleaved |

Both machines: release `capture,dev-tools`, `POSTRETRO_SH_STREAMING=sync-proof`, fresh spike PRLs, and fluid drawn at opacity 0.6, reflectivity 1, pixel style.

Poses and arms:
- **worst:** fixture room A. Three probes on one plane, each storing K = 4 curve-animated lights. Camera straight down, coverage forced, so every fluid fragment takes three samples.
- **typ:** fixture canal, low eye looking through the doorway, with a real 1 m band.
- **e24:** the e24 east-wing pool straight down, coverage forced. e24 has no animated lights.
- **blendw0:** the blend code is present but the band is 0, so one sample everywhere. This is what a third pipeline variant would save.

## Fluid pass, ms, against single
| Pose | GPU | single | blend | precomputed | stochastic | blendw0 |
|---|---|---|---|---|---|---|
| worst | 1660 1080p | 3.044 | 1.78× | 1.04× | 1.06× | |
| worst | 1660 4K | 12.16 | 1.78× | 1.03× | 1.06× | |
| worst | 5300M 4K | 38.91 | 1.70× | 1.14× | 1.04× | |
| typ | 1660 1080p | 0.289 | +0.020 | **−0.060 (0.79×)** | +0.013 | +0.014 |
| typ | 5300M 4K | 3.625 | +0.113 (1.03×) | **−0.620 (0.83×)** | +0.096 | +0.058 |
| e24 | 1660 1080p | 5.375 | 1.09× | 1.06× | 1.02× | 1.016× |
| e24 | 1660 4K | 22.01 | 1.08× | 1.06× | 1.01× | |
| e24 | 5300M 4K | 66.45 | 1.12× | 1.09× | 0.98× | 1.011× |

Run-to-run spread is within 0.06 ms on the 1660 at 1080p. On the 5300M it is within 0.18 ms, except e24-precomputed at 0.52 ms. Mode 0 equals the parent's final binary within spread on the 1660, so the debug lever itself costs nothing. The precompute costs 1–4 µs of CPU per frame and uploads 80 B per probe.

## Reading
- **Per-fragment live lighting is the biggest share of a sample's cost when probes store curve-animated lights.** Precomputing it per probe per frame cuts a forced three-sample blend from +78% to +3–4% on the 1660, and from +70% to +14% on the 5300M.
- **The precompute pays for itself.** At the typical pose, precomputed (blend included) is 21% cheaper than today's single sample on the 1660 and 17% cheaper on the 5300M. Today's reflection term gains that on its own.
- **The extra searches weigh more on the 5300M.** With lighting precomputed, three samples at full forced coverage still cost +14% (worst) and +9% (e24) there, against +3% and +6% on the 1660. Real bands are strips about 1 m wide at probe boundaries. The typical pose, the only realistic one, shows the blend itself adding about 3% on both GPUs.
- **Stochastic is the cheapest blend on the 5300M**: +4% at worst, −2% at e24, where it reads below single, unexplained. It trades a crossfade for a per-cell dither. Precompute plus stochastic was not measured; it should be cheaper still.
- **The blend code's mere presence costs little:** 1.1–1.6% on both GPUs (0.06 ms typical on the 5300M at 4K). That doesn't warrant a third pipeline variant.
- **The hypotheses hold.** Blend stayed under 2× single at the worst case on both GPUs (1.78×, 1.70×), and precomputed is far under. The typical delta under 0.1 ms holds on the 1660 at 1080p (+0.02). On the 5300M at 4K it is +0.11, about +0.03 at 1080p.
- **Fragment-bound.** The 1660's 4K ratios match its 1080p ratios.

## Side finding: reflection-free fluid is not free on the 5300M
At 720p on the e24 pool pose, the reflection-probes binary costs +0.30 ms (+5%) of fluid time over main even on main's probe-free PRL, with every lighting term on. That equals the reflection-free arm's cost and is unaffected by probe data or the specular mask (`noprobe-arm-720p.md`). On the 1660 the reflection-free variant equals main. So the two-pipeline fix restores main on NVIDIA but not on AMD/Metal, and reflection-probes owns the follow-up. The reflecting term costs the 5300M +21% of fluid time at that pose, against +72% on the 1660, so the 1660 overstates the reflection term's relative cost.

## Limits
- **No visual read yet.** Straight-down poses give Fresnel ≈ 0.02, so the images cannot judge the crossfade or the dither. The manual-visual read is the owner's in-engine look at an oblique view.
- **The typical pose is narrow.** Its canal covers few pixels, so its absolute deltas are small by construction.
- **Lighting extremes.** e24 has no animated lights, and the worst fixture has K = 4 curve lights on every probe. Real maps sit between.
- **Precompute scope.** The spike's precompute is CPU-side over every installed probe, about 0.6 µs per probe. At the 256 cap that is about 150 µs per frame unless it is limited to probes of drawn volumes. That matters on the Mac, which is CPU-bound on the hallway map.
- **Distance stand-in.** Box-rectangle distance stands in for region distance. The cost is comparable, not identical.

## Recommendation
1. **Per-probe ratio precompute, limited to probes of drawn volumes.** It is the clear win on both GPUs and makes today's single sample cheaper. It belongs in reflection-probes, or as the seams brief's first task.
2. **Deterministic three-sample blend, as written.** With the precompute, its realistic cost is a few percent, and its forced full-screen bound is +14% on the 5300M.
3. **Stochastic per-cell pick as the recorded fallback.** Use it only if a map's bands cover a large share of the screen on the 5300M, or if the owner's look prefers the dither for pixel style. Measure precompute plus stochastic then.
4. **No third pipeline variant.**
