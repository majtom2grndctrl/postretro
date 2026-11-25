# Glossy world reflections, 5300M (glossy-task10-current)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 2/2 | 3.125 | 0.004 | 0.019 | 11.507 | 0.075 | 16.941 | 0.058 | 17.111 | 0.170 | 1081 (388-1168, n=22); 1093 (898-1170, n=20) |
| glossy-across-4k-after-m17f-high | 2/2 | 6.838 | 0.002 | 0.020 | 25.642 | 0.002 | 35.597 | 0.054 | 35.823 | 0.225 | 1204 (977-1230, n=39); 1201 (392-1220, n=41) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.527 | 0.002 | 0.020 | 10.562 | 0.042 | 15.260 | 0.054 | 15.413 | 0.153 | 1115 (460-1164, n=19); 1093 (471-1162, n=19) |
| glossy-down-4k-after-m17f-high | 2/2 | 5.639 | 0.011 | 0.021 | 23.795 | 0.015 | 32.455 | 0.024 | 32.660 | 0.204 | 1202 (276-1219, n=37); 1208 (287-1226, n=38) |

## Baseline on the current dev map (Task 10 + origin/main merge, HEAD 32254d1b6)

After binary only: release build of 32254d1b6, `capture,dev-tools`. `content/dev/maps/glossy-reflections-test.map` baked with `prl-build --no-cache --glossy-reflections -v`: 10 probes, 50 bindings, probe lines identical to the Task 8 bake. 2 interleaved rounds, 300 + 300 frames, mask 17F, quality high; 8/8 valid; pilot 4/4.

| arm | Task 10 | Task 7 | change |
|---|---|---|---|
| overlay across, 1440p | 3.125 | 2.106 | +1.019 (+48.4%) |
| overlay across, 4K | 6.838 | 4.637 | +2.201 (+47.5%) |
| overlay down, 1440p | 2.527 | 1.916 | +0.611 (+31.9%) |
| overlay down, 4K | 5.639 | 4.258 | +1.381 (+32.4%) |

`down` sees the floor alone and rises by the same 32% as the floor-only fixture. `across` also sees the carved glossy wall and ceiling, which Task 10 now marches with Surface Depth and reflects about their own hemisphere. The extra ~16 points there are the march and the wall/ceiling probe terms.
