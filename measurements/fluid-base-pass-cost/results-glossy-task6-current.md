# Glossy world reflections, 5300M (glossy-task6-current)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 2/2 | 1.885 | 0.001 | 0.020 | 11.641 | 0.018 | 16.171 | 0.013 | 16.342 | 0.171 | 1083 (934-1148, n=20); 1069 (943-1170, n=19) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.140 | 0.007 | 0.019 | 25.862 | 0.015 | 33.436 | 0.063 | 33.712 | 0.277 | 1182 (1007-1218, n=38); 1194 (1080-1217, n=37) |
| glossy-down-1440-after-m17f-high | 2/2 | 1.892 | 0.007 | 0.021 | 10.613 | 0.012 | 14.916 | 0.025 | 15.092 | 0.176 | 1067 (962-1135, n=18); 1085 (956-1155, n=18) |
| glossy-down-4k-after-m17f-high | 2/2 | 4.200 | 0.010 | 0.022 | 23.785 | 0.058 | 31.204 | 0.082 | 31.461 | 0.257 | 1179 (981-1219, n=35); 1197 (953-1218, n=34) |
