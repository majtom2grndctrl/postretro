# Glossy world reflections, 5300M (glossy-task10-flooronly)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.879 | 0.002 | 0.014 | 4.574 | 0.033 | 6.492 | 0.059 | 6.523 | 0.031 | 951 (792-1161, n=10); 927 (845-1049, n=9) |
| glossy-across-1440-after-m17f-high | 2/2 | 2.181 | 0.002 | 0.014 | 10.976 | 0.015 | 14.628 | 0.003 | 14.662 | 0.034 | 1129 (349-1168, n=19); 1103 (980-1161, n=18) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.816 | 0.002 | 0.013 | 24.459 | 0.004 | 31.612 | 0.047 | 31.644 | 0.032 | 1206 (1036-1220, n=36); 1200 (279-1219, n=37) |
| glossy-down-auto-after-m17f-high | 2/2 | 1.359 | 0.001 | 0.015 | 4.177 | 0.023 | 6.330 | 0.022 | 6.360 | 0.030 | 974 (850-1035, n=9); 930 (854-1033, n=9) |
| glossy-down-1440-after-m17f-high | 2/2 | 3.255 | 0.001 | 0.015 | 10.248 | 0.008 | 14.703 | 0.052 | 14.742 | 0.038 | 1126 (955-1169, n=18); 1129 (946-1211, n=18) |
| glossy-down-4k-after-m17f-high | 2/2 | 7.227 | 0.003 | 0.016 | 22.964 | 0.011 | 32.202 | 0.010 | 32.235 | 0.033 | 1192 (998-1228, n=36); 1200 (1010-1228, n=36) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 10.951 | 0.007 | 12.407 | 0.015 | 12.426 | 0.019 | 1101 (970-1151, n=15); 1096 (961-1153, n=15) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 24.149 | 0.026 | 26.456 | 0.001 | 26.470 | 0.015 | 1174 (294-1214, n=30); 1162 (1008-1216, n=29) |

## Ceiling check (Task 10 + origin/main merge, HEAD 32254d1b6)

After binary: release build of 32254d1b6, `capture,dev-tools`. Before binary: the pre-branch build (8dda8a34a) Tasks 6 and 7 used. Each binary baked `metal/maps/glossy-floor-only.map` with its own `prl-build --no-cache` (after: `--glossy-reflections`): 2 probes and 7 bindings, as in Task 7. The after `.prl` is byte-identical to one baked by the pre-Task-10 `prl-build` (25a74b0d1). 2 interleaved rounds, 300 + 300 frames, mask 17F, quality high; 16/16 runs valid. Pilot (`results-glossy-task10-flooronly-pilot.md`): 8/8 valid on the first attempt.

| ceiling | measured | ceiling | margin | result | Task 7 | change |
|---|---|---|---|---|---|---|
| overlay across, Auto | 0.879 | 0.76 | -0.119 (+15.7% over) | FAIL | 0.679 | +0.200 (+29.4%) |
| overlay across, 1440p | 2.181 | 1.83 | -0.351 (+19.2% over) | FAIL | 1.641 | +0.540 (+32.9%) |
| overlay across, 4K | 4.816 | 4.07 | -0.746 (+18.3% over) | FAIL | 3.629 | +1.187 (+32.7%) |
| overlay down, Auto | 1.359 | 1.43 | 0.071 | PASS | 1.031 | +0.328 (+31.9%) |
| overlay down, 1440p | 3.255 | 3.16 | -0.095 (+3.0% over) | FAIL | 2.466 | +0.789 (+32.0%) |
| overlay down, 4K | 7.227 | 7.05 | -0.177 (+2.5% over) | FAIL | 5.475 | +1.752 (+32.0%) |
| forward added, 4K across (24.459 - 24.149) | 0.310 | 0.41 | 0.100 | PASS | 0.306 | +0.004 |
| sums pass (worst arm) | 0.016 | 0.025 | 0.009 | PASS | 0.017 | -0.001 |

Forward added at 1440p across: 10.976 - 10.951 = 0.025 ms (Task 7: 0.016).

Machine state matches Task 7. The pre-branch arms' forward is within 0.04 ms of Task 7's (1440p 10.951 vs 10.933; 4K 24.149 vs 24.188), and clocks are in the same bands. Every overlay arm rose by the same ~32%, at every resolution and in both poses. That is a per-fragment shader cost, not a clock regime.

Attribution (`results-glossy-task10-attribution.md`, same session, same `.prl`, floor-only `glossy-across`):

| arm | 1440p | 4K |
|---|---|---|
| pre-Task-10 binary (25a74b0d1) | 1.645 | 3.657 |
| HEAD, smooth style (`reflection_style: smooth`) | 1.978 | 4.384 |
| HEAD, pixel style (the capture and play default) | 2.177 | 4.832 |

- **Pixel-style texel-centre point:** about 38% of the increase (1440p 0.199 of 0.532; 4K 0.448 of 1.175). That is `specular_texel_point`, with the world-position derivatives and Jacobian it needs, which the overlay now runs per fragment under pixel style.
- **Common to both styles:** the remaining ~62% (+0.333 / +0.727 ms, +20%). This fixture has no movers, so the mover-guard pass never records, and its floor material (`test/metal_smoothness_ramp`) has no `_n` and no Surface Depth carve. The march therefore returns flat. What remains is per-fragment overhead:
  - `surface_depth_resolve`'s flat early-out;
  - the `sample_normal` fetch and TBN rebuild;
  - the hemisphere-axis select;
  - the generic axis search;
  - the extra D_static alpha `textureLoad`;
  - the tangent decode with 3 more interpolated varyings (a smooth `world_normal` in place of a flat one).
  
  Any of these could lower occupancy through register pressure on this GPU; this run cannot split them further without shader variants.
- Smooth style alone still breaches across at 1440p (1.978 vs 1.83) and 4K (4.384 vs 4.07).
