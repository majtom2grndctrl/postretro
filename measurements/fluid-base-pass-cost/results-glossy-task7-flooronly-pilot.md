# Glossy world reflections, 5300M (glossy-task7-flooronly-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.675 | n/a | 0.017 | 4.723 | n/a | 6.490 | n/a | 6.531 | 0.040 | 868 (671-984, n=10) |
| glossy-across-1440-after-m17f-high | 1/1 | 1.638 | n/a | 0.015 | 10.974 | n/a | 14.064 | n/a | 14.115 | 0.051 | 1089 (978-1148, n=17) |
| glossy-across-4k-after-m17f-high | 1/1 | 3.607 | n/a | 0.015 | 24.497 | n/a | 30.412 | n/a | 30.451 | 0.039 | 1185 (1022-1221, n=34) |
| glossy-down-auto-after-m17f-high | 1/1 | 1.012 | n/a | 0.017 | 4.281 | n/a | 6.094 | n/a | 6.129 | 0.035 | 936 (813-1019, n=9) |
| glossy-down-1440-after-m17f-high | 1/1 | 2.465 | n/a | 0.017 | 10.239 | n/a | 13.879 | n/a | 13.931 | 0.052 | 1104 (934-1157, n=17) |
| glossy-down-4k-after-m17f-high | 1/1 | 5.466 | n/a | 0.017 | 22.929 | n/a | 30.362 | n/a | 30.406 | 0.044 | 1165 (1000-1219, n=34) |
| glossy-across-1440-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 10.933 | n/a | 12.390 | n/a | 12.390 | 0.000 | 1063 (959-1125, n=15) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 24.208 | n/a | 26.459 | n/a | 26.502 | 0.043 | 1166 (1030-1215, n=29) |
