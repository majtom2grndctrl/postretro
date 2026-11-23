# Glossy world reflections, 5300M (glossy-task6-pilot2)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-down-auto-after-m17f-high | 1/1 | 0.942 | n/a | 0.019 | 4.304 | n/a | 6.055 | n/a | 6.095 | 0.039 | 895 (704-1008, n=9) |
| glossy-down-1440-after-m17f-high | 1/1 | 2.262 | n/a | 0.016 | 10.245 | n/a | 13.737 | n/a | 13.741 | 0.004 | 1076 (934-1171, n=17) |
| glossy-down-4k-after-m17f-high | 1/1 | 5.020 | n/a | 0.017 | 22.925 | n/a | 29.902 | n/a | 29.949 | 0.047 | 1159 (997-1228, n=34) |
| glossy-across-1440-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 11.358 | n/a | 12.761 | n/a | 12.806 | 0.045 | 1080 (356-1144, n=16) |
| glossy-across-1440-after-m17f-off | 1/1 | 0.000 | n/a | 0.000 | 11.343 | n/a | 12.759 | n/a | 12.808 | 0.049 | 1095 (965-1132, n=15) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 25.075 | n/a | 27.283 | n/a | 27.329 | 0.046 | 1183 (1008-1216, n=30) |
| glossy-across-4k-after-m17f-off | 1/1 | 0.000 | n/a | 0.000 | 25.039 | n/a | 27.250 | n/a | 27.250 | -0.000 | 1165 (1008-1223, n=30) |
| near-4k-before-m17f | 1/1 | 0.000 | n/a | 0.054 | 31.485 | n/a | 60.929 | n/a | 61.027 | 0.098 | 1219 (1080-1226, n=65) |
| near-4k-after-m17f | 1/1 | 0.000 | n/a | 0.032 | 31.489 | n/a | 61.045 | n/a | 61.114 | 0.070 | 1217 (947-1225, n=66) |
