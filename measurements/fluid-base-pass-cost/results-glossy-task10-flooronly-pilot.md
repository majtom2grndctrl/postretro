# Glossy world reflections, 5300M (glossy-task10-flooronly-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.889 | n/a | 0.014 | 4.617 | n/a | 6.573 | n/a | 6.603 | 0.030 | 940 (902-1004, n=10) |
| glossy-across-1440-after-m17f-high | 1/1 | 2.184 | n/a | 0.013 | 10.973 | n/a | 14.620 | n/a | 14.657 | 0.037 | 1095 (277-1148, n=19) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.821 | n/a | 0.014 | 24.484 | n/a | 31.654 | n/a | 31.687 | 0.033 | 1202 (1036-1220, n=35) |
| glossy-down-auto-after-m17f-high | 1/1 | 1.364 | n/a | 0.017 | 4.182 | n/a | 6.336 | n/a | 6.368 | 0.032 | 912 (905-1032, n=9) |
| glossy-down-1440-after-m17f-high | 1/1 | 3.263 | n/a | 0.015 | 10.211 | n/a | 14.658 | n/a | 14.697 | 0.039 | 1116 (334-1167, n=19) |
| glossy-down-4k-after-m17f-high | 1/1 | 7.227 | n/a | 0.015 | 22.966 | n/a | 32.170 | n/a | 32.204 | 0.034 | 1198 (1001-1226, n=36) |
| glossy-across-1440-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 10.939 | n/a | 12.381 | n/a | 12.414 | 0.033 | 1067 (945-1138, n=15) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 24.136 | n/a | 26.419 | n/a | 26.446 | 0.028 | 1175 (1028-1213, n=29) |
