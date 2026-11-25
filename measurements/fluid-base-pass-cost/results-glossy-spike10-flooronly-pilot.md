# Glossy world reflections, 5300M (glossy-spike10-flooronly-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 1.940 | n/a | 0.014 | 10.984 | n/a | 14.388 | n/a | 14.426 | 0.038 | 1078 (247-1155, n=21) |
| glossy-down-4k-after-m17f-high | 1/1 | 6.447 | n/a | 0.014 | 22.985 | n/a | 31.407 | n/a | 31.434 | 0.027 | 1168 (1064-1216, n=35) |
| glossy-across-1440-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 10.941 | n/a | 12.397 | n/a | 12.437 | 0.040 | 1063 (295-1152, n=17) |
| glossy-across-1440-head-m17f-high | 1/1 | 2.180 | n/a | 0.014 | 10.993 | n/a | 14.650 | n/a | 14.689 | 0.039 | 1118 (477-1156, n=18) |
