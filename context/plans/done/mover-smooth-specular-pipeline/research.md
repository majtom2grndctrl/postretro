# mover-smooth-specular-pipeline — research

Evidence from the spike on local branch `spike/specular-mover-cost` (tip 35ce20d10), write-up at `measurements/specular-mover-cost/README.md` there. Radeon Pro 5300M, 4K, Surface Depth Off, clock-pinned Metal System Trace, 3 interleaved rounds × 240 frames per arm, 77 valid runs.

## Mover pass medians (ms)

| Arm | Setpiece "wall" | Dev-map "ramp" | VGPRs |
|---|---|---|---|
| Pre-feature build (1c7fc9e3a) | 21.62 | 20.66 | 126 |
| Old shader in today's tree (V0) | 21.63 | 20.69 | 126 |
| Today, smooth | 24.07 | 23.25 | 128 |
| Today, pixel | 25.76 | 24.28 | 128 |
| Smooth only, two sums (V1) | 35.20 | 33.49 | 216 |
| One sum, both styles (V3), smooth | 24.02 | 23.22 | 128 |
| **Smooth only, one sum (V5s)** | **21.26** | **20.23** | **120** |
| Pixel only, one sum, directional skip (V5p) | 25.73 | 24.19 | 128 |

V0 shows the regression is the mover shader alone; engine, snippet and data changes cost nothing.

## Cliff bisection

From the old shader: adding albedo-after-loop (121 VGPRs) or the per-texel exponent decode (126) stays under the cliff. Adding the tinted-specular second accumulator crosses it (216), whether split by branch or by per-light select, and wherever albedo is read. Today's two-style shader escapes at 128. One specular sum brings smooth-only to 120. The rule is observed on this compiler, not guaranteed; any production shape is re-screened.

## V5s

Smooth-only; the pixel path, its texel-point influence test and its Jacobian and height load are gone. The specular point is the Surface Depth hit with the fragment's view. Diffuse sums in the loop; specular sums apart; after the loop `albedo × lit + emissive + select(albedo, tint(albedo, t), tinted) × spec`. Albedo is read after the loop. It runs 2.81 / 3.02 ms faster than today's smooth and 0.36 / 0.43 ms faster than the pre-feature build. Its win comes from instruction count and scheduling, not occupancy: 120 and 128 sit in one tier (wave32 next tier needs ≤112).

Output at 1920×1080: today's smooth against the pre-feature build differs on 0 px. V5s against today's smooth differs on 2 px (setpiece) and 8 px (ramp), each by at most 1/255. An exact-match smooth-only shape needs per-light interleaving of untinted specular into the lit sum, which needs the second accumulator, which crosses the cliff.

## Albedo placement

Albedo after the loop costs 1.07 / 1.23 ms in the old shader's shape (B1). V5s keeps it after the loop and still beats the pre-feature build. V5s with albedo before the loop was not screened.

## Pixel

Every pixel ablation lands within −0.14 to 0.00 ms of today's pixel: pixel-only (V1p), one sum (V3), directional re-evaluation skipped (V4), all three (V5p). Pixel's +4.1 / +3.6 ms over the pre-feature build is the texel-point evaluation itself.

## Kit

`measurements/specular-mover-cost/` on the spike branch: `run.py` (arms as `<pose>:<variant>:<style>`, xctrace capture, watchdog, ioreg clock and temperature polling, lock, screen-saver and foreign-process checks, opt-in caffeinate), `gpu_time.py` (per-encoder reduction framed on the cull marker), `pooled.py` (pooled median and p95, per-run brackets), `apply_hook.py` (applies the Surface Depth Off env hook to a measurement worktree), `variants/` (every shader, `build_variant.sh`, `screen_only.sh`). The hook commit 9406a6655 is spike-only; the kit applies the hook in a measurement worktree, so the hook never needs to merge. The pre-feature binary is rebuilt from 1c7fc9e3a with the hook applied.

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| R1 | A style change lands on a frame the mirror draws movers | Group-0 upload snapshots the style, then the main-view mover record, then the planar mover record | Both records bind the snapshot style's pipeline |
| R2 | Style committed, then a full-renderer rebuild (suspend and resume), then a level install | The rebuild builds both pipelines and resets the snapshot to pixel; the first frame's group-0 upload re-snapshots before the mover record | The committed style's pipeline draws on frame one |
