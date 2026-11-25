# Glossy world reflections, 5300M (glossy-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.597 | n/a | 0.017 | 4.896 | n/a | 6.553 | n/a | 6.599 | 0.046 | 943 (566-985, n=10) |
| glossy-across-1440-after-m17f-high | 1/1 | 1.468 | n/a | 0.016 | 11.407 | n/a | 14.283 | n/a | 14.332 | 0.049 | 1093 (348-1158, n=20) |
| glossy-across-4k-after-m17f-high | 1/1 | 3.241 | n/a | 0.015 | 25.389 | n/a | 30.889 | n/a | 30.935 | 0.046 | 1198 (314-1216, n=36) |
| glossy-down-auto-after-m17f-high | 1/1 | 1.117 | n/a | 0.018 | 4.591 | n/a | 6.533 | n/a | 6.576 | 0.043 | 839 (721-1046, n=9) |
| glossy-down-1440-after-m17f-high | 1/1 | 2.516 | n/a | 0.016 | 10.252 | n/a | 13.956 | n/a | 14.004 | 0.048 | 1077 (967-1176, n=17) |
| glossy-down-4k-after-m17f-high | 1/1 | 5.636 | n/a | 0.020 | 22.965 | n/a | 30.578 | n/a | 30.625 | 0.048 | 1176 (297-1216, n=35) |
| glossy-across-auto-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 5.217 | n/a | 6.312 | n/a | 6.356 | 0.043 | 855 (292-1011, n=11) |
| glossy-across-auto-after-m17f-off | 1/1 | 0.000 | n/a | 0.000 | 5.045 | n/a | 6.121 | n/a | 6.121 | 0.000 | 891 (406-978, n=10) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 25.171 | n/a | 27.412 | n/a | 27.454 | 0.042 | 1142 (1046-1210, n=30) |
| glossy-across-4k-after-m17f-off | 1/1 | 0.000 | n/a | 0.000 | 25.044 | n/a | 27.252 | n/a | 27.297 | 0.044 | 1170 (1053-1212, n=30) |
| near-auto-before-m17f | 1/1 | 0.000 | n/a | 0.039 | 5.651 | n/a | 11.515 | n/a | 11.597 | 0.083 | 1053 (476-1127, n=15) |
| near-auto-after-m17f | 1/1 | 0.000 | n/a | 0.035 | 5.673 | n/a | 11.588 | n/a | 11.665 | 0.077 | 1028 (898-1124, n=14) |
| near-4k-before-m17f | 1/1 | 0.000 | n/a | 0.062 | 31.542 | n/a | 61.044 | n/a | 61.148 | 0.104 | 1219 (307-1227, n=66) |
| near-4k-after-m17f | 1/1 | 0.000 | n/a | 0.030 | 31.614 | n/a | 61.297 | n/a | 61.368 | 0.070 | 1215 (1019-1221, n=64) |
