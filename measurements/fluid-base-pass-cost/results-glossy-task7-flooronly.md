# Glossy world reflections, 5300M (glossy-task7-flooronly)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.679 | 0.012 | 0.017 | 4.764 | 0.064 | 6.530 | 0.095 | 6.554 | 0.024 | 922 (753-1028, n=9); 863 (775-965, n=9) |
| glossy-across-1440-after-m17f-high | 2/2 | 1.641 | 0.008 | 0.015 | 10.949 | 0.024 | 14.041 | 0.003 | 14.089 | 0.048 | 1102 (953-1159, n=17); 1094 (986-1154, n=17) |
| glossy-across-4k-after-m17f-high | 2/2 | 3.629 | 0.045 | 0.015 | 24.494 | 0.027 | 30.418 | 0.025 | 30.462 | 0.044 | 1199 (1038-1221, n=33); 1196 (989-1220, n=33) |
| glossy-down-auto-after-m17f-high | 2/2 | 1.031 | 0.015 | 0.017 | 4.328 | 0.052 | 6.167 | 0.081 | 6.206 | 0.039 | 933 (824-1003, n=9); 864 (805-1035, n=9) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.466 | 0.004 | 0.015 | 10.263 | 0.019 | 13.896 | 0.025 | 13.948 | 0.052 | 1109 (968-1161, n=17); 1051 (914-1162, n=17) |
| glossy-down-4k-after-m17f-high | 2/2 | 5.475 | 0.006 | 0.016 | 22.926 | 0.030 | 30.374 | 0.008 | 30.399 | 0.025 | 1172 (313-1229, n=35); 1171 (1037-1220, n=34) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 10.933 | 0.007 | 12.376 | 0.002 | 12.421 | 0.044 | 1030 (953-1130, n=15); 1031 (945-1176, n=15) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 24.188 | 0.014 | 26.460 | 0.075 | 26.497 | 0.037 | 1193 (1048-1215, n=29); 1176 (1030-1213, n=29) |

## Ceiling check (Task 7 colour mips + Task 9 exclusion, HEAD 3edd83dbb)

After binary: release build of 25a74b0d1's source (HEAD adds only map, texture, plan and tools files), `capture,dev-tools`. Before binary: the pre-branch build Task 6 used. Each binary baked `metal/maps/glossy-floor-only.map` with its own `prl-build --no-cache` (after: `--glossy-reflections`), 2 probes as in Task 6 (Task 9 now excludes 4 and 1 brushes from their captures). 2 interleaved rounds, 300 + 300 frames, mask 17F, quality high; 16/16 runs valid.

| ceiling | measured | ceiling | margin | result | Task 6 | change |
|---|---|---|---|---|---|---|
| overlay across, Auto | 0.679 | 0.76 | 0.081 | PASS | 0.607 | +0.072 (+11.9%) |
| overlay across, 1440p | 1.641 | 1.83 | 0.189 | PASS | 1.432 | +0.209 (+14.6%) |
| overlay across, 4K | 3.629 | 4.07 | 0.441 | PASS | 3.214 | +0.415 (+12.9%) |
| overlay down, Auto | 1.031 | 1.43 | 0.399 | PASS | 0.944* | +0.087 (+9.2%) |
| overlay down, 1440p | 2.466 | 3.16 | 0.694 | PASS | 2.270* | +0.196 (+8.6%) |
| overlay down, 4K | 5.475 | 7.05 | 1.575 | PASS | 5.039* | +0.436 (+8.7%) |
| forward added, 4K across (24.494 − 24.188) | 0.306 | 0.41 | 0.104 | PASS | 0.290 | +0.016 |
| sums pass (worst arm) | 0.017 | 0.025 | 0.008 | PASS | 0.017 | 0 |

\* Task 6 measured `down` on the Task 5 map with the north wall reflecting (`results-glossy-task6.md`), not on this fixture; the straight-down pose sees floor only, so it is the nearest comparison.

Forward added at 1440p across: 10.949 − 10.933 = 0.016 ms (Task 6: 0.014).

Pilot (`results-glossy-task7-flooronly-pilot.md`, 1 run per arm) was run twice: the first attempt stopped at `glossy-across-auto` r1, invalid on the complete/incomplete frame-time check (6.518 vs 6.731 ms, 3.3% > 3%), the known Auto-resolution trace row drop (Task 6's pilot hit it at `glossy-down-auto`); machine state was clean. The second attempt's valid run of the same label overwrote that run's record in `metal/runs/`; the figures above are from its log. The second attempt was 8/8 valid. Baseline on the current dev map: `results-glossy-task7-current.md`.
