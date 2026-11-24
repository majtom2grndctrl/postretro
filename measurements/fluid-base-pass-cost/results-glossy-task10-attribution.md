# Glossy world reflections, 5300M (glossy-task10-attribution)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-before-m17f-high | 2/2 | 1.645 | 0.020 | 0.014 | 10.971 | 0.011 | 14.050 | 0.045 | 14.083 | 0.033 | 1122 (523-1162, n=18); 1117 (965-1154, n=17) |
| glossy-across-1440-after-m17f-high | 2/2 | 2.177 | 0.001 | 0.014 | 10.976 | 0.017 | 14.631 | 0.041 | 14.669 | 0.037 | 1103 (1005-1169, n=18); 1140 (971-1177, n=18) |
| glossy-across-1440-stock-m17f-high | 2/2 | 1.978 | 0.009 | 0.014 | 11.008 | 0.055 | 14.445 | 0.049 | 14.483 | 0.038 | 1048 (354-1168, n=19); 1127 (253-1213, n=18) |
| glossy-across-4k-before-m17f-high | 2/2 | 3.657 | 0.015 | 0.013 | 24.533 | 0.048 | 30.504 | 0.057 | 30.520 | 0.016 | 1208 (1005-1217, n=34); 1210 (1018-1217, n=34) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.832 | 0.013 | 0.013 | 24.532 | 0.038 | 31.687 | 0.051 | 31.718 | 0.031 | 1205 (1011-1219, n=35); 1196 (992-1215, n=35) |
| glossy-across-4k-stock-m17f-high | 2/2 | 4.384 | 0.004 | 0.014 | 24.477 | 0.003 | 31.207 | 0.011 | 31.239 | 0.031 | 1210 (1018-1218, n=34); 1200 (1021-1229, n=34) |

Arms: `before` = the pre-Task-10 binary (25a74b0d1, `capture,dev-tools`) on its own `prl-build` bake of `metal/maps/glossy-floor-only.map`, byte-identical to the after bake. `after` = HEAD 32254d1b6, pixel style (the default). `stock` = HEAD 32254d1b6 on the after bake with `reflection_style: smooth` in the capture scene. The scratch wrapper over `run_ablation.py` changes nothing else. 2 interleaved rounds, 300 + 300 frames, mask 17F, quality high; 12/12 valid. Interpretation: `results-glossy-task10-flooronly.md`, Ceiling check.
