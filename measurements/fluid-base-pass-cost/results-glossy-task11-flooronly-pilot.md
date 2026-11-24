# Glossy world reflections, 5300M (glossy-task11-flooronly-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.829 | n/a | 0.016 | 0.195 | 0.648 | 4.821 | n/a | 7.562 | n/a | 7.590 | 0.028 | 883 (271-964, n=12) |
| glossy-across-1440-after-m17f-medium | 1/1 | 2.040 | n/a | 0.015 | 0.904 | 0.392 | 10.510 | n/a | 15.298 | n/a | 15.339 | 0.042 | 1060 (317-1161, n=20) |
| glossy-down-1440-after-m17f-very_high | 1/1 | 3.056 | n/a | 0.017 | 1.242 | 3.299 | 9.441 | n/a | 18.212 | n/a | 18.263 | 0.051 | 1045 (538-1170, n=22) |
| glossy-across-4k-after-m17f-ultra | 1/1 | 4.496 | n/a | 0.014 | 0.458 | 9.319 | 25.020 | n/a | 41.579 | n/a | 41.621 | 0.042 | 1197 (329-1221, n=47) |
| glossy-down-4k-after-m17f-high | 1/1 | 6.765 | n/a | 0.016 | 0.436 | 4.140 | 23.337 | n/a | 36.636 | n/a | 36.685 | 0.049 | 1153 (949-1226, n=41) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 0.000 | 0.000 | 24.208 | n/a | 26.499 | n/a | 26.543 | 0.043 | 1195 (1062-1216, n=30) |
| glossy-down-1440-after-m17f-low | 1/1 | 2.906 | n/a | 0.017 | 0.000 | 0.000 | 10.241 | n/a | 14.353 | n/a | 14.358 | 0.005 | 1108 (901-1141, n=18) |
| glossy-across-4k-after-m17f-low | 1/1 | 4.349 | n/a | 0.015 | 0.000 | 0.000 | 24.583 | n/a | 31.221 | n/a | 31.265 | 0.045 | 1191 (1024-1223, n=35) |
