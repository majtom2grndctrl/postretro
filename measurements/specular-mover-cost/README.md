# Specular mover cost spike

> **In the repo.** `run.py`, `gpu_time.py`, `pooled.py` and `apply_hook.py` are the method; point `SPIKE_BINS` at binaries built from any commit with `apply_hook.py` applied in a measurement worktree. The hook never lands in engine source. `variants/` and its two scripts are the spike's record: each variant shader runs only at its spike commit (`spike/specular-mover-cost`), because the mover now builds one beauty pipeline per style (`fs_pixel`, `fs_smooth`), and the scripts carry that session's absolute paths. The production shape came from the recommendation below through the `mover-smooth-specular-pipeline` brief.

Why the mover pass costs more after specular-texel-sharpness (PR #690, f8cb485b7) than before it (1c7fc9e3a), at 4K with Surface Depth Off on the Radeon Pro 5300M. Task 9 measured +11–12% under `smooth`, whose output is byte-identical to the old build, and +16–19% under `pixel`. The register count barely moved (126 → 128 VGPRs).

## Answer

- **The whole regression is in the mover shader.** The old mover shader dropped into the HEAD tree (V0) times like the old build: +0.01 ms on the setpiece, +0.03 ms on the ramp. Engine, snippet and data changes cost nothing.
- **Smooth's extra time is the pixel path being compiled into the shader**, though cutting it out works only in one code shape. In the shipped two-sum shape, deleting the pixel path pushes AMD's compiler to 216 VGPRs, and the pass gets 45% slower. With one specular sum and the tint applied after the loop, a smooth-only shader (V5s) screens at 120 VGPRs. It runs 2.81 ms (setpiece) and 3.02 ms (ramp) faster than HEAD smooth, and 0.36 to 0.43 ms faster than the old build.
- **Albedo read after the loop** costs about 1.1–1.2 ms in the old shader's shape (B1). V5s still reads albedo after the loop and beats the old build anyway, so this cost does not need fixing separately.
- **Pixel's extra time is its own work.** It is the texel-point light evaluation itself. Compiling out the smooth branch, the branchless tint, the directional skip and all three combined each move pixel by 0.00 to −0.14 ms.

## Method

- **Builds.** Each variant replaces only `crates/renderer/src/shaders/kinematic_brush.wgsl` and is one commit on `spike/specular-mover-cost` (table below). Builds ran in this worktree's own `target/` with `CARGO_INCREMENTAL=0 cargo build --release -p postretro --features capture`. Every tree also carries the env-gated Surface Depth Off capture hook (9406a6655, `apply_hook.py`), which exists on this spike branch only and must never be merged. The old build is the Task 9 binary for 1c7fc9e3a, built with the same hook. HEAD was rebuilt from the spike base and matches Task 9's HEAD binary within 0.04 ms (below).
- **Registers.** `tools/metal_register_screen/screen_mover.sh` was run at each variant's commit. `variants/screen_only.sh` screened candidates without building them.
- **Maps.** Both maps were compiled in this worktree with its own `prl-build`. `specular-texel-sharpness.prl` is byte-identical to the main checkout's. `mover-setpiece.map` was compiled to a scratch `mover-setpiece-spike.prl`, deleted afterwards. Every binary reads the same bytes.
- **Poses.** The primary pose is setpiece "wall": Station 1 at the closed wall, FOV 100, no forced promotion, shipped content. The second is dev-map "ramp": the ramp mover's front face, with both static lights' promotion forced to 1.0.
- **Runs.** `run.py` ran each arm as `<pose>:<variant>:<style>`, a clock-pinned 3840×2160 headless `--capture` under `xctrace record --template 'Metal System Trace'` with a watchdog.
  - Settings: `POSTRETRO_SH_STREAMING=sync-proof`, Surface Depth Off through the hook, and 120 warmup plus 240 sampled frames.
  - Each run was framed on the cull marker and reduced per encoder by `gpu_time.py`.
  - During each run the kit polled ioreg for clock and temperature, and checked for a screen lock, the screen saver and foreign processes. Each trace and ktrace was deleted once reduced.
  - The old build has no style, so its scene sets no `specular_style`.
- **Order.**
  - Pilot 1: one run per arm with abort-on-invalid. It covered all setpiece arms and the ramp arms up to V3 smooth.
  - Pilot 2: the arms added after the bisection.
  - Batch: rounds 2 (reversed) and 3 (forward) over all 25 arms, interleaved, with a 15 s idle gap before every run.
  - Each pilot counts as an arm's round 1. Ramp V1 and V2 ran in the pilot only, since their cliff was already clear. Ramp V4 was not run.
- **Validity.** All 77 runs are valid. Busy-clock medians ran 1134–1228 MHz and temperatures peaked at 81 °C, with no lock, screen saver or foreign process.
  - One kit rule changed: the dropped-rows cross-check (complete against incomplete frame time) now needs at least 5 incomplete frames.
  - Pilot run `ramp-v3-smooth-r1` had failed that check on a single incomplete frame that was missing its mover pass, with 239 complete frames. It was revalidated under the new rule.
- **Caffeinate.** The coordinator held `caffeinate -dimsu`. `run.py` starts its own only with `OWN_CAFFEINATE=1`.
- **Pooling.** `pooled.py` pools per-frame mover times across an arm's valid runs and reports the median and p95. Each delta is a difference of pooled medians. Brackets give the min..max over every pair of per-run medians.
- **Raw data.** In the session scratchpad: `spike-data/`, including runs, results and identity captures, `spike-bin/<variant>/` and `spike-regs/<variant>/`.

## Variants

| Variant | Commit | Change to the mover shader |
|---|---|---|
| head | 9406a6655 | none (f8cb485b7 plus the hook) |
| V1 | f8f708370 | smooth only. Pixel path deleted: texel-point influence test, `mover_texel_specular`, `mover_specular_point` (Jacobian and height load). The specular point is `SpecularPoint(depth.world_position, V)` |
| V1b | 16e99a1d1 | smooth only, as the brief worded it: `mover_specular_style_smooth()` returns `true` and the compiler folds the pixel path away. Screened, not timed |
| V1p | 6253385e6 | pixel only: smooth branch deleted, style hard-coded |
| V2 | d19e53a48 | V1 plus albedo sampled before the light loop |
| V3 | d9800fe19 | full shader with one specular sum, combined once as `albedo*lit + select(albedo, tint, tinted)*spec`, so no per-light tint branch remains |
| V4 | 04d712a88 | full shader; `mover_texel_specular` reuses the per-pixel light geometry for directional records |
| V0 | c1540a027 | the 1c7fc9e3a mover shader, unchanged, in the HEAD tree |
| B1 | 2aeb9deb0 | V0 plus albedo read after the loop |
| B2 | d18b3f44e | V0 plus HEAD's per-texel exponent decode from the RGBA slot |
| B3 | screen only (`variants/kinematic_brush.b3.wgsl`) | B2 plus HEAD's tint (two sums, per-light branch) with albedo after the loop |
| V1s | screen only (`variants/kinematic_brush.v1s.wgsl`) | V1 with per-light `select`s instead of the tint branch, which keeps the untinted summation order |
| V5s | d70077eec | V1 plus V3: smooth only, with one specular sum and the tint selected after the loop |
| V5p | 152cef926 | V1p plus V3 plus V4 |

V1 keeps `dpdx`/`dpdy` of world position, because `surface_depth_resolve` still takes them at Off.

## Registers (mover `fs_main`, Radeon Pro 5300M)

| Variant | SGPRs | VGPRs | Waves/SIMD, wave32 (granule 8 of 1024) | Waves/SIMD, wave64 (granule 4 of 512) |
|---|---|---|---|---|
| head | 104 | 128 | 8 | 4 |
| V0 (old shader) | 104 | 126 | 8 | 4 |
| B1 | 104 | 121 | 8 | 4 |
| B2 | 104 | 126 | 8 | 4 |
| V1p, V3, V4, V5p | 104 | 128 | 8 | 4 |
| V5s | 104 | 120 | 8 | 4 |
| V1, V1b, B3, V1s | 103 | 216 | 4 | 2 |
| V2 | 103 | 221 | 4 | 2 |

Only the ~216–221 group crosses a granule tier: it halves occupancy under either wave size. V5s's 120 stays in HEAD's tier; the next tier down needs 112 or fewer (wave32) or 100 or fewer (wave64). V5s's win therefore comes from instruction count and scheduling, not occupancy.

The bisection shows what triggers the cliff. A **smooth-only** shader that keeps **two light-loop accumulators** (lit, plus tinted specular) lands at 216 or above. That holds whether the tint split is a branch (V1, B3) or a select (V1s), and whatever the albedo placement. HEAD's full shader escapes the cliff at 128, and old's single accumulator sits at 126. One specular sum (V5s) brings smooth-only down to 120. AMD's Metal compiler chooses the allocation, so this is an observed rule for this shader, not a guarantee. Any production shape must be re-screened.

## Results: mover pass, 4K, Surface Depth Off

### Setpiece "wall" (primary)

| Arm | Runs | Median / p95 (ms) | Per-run medians (ms) | Δ vs old | Δ vs HEAD, same style |
|---|---|---|---|---|---|
| old | 3 | 21.623 / 21.956 | 21.604–21.665 | — | — |
| V0 (old shader in HEAD tree) | 3 | 21.631 / 22.002 | 21.601–21.664 | +0.008 (+0.0%) [−0.064..+0.060] | — |
| B1 (old + albedo after loop) | 3 | 22.691 / 23.010 | 22.675–22.701 | +1.068 (+4.9%) [+1.010..+1.097] | — |
| B2 (old + exponent decode) | 3 | 21.555 / 21.931 | 21.529–21.569 | −0.068 (−0.3%) [−0.136..−0.035] | — |
| head smooth | 3 | 24.067 / 24.213 | 24.059–24.077 | +2.444 (+11.3%) [+2.394..+2.473] | — |
| V1 smooth | 3 | 35.203 / 35.372 | 35.167–35.231 | +13.580 (+62.8%) | +11.136 (+46.3%) [+11.090..+11.172] |
| V2 smooth | 3 | 33.220 / 33.368 | 33.198–33.273 | +11.597 (+53.6%) | +9.153 (+38.0%) [+9.121..+9.214] |
| V3 smooth | 3 | 24.015 / 24.167 | 23.997–24.030 | +2.392 (+11.1%) | −0.052 (−0.2%) [−0.080..−0.029] |
| **V5s smooth** | 3 | **21.259** / 21.620 | 21.214–21.301 | **−0.364 (−1.7%)** [−0.451..−0.303] | **−2.808 (−11.7%)** [−2.863..−2.758] |
| head pixel | 3 | 25.756 / 26.015 | 25.738–25.776 | +4.133 (+19.1%) [+4.073..+4.172] | — |
| V1p pixel | 3 | 25.617 / 25.765 | 25.587–25.656 | +3.994 (+18.5%) | −0.139 (−0.5%) [−0.189..−0.082] |
| V3 pixel | 3 | 25.716 / 25.865 | 25.694–25.764 | +4.093 (+18.9%) | −0.040 (−0.2%) [−0.082..+0.026] |
| V4 pixel | 3 | 25.681 / 25.983 | 25.653–25.727 | +4.058 (+18.8%) | −0.075 (−0.3%) [−0.123..−0.011] |
| V5p pixel | 3 | 25.731 / 25.885 | 25.716–25.741 | +4.108 (+19.0%) | −0.025 (−0.1%) [−0.060..+0.003] |

### Dev map "ramp"

| Arm | Runs | Median / p95 (ms) | Per-run medians (ms) | Δ vs old | Δ vs HEAD, same style |
|---|---|---|---|---|---|
| old | 3 | 20.664 / 21.018 | 20.650–20.703 | — | — |
| V0 | 3 | 20.693 / 21.027 | 20.670–20.826 | +0.029 (+0.1%) [−0.033..+0.176] | — |
| B1 | 3 | 21.891 / 22.124 | 21.876–21.897 | +1.227 (+5.9%) [+1.173..+1.247] | — |
| B2 | 3 | 20.783 / 20.997 | 20.698–20.892 | +0.119 (+0.6%) [−0.005..+0.242] | — |
| head smooth | 3 | 23.247 / 23.444 | 23.222–23.323 | +2.583 (+12.5%) [+2.519..+2.673] | — |
| V1 smooth | 1 (pilot) | 33.487 / 33.630 | 33.487 | +12.823 (+62.1%) | +10.240 (+44.0%) |
| V2 smooth | 1 (pilot) | 31.578 / 31.695 | 31.578 | +10.914 (+52.8%) | +8.331 (+35.8%) |
| V3 smooth | 3 | 23.215 / 23.413 | 23.180–23.252 | +2.551 (+12.3%) | −0.032 (−0.1%) [−0.143..+0.030] |
| **V5s smooth** | 3 | **20.232** / 20.564 | 20.180–20.266 | **−0.432 (−2.1%)** [−0.523..−0.384] | **−3.015 (−13.0%)** [−3.143..−2.956] |
| head pixel | 3 | 24.278 / 24.424 | 24.248–24.303 | +3.614 (+17.5%) [+3.545..+3.653] | — |
| V1p pixel | 3 | 24.176 / 24.322 | 24.173–24.182 | +3.512 (+17.0%) | −0.102 (−0.4%) [−0.130..−0.066] |
| V3 pixel | 3 | 24.273 / 24.408 | 24.259–24.287 | +3.609 (+17.5%) | −0.005 (−0.0%) [−0.044..+0.039] |
| V5p pixel | 3 | 24.187 / 24.321 | 24.186–24.192 | +3.523 (+17.0%) | −0.091 (−0.4%) [−0.117..−0.056] |

Old and HEAD reproduce Task 9's setpiece pooled medians within 0.04 ms: 21.62 / 24.03 / 25.72 ms then, 21.62 / 24.07 / 25.76 ms now.

GPU frame time follows the mover. On the setpiece, V5s smooth is 54.79 ms against 55.88 for old and 59.09 for HEAD smooth. The forward pass's per-encoder time moves by up to about 1.7 ms between arms although its shader never changes. It runs lower when the mover is slow, which suggests the two passes overlap in the trace, so compare mover rows only. The full tables are in `pooled-all.md` and `pooled-all.json`.

## Output identity

Single 1920×1080 captures of both poses at Surface Depth Off were compared pixel by pixel, all channels:

| Pair | Setpiece | Ramp |
|---|---|---|
| old vs head smooth | 0 px differ | old differs on the ramp's `_p`/`_t` fixtures, as expected |
| head smooth vs V5s smooth | 2 of 2,073,600 px, max 1/255 | 8 px, max 1/255 |
| head smooth vs V3 smooth | 2 px, max 1/255 | 8 px, max 1/255 |
| head pixel vs V5p pixel | 5 px, max 1/255 | 9 px, max 1/255 |
| old vs V0 | 5,561 px, max 1/255 | — |

The one-sum shape (V3, V5s) moves the summation order, so smooth is no longer byte-identical to 1c7fc9e3a, only within 1/255 on a handful of pixels. V0 differs from old by 1/255 on 0.3% of the setpiece. The old shader's arithmetic composed against HEAD's snippets is therefore not bit-stable either. It is not a production candidate.

## Verdicts

| Theory | Verdict | What it explains |
|---|---|---|
| 1: smooth pays for the pixel path compiled in | **Confirmed, in the one-sum shape** | Removing the pixel path saves 2.76 ms on the setpiece and 2.98 ms on the ramp (V3 smooth → V5s). That covers all of smooth's +2.44 / +2.58 ms regression and goes 0.36 / 0.43 ms past the old build. In HEAD's two-sum shape the same removal (V1, V1b) costs +11.1 / +10.2 ms, from the 216-VGPR cliff. |
| 2: albedo after the loop exposes its fetch latency | **Partial** | +1.07 / +1.23 ms in the old shader's shape (B1). That is not additive with theory 1: V5s keeps albedo after the loop and is faster than the old build. V2 (albedo before the loop, smooth-only) went to 221 VGPRs and cannot isolate it. |
| V3: per-light tint branch | **Refuted as a cost** | −0.05 / −0.03 ms smooth and −0.04 / −0.01 ms pixel: noise. It is the enabler: one sum keeps a smooth-only shader under the cliff. |
| V4: second geometry evaluation for directional lights | **Refuted** | −0.075 ms on setpiece pixel. Neither pose promotes a directional light, so V4 changes codegen only; `mover_light_geometry` already returns early for directional records. |
| Pixel's own cost | **Intrinsic** | +4.13 / +3.61 ms over old. Every pixel ablation (V1p, V3, V4, V5p) recovers at most 0.14 ms. |
| Per-texel exponent decode (B2) | **No cost** | −0.07 / +0.12 ms |

## Recommended production shape (for a follow-up brief)

1. **Two mover pipeline variants, chosen by style.** Use a WGSL `override` constant (or two composed modules) for smooth and pixel.
   - Smooth variant: the V5s shape. No pixel path; one specular sum; diffuse summed apart; `albedo*lit + emissive + select(albedo, specular_tint(albedo, t), tinted)*spec` once after the loop; albedo still read after the loop. Expected: about −2.8 to −3.0 ms against HEAD smooth, and −0.4 ms against the pre-feature build.
   - Pixel variant: HEAD's pixel path. The one-sum and directional-skip changes are optional; they are worth at most 0.1 ms.
2. **Re-screen the real shape.** Naga's override pipeline should fold the constant before MSL, but the AMD allocation has been shown to flip from 120 to 216 on shape alone. Screen both variants under `tools/metal_register_screen` and re-time V5s's shape before landing.
3. **Leave forward as it is.** Forward is outside this spike: its pixel cost (+0.4–1.4 ms in Task 9) is separate, and smooth forward was flat.
4. **Acceptance rows the brief would touch:**
   - **Smooth byte-identity to 1c7fc9e3a.** The one-sum shape breaks exact identity on 2–8 px per 2 Mpx, by 1/255. Two identity-preserving smooth-only shapes were tried: the per-light branch (V1) and per-light selects (V1s). Both land at 216 VGPRs. The row needs an owner call: accept "within 1/255", or fund a search for an identity-preserving shape under the cliff.
   - **Live style change, no pipeline rebuild.** Build both mover pipelines at init, including the planar mirror's use. A style change then selects a pipeline per frame and rebuilds nothing. The row survives only if "no rebuild" admits a second prebuilt pipeline. The tests that pin the style word and the per-frame write would change to pin pipeline selection. Init compiles one more mover pipeline.
   - **Mover register cliff (§7.3).** Record the rule found here: a smooth-only mover with two light-loop accumulators lands at 216 or more.
5. **Optional extra.** Albedo before the loop on the smooth variant (V5s plus B1's reverse) is untested. Screen it first; it could recover more, or flip the allocation.

## Files

- `run.py`, `gpu_time.py`, `pooled.py`, `apply_hook.py`: the Task 9 kit, adapted for variant×style arms, `SPIKE_OUT`, `--start-round`, an opt-in caffeinate, and the 5-incomplete-frame rule.
- `variants/`: every variant shader, `build_variant.sh` (commit, screen, build, copy out) and `screen_only.sh`.
- `pooled-all.md`, `pooled-all.json`: the pooled tables.
